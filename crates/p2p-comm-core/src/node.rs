use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use p2p_core::{DialHints, Endpoint, RelayConfig, Session};
use p2p_trust::{FileKeyStore, FileTrustStore, PeerId};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

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

/// One row in the sidebar: nickname or Peer ID prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarItem {
    pub peer_id_hex: String,
    pub label: String,
}

/// Immutable view of the node for the GUI thread.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub local_peer_id_hex: String,
    pub sidebar: Vec<SidebarItem>,
    pub selected: Option<String>,
    pub selected_status: Option<PeerStatus>,
    pub selected_error: Option<ChatError>,
}

/// Headless node: nicknames + sessions + dial/accept.
///
/// Holds live [`Session`]s so they stay open while the GUI switches views.
/// #4 does not send or receive application bytes.
pub struct Node {
    local_peer_id_hex: String,
    nicknames: NicknameStore,
    roster: Roster,
    sessions: HashMap<String, Session>,
    commands: mpsc::UnboundedSender<Command>,
    events: mpsc::UnboundedReceiver<Event>,
    accept: JoinHandle<()>,
    dialer: JoinHandle<()>,
}

impl Node {
    /// Unlock identity (or reuse the store from [`crate::unlock_in`]), bind the
    /// endpoint, load nicknames, and start the accept loop.
    ///
    /// # Errors
    ///
    /// * [`Error::EmptyPassword`] / [`Error::WrongPassword`] / [`Error::Io`] /
    ///   [`Error::CorruptStore`] from identity unlock
    /// * [`Error::Bind`] if the endpoint cannot bind
    pub async fn start(dir: &Path, password: &str) -> Result<Self, Error> {
        let identity = crate::unlock_key(dir, password)?;
        let local_peer_id_hex = to_hex(identity.peer_id().as_bytes());
        let mut keys = FileKeyStore::new(dir, password.as_bytes());
        let trust = FileTrustStore::open(dir, identity).map_err(map_trust)?;
        let endpoint = Endpoint::bind(&mut keys, Box::new(trust), RelayConfig::n0_public())
            .await
            .map_err(|_| Error::Bind)?;
        let nicknames = NicknameStore::load(dir)?;
        let endpoint = Arc::new(endpoint);

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (evt_tx, evt_rx) = mpsc::unbounded_channel();

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
            sessions: HashMap::new(),
            commands: cmd_tx,
            events: evt_rx,
            accept,
            dialer,
        })
    }

    /// Drain network events. Call from the GUI tick; never blocks.
    ///
    /// Returns `true` if any event was applied (so the GUI should repaint).
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.events.try_recv() {
            changed = true;
            match event {
                Event::SessionReady {
                    peer_id_hex,
                    session,
                } => {
                    self.sessions.insert(peer_id_hex.clone(), session);
                    self.roster.connected(peer_id_hex);
                }
                Event::ConnectFailed {
                    peer_id_hex,
                    message,
                } => self.roster.connect_failed(peer_id_hex, message),
            }
        }
        changed
    }

    /// Snapshot for painting the sidebar and chat pane.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        snapshot(&self.nicknames, &self.roster, &self.local_peer_id_hex)
    }

    /// Parse the dial box and start a connect. Invalid input does not go on the wire.
    ///
    /// # Errors
    ///
    /// * [`Error::InvalidPeerId`] / [`Error::UnknownNickname`] — shown at the dialer
    pub fn dial(&mut self, input: &str) -> Result<(), Error> {
        let (peer, hex) = resolve_dial(&self.nicknames, input)?;
        match self.roster.status(&hex) {
            Some(PeerStatus::Connected | PeerStatus::Connecting) => {
                self.roster.select(&hex);
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
    }

    /// Set or replace a local nickname. Writes `nicknames.json` immediately.
    ///
    /// # Errors
    ///
    /// See [`NicknameStore::set`].
    pub fn set_nickname(&mut self, peer_id_hex: &str, nickname: &str) -> Result<(), Error> {
        self.nicknames.set(peer_id_hex, nickname)
    }

    /// Delete a local nickname. Writes immediately. Display falls back to short ID.
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
}

impl Drop for Node {
    fn drop(&mut self) {
        self.accept.abort();
        self.dialer.abort();
        self.sessions.clear();
    }
}

fn snapshot(nicknames: &NicknameStore, roster: &Roster, local: &str) -> Snapshot {
    let mut seen = BTreeSet::new();
    let mut sidebar = Vec::new();
    let mut push = |peer: &str| {
        if seen.insert(peer.to_owned()) {
            sidebar.push(SidebarItem {
                peer_id_hex: peer.to_owned(),
                label: nicknames.display_name(peer),
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
    Snapshot {
        local_peer_id_hex: local.to_owned(),
        sidebar,
        selected,
        selected_status,
        selected_error,
    }
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
        let snap = snapshot(&nicks, &roster, "me");
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
        let snap = snapshot(&nicks, &roster, "me");
        assert_eq!(snap.sidebar.len(), 1);
        assert_eq!(snap.sidebar[0].label, short_id(&peer));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
