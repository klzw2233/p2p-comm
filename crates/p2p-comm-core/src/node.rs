use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use p2p_core::{DialHints, Endpoint, RelayConfig, Session};
use p2p_trust::{FileKeyStore, FileTrustStore, PeerId};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::chatlog::ChatKeys;
use crate::frame::{decode_frame, encode_text, Decoded};
use crate::inbox::{ChatMessage, Direction, Inbox};
use crate::nicknames::{resolve_dial, NicknameStore};
use crate::roster::{ChatError, PeerStatus, Roster};
use crate::{map_trust, to_hex, Error};

/// n0 public relays, matching iroh 1.1.0 `defaults::prod`.
///
/// `RelayConfig::n0_public()` only sets `RelayMode::Default` on *this* endpoint.
/// Outbound `dial` still needs URLs in [`DialHints`] or `P2PCore` maps a timeout
/// to [`p2p_core::Error::RelayUnreachable`].
const N0_RELAY_URLS: &[&str] = &[
    "https://use1-1.relay.n0.iroh.link.",
    "https://usw1-1.relay.n0.iroh.link.",
    "https://euc1-1.relay.n0.iroh.link.",
    "https://aps1-1.relay.n0.iroh.link.",
];

enum Command {
    Dial(PeerId, String),
}

enum Event {
    SessionReady {
        peer_id_hex: String,
        session: Session,
    },
    ConnectFailed {
        peer_id_hex: String,
        message: String,
    },
}

enum IoEvent {
    Incoming {
        peer_id_hex: String,
        content: String,
        timestamp: u64,
    },
    SendFailed {
        peer_id_hex: String,
    },
}

/// One row in the sidebar: nickname or Peer ID prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarItem {
    pub peer_id_hex: String,
    pub label: String,
    pub unread: u32,
}

/// Immutable view of the node for the GUI thread.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub local_peer_id_hex: String,
    pub sidebar: Vec<SidebarItem>,
    pub selected: Option<String>,
    pub selected_status: Option<PeerStatus>,
    pub selected_error: Option<ChatError>,
    pub messages: Vec<ChatMessage>,
}

/// Headless node: nicknames + sessions + dial/accept + text.
pub struct Node {
    local_peer_id_hex: String,
    nicknames: NicknameStore,
    roster: Roster,
    inbox: Inbox,
    keys: ChatKeys,
    live: HashMap<String, mpsc::UnboundedSender<Vec<u8>>>,
    workers: Vec<JoinHandle<()>>,
    commands: mpsc::UnboundedSender<Command>,
    events: mpsc::UnboundedReceiver<Event>,
    io_events: mpsc::UnboundedReceiver<IoEvent>,
    io_tx: mpsc::UnboundedSender<IoEvent>,
    accept: JoinHandle<()>,
    dialer: JoinHandle<()>,
}

impl Node {
    /// Unlock identity, bind the endpoint, load nicknames and chat keys.
    ///
    /// # Errors
    ///
    /// * identity / bind errors from #4
    /// * [`Error::Io`] / [`Error::CorruptStore`] from chat salt
    pub async fn start(dir: &Path, password: &str) -> Result<Self, Error> {
        let identity = crate::unlock_key(dir, password)?;
        let local_peer_id_hex = to_hex(identity.peer_id().as_bytes());
        let mut key_store = FileKeyStore::new(dir, password.as_bytes());
        let trust = FileTrustStore::open(dir, identity).map_err(map_trust)?;
        let endpoint = Endpoint::bind(&mut key_store, Box::new(trust), RelayConfig::n0_public())
            .await
            .map_err(|_| Error::Bind)?;
        let nicknames = NicknameStore::load(dir)?;
        let keys = ChatKeys::unlock(dir, password)?;
        let endpoint = Arc::new(endpoint);

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (evt_tx, evt_rx) = mpsc::unbounded_channel();
        let (io_tx, io_rx) = mpsc::unbounded_channel();

        let accept_ep = Arc::clone(&endpoint);
        let accept_tx = evt_tx.clone();
        let accept = tokio::spawn(async move {
            accept_loop(accept_ep, accept_tx).await;
        });

        let dial_ep = Arc::clone(&endpoint);
        let dialer = tokio::spawn(async move {
            dial_loop(dial_ep, cmd_rx, evt_tx).await;
        });

        Ok(Self {
            local_peer_id_hex,
            nicknames,
            roster: Roster::new(),
            inbox: Inbox::new(),
            keys,
            live: HashMap::new(),
            workers: Vec::new(),
            commands: cmd_tx,
            events: evt_rx,
            io_events: io_rx,
            io_tx,
            accept,
            dialer,
        })
    }

    /// Drain network events. Call from the GUI tick; never blocks.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.events.try_recv() {
            changed = true;
            match event {
                Event::SessionReady {
                    peer_id_hex,
                    session,
                } => self.attach_session(peer_id_hex, session),
                Event::ConnectFailed {
                    peer_id_hex,
                    message,
                } => self.roster.connect_failed(peer_id_hex, message),
            }
        }
        while let Ok(event) = self.io_events.try_recv() {
            changed = true;
            match event {
                IoEvent::Incoming {
                    peer_id_hex,
                    content,
                    timestamp,
                } => {
                    self.ensure_history(&peer_id_hex);
                    let msg = ChatMessage {
                        content,
                        timestamp,
                        direction: Direction::Incoming,
                        failed: false,
                    };
                    let selected = self.roster.selected().map(str::to_owned);
                    let _ = self.inbox.received(
                        &peer_id_hex,
                        msg,
                        selected.as_deref(),
                        Some(&self.keys),
                    );
                }
                IoEvent::SendFailed { peer_id_hex } => {
                    self.inbox.mark_last_failed(&peer_id_hex);
                    self.live.remove(&peer_id_hex);
                }
            }
        }
        changed
    }

    /// Snapshot for painting the sidebar and chat pane.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        snapshot(
            &self.nicknames,
            &self.roster,
            &self.inbox,
            &self.local_peer_id_hex,
        )
    }

    /// Parse the dial box and start a connect. Invalid input does not go on the wire.
    ///
    /// # Errors
    ///
    /// * [`Error::InvalidPeerId`] / [`Error::UnknownNickname`]
    pub fn dial(&mut self, input: &str) -> Result<(), Error> {
        let (peer, hex) = resolve_dial(&self.nicknames, input)?;
        match self.roster.status(&hex) {
            Some(PeerStatus::Connected | PeerStatus::Connecting) => {
                self.roster.select(&hex);
                self.inbox.clear_unread(&hex);
                return Ok(());
            }
            Some(PeerStatus::Failed) | None => {}
        }
        self.roster.begin_connect(hex.clone());
        let _ = self.commands.send(Command::Dial(peer, hex));
        Ok(())
    }

    /// Switch the chat view without closing the Session.
    pub fn select(&mut self, peer_id_hex: &str) {
        self.roster.select(peer_id_hex);
        self.inbox.clear_unread(peer_id_hex);
        self.ensure_history(peer_id_hex);
    }

    /// Send text to the selected-or-named connected Peer.
    ///
    /// # Errors
    ///
    /// * [`Error::NotConnected`] if there is no live Session
    /// * [`Error::Io`] if the log cannot be written
    pub fn send_text(&mut self, peer_id_hex: &str, content: &str) -> Result<(), Error> {
        let content = content.trim();
        if content.is_empty() {
            return Ok(());
        }
        let tx = self.live.get(peer_id_hex).ok_or(Error::NotConnected)?;
        let timestamp = unix_millis();
        let frame = encode_text(content, timestamp);
        let msg = ChatMessage {
            content: content.to_owned(),
            timestamp,
            direction: Direction::Outgoing,
            failed: false,
        };
        self.inbox.sent(peer_id_hex, msg, Some(&self.keys))?;
        if tx.send(frame).is_err() {
            self.inbox.mark_last_failed(peer_id_hex);
            self.live.remove(peer_id_hex);
        }
        Ok(())
    }

    /// Set or replace a local nickname.
    ///
    /// # Errors
    ///
    /// See [`NicknameStore::set`].
    pub fn set_nickname(&mut self, peer_id_hex: &str, nickname: &str) -> Result<(), Error> {
        self.nicknames.set(peer_id_hex, nickname)
    }

    /// Delete a local nickname.
    ///
    /// # Errors
    ///
    /// See [`NicknameStore::remove`].
    pub fn remove_nickname(&mut self, peer_id_hex: &str) -> Result<(), Error> {
        self.nicknames.remove(peer_id_hex)
    }

    #[must_use]
    pub fn display_name(&self, peer_id_hex: &str) -> String {
        self.nicknames.display_name(peer_id_hex)
    }

    #[must_use]
    pub fn local_peer_id_hex(&self) -> &str {
        &self.local_peer_id_hex
    }

    fn attach_session(&mut self, peer_id_hex: String, session: Session) {
        self.ensure_history(&peer_id_hex);
        let (tx, rx) = mpsc::unbounded_channel();
        self.live.insert(peer_id_hex.clone(), tx);
        self.roster.connected(peer_id_hex.clone());
        let io_tx = self.io_tx.clone();
        self.workers.push(tokio::spawn(async move {
            session_loop(peer_id_hex, session, rx, io_tx).await;
        }));
    }

    fn ensure_history(&mut self, peer_id_hex: &str) {
        if !self.inbox.messages(peer_id_hex).is_empty() {
            return;
        }
        match self.keys.load(peer_id_hex) {
            Ok(msgs) => self.inbox.load_peer(peer_id_hex, msgs),
            Err(_) => {
                self.roster.connect_failed(
                    peer_id_hex.to_owned(),
                    "Could not decrypt chat history.".into(),
                );
            }
        }
    }

    /// Test seam: drive send/recv over in-memory byte channels, no `P2PCore` Session.
    #[cfg(test)]
    pub(crate) fn test_node(dir: &Path, password: &str) -> Result<Self, Error> {
        let identity = crate::unlock_key(dir, password)?;
        let local_peer_id_hex = to_hex(identity.peer_id().as_bytes());
        let nicknames = NicknameStore::load(dir)?;
        let keys = ChatKeys::unlock(dir, password)?;
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        let (_evt_tx, evt_rx) = mpsc::unbounded_channel();
        let (io_tx, io_rx) = mpsc::unbounded_channel();
        Ok(Self {
            local_peer_id_hex,
            nicknames,
            roster: Roster::new(),
            inbox: Inbox::new(),
            keys,
            live: HashMap::new(),
            workers: Vec::new(),
            commands: cmd_tx,
            events: evt_rx,
            io_events: io_rx,
            io_tx,
            accept: tokio::spawn(async {}),
            dialer: tokio::spawn(async {}),
        })
    }

    #[cfg(test)]
    pub(crate) fn attach_byte_sink(
        &mut self,
        peer_id_hex: String,
        outbound: mpsc::UnboundedSender<Vec<u8>>,
    ) {
        self.ensure_history(&peer_id_hex);
        self.live.insert(peer_id_hex.clone(), outbound);
        self.roster.connected(peer_id_hex);
    }

    #[cfg(test)]
    pub(crate) fn push_incoming_bytes(&mut self, peer_id_hex: &str, bytes: &[u8]) {
        let mut rest = bytes;
        while !rest.is_empty() {
            match decode_frame(rest) {
                Ok((Decoded::Text { content, timestamp }, n)) => {
                    let msg = ChatMessage {
                        content,
                        timestamp,
                        direction: Direction::Incoming,
                        failed: false,
                    };
                    let selected = self.roster.selected().map(str::to_owned);
                    let _ = self.inbox.received(
                        peer_id_hex,
                        msg,
                        selected.as_deref(),
                        Some(&self.keys),
                    );
                    rest = &rest[n..];
                }
                Ok((Decoded::Ignored, n)) => rest = &rest[n..],
                Err(_) => break,
            }
        }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.accept.abort();
        self.dialer.abort();
        for w in &self.workers {
            w.abort();
        }
        self.live.clear();
    }
}

fn snapshot(
    nicknames: &NicknameStore,
    roster: &Roster,
    inbox: &Inbox,
    local: &str,
) -> Snapshot {
    let mut seen = BTreeSet::new();
    let mut sidebar = Vec::new();
    let mut push = |peer: &str| {
        if seen.insert(peer.to_owned()) {
            sidebar.push(SidebarItem {
                peer_id_hex: peer.to_owned(),
                label: nicknames.display_name(peer),
                unread: inbox.unread(peer),
            });
        }
    };
    for (peer, _) in nicknames.iter() {
        push(peer);
    }
    for peer in roster.peer_ids() {
        push(peer);
    }
    let selected = roster.selected().map(ToOwned::to_owned);
    let selected_status = selected.as_deref().and_then(|peer| roster.status(peer));
    let selected_error = selected
        .as_deref()
        .and_then(|peer| roster.error(peer))
        .cloned();
    let messages = selected
        .as_deref()
        .map(|peer| inbox.messages(peer).to_vec())
        .unwrap_or_default();
    Snapshot {
        local_peer_id_hex: local.to_owned(),
        sidebar,
        selected,
        selected_status,
        selected_error,
        messages,
    }
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

fn n0_hints() -> DialHints {
    DialHints::relays(N0_RELAY_URLS.iter().copied())
}

fn connect_error_text(err: &p2p_core::Error) -> String {
    match err {
        p2p_core::Error::PeerOffline => "Peer is offline.".to_owned(),
        p2p_core::Error::RelayUnreachable => "Relay is unreachable.".to_owned(),
        p2p_core::Error::AlreadyConnected { .. } => "Already connected to this Peer.".to_owned(),
        p2p_core::Error::Rejected { .. } => "Connection rejected.".to_owned(),
        p2p_core::Error::Alert { .. } => "Remote identity changed (TOFU alert).".to_owned(),
        p2p_core::Error::InvalidRelayUrl => "Invalid relay URL.".to_owned(),
        other => other.to_string(),
    }
}

async fn dial_loop(
    endpoint: Arc<Endpoint>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<Event>,
) {
    while let Some(Command::Dial(peer, hex)) = commands.recv().await {
        match endpoint.dial(peer, n0_hints()).await {
            Ok(session) => {
                let _ = events.send(Event::SessionReady {
                    peer_id_hex: hex,
                    session,
                });
            }
            Err(err) => {
                let _ = events.send(Event::ConnectFailed {
                    peer_id_hex: hex,
                    message: connect_error_text(&err),
                });
            }
        }
    }
}

async fn accept_loop(endpoint: Arc<Endpoint>, events: mpsc::UnboundedSender<Event>) {
    loop {
        match endpoint.accept().await {
            Ok(session) => {
                let hex = to_hex(session.remote_peer_id().as_bytes());
                if events
                    .send(Event::SessionReady {
                        peer_id_hex: hex,
                        session,
                    })
                    .is_err()
                {
                    break;
                }
            }
            Err(p2p_core::Error::Closed) => break,
            Err(_) => {}
        }
    }
}

async fn session_loop(
    peer_id_hex: String,
    mut session: Session,
    mut outbound: mpsc::UnboundedReceiver<Vec<u8>>,
    events: mpsc::UnboundedSender<IoEvent>,
) {
    let mut buf = Vec::new();
    let mut read = [0u8; 4096];
    loop {
        tokio::select! {
            outgoing = outbound.recv() => {
                match outgoing {
                    Some(bytes) => {
                        if session.send(&bytes).await.is_err() {
                            let _ = events.send(IoEvent::SendFailed { peer_id_hex: peer_id_hex.clone() });
                            break;
                        }
                    }
                    None => break,
                }
            }
            incoming = session.recv(&mut read) => {
                match incoming {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&read[..n]);
                        drain_frames(&peer_id_hex, &mut buf, &events);
                    }
                }
            }
        }
    }
}

fn drain_frames(peer_id_hex: &str, buf: &mut Vec<u8>, events: &mpsc::UnboundedSender<IoEvent>) {
    loop {
        match decode_frame(buf) {
            Ok((Decoded::Text { content, timestamp }, n)) => {
                let _ = events.send(IoEvent::Incoming {
                    peer_id_hex: peer_id_hex.to_owned(),
                    content,
                    timestamp,
                });
                buf.drain(..n);
            }
            Ok((Decoded::Ignored, n)) => {
                buf.drain(..n);
            }
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nicknames::{short_id, NicknameStore};
    use crate::tests_support::{temp_path, valid_peer_hex};

    #[test]
    fn sidebar_shows_nickname_or_short_id() {
        let dir = temp_path();
        let mut nicks = NicknameStore::load(&dir).expect("load");
        let alice = valid_peer_hex();
        let bob = valid_peer_hex();
        nicks.set(&alice, "Alice").expect("set");
        let mut roster = Roster::new();
        roster.connected(bob.clone());
        let inbox = Inbox::new();
        let snap = snapshot(&nicks, &roster, &inbox, "me");
        let labels: Vec<_> = snap.sidebar.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"Alice"));
        assert!(labels.iter().any(|l| *l == short_id(&bob)));
        assert!(!labels.contains(&bob.as_str()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inbound_unknown_peer_uses_short_id() {
        let dir = temp_path();
        let nicks = NicknameStore::load(&dir).expect("load");
        let peer = valid_peer_hex();
        let mut roster = Roster::new();
        roster.connected(peer.clone());
        let inbox = Inbox::new();
        let snap = snapshot(&nicks, &roster, &inbox, "me");
        assert_eq!(snap.sidebar.len(), 1);
        assert_eq!(snap.sidebar[0].label, short_id(&peer));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn send_and_recv_over_byte_sink() {
        let dir = temp_path();
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, mut rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.send_text(&peer, "hello").expect("send");
        let frame = rx.try_recv().expect("bytes");
        let (decoded, _) = decode_frame(&frame).expect("decode");
        match decoded {
            Decoded::Text { content, .. } => assert_eq!(content, "hello"),
            Decoded::Ignored => panic!("expected text"),
        }
        let inbound = encode_text("yo", 99);
        node.push_incoming_bytes(&peer, &inbound);
        let snap = node.snapshot();
        assert_eq!(snap.messages.len(), 2);
        assert_eq!(snap.messages[0].content, "hello");
        assert_eq!(snap.messages[1].content, "yo");
        assert!(!snap.messages[0].failed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn send_fail_and_unread() {
        let dir = temp_path();
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let a = valid_peer_hex();
        let b = valid_peer_hex();
        let (tx_a, _rx_a) = mpsc::unbounded_channel();
        drop(tx_a.clone());
        // closed sender: send_text marks failed
        node.attach_byte_sink(a.clone(), tx_a);
        // immediately drop the only remaining clone by replacing with a dead tx
        let (dead, rx) = mpsc::unbounded_channel();
        drop(rx);
        node.attach_byte_sink(a.clone(), dead);
        node.select(&a);
        node.send_text(&a, "nope").expect("queued then fail");
        assert!(node.snapshot().messages.last().is_some_and(|m| m.failed));

        let (tx_b, _rx_b) = mpsc::unbounded_channel();
        node.attach_byte_sink(b.clone(), tx_b);
        node.push_incoming_bytes(&b, &encode_text("ping", 1));
        let snap = node.snapshot();
        let unread = snap
            .sidebar
            .iter()
            .find(|i| i.peer_id_hex == b)
            .map_or(0, |i| i.unread);
        assert_eq!(unread, 1);
        node.select(&b);
        assert_eq!(
            node.snapshot()
                .sidebar
                .iter()
                .find(|i| i.peer_id_hex == b)
                .map(|i| i.unread),
            Some(0)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn history_survives_reload() {
        let dir = temp_path();
        let peer = valid_peer_hex();
        {
            let mut node = Node::test_node(&dir, "correct-horse").expect("node");
            let (tx, _rx) = mpsc::unbounded_channel();
            node.attach_byte_sink(peer.clone(), tx);
            node.select(&peer);
            node.send_text(&peer, "keep-me").expect("send");
        }
        let mut node = Node::test_node(&dir, "correct-horse").expect("reload");
        node.select(&peer);
        let snap = node.snapshot();
        assert_eq!(snap.messages.len(), 1);
        assert_eq!(snap.messages[0].content, "keep-me");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
