use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use p2p_core::{DialHints, Endpoint, RelayConfig, Session};
use p2p_trust::{FileKeyStore, FileTrustStore, PeerId, TrustState};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::audio_io::LiveMedia;
use crate::chatlog::ChatKeys;
use crate::frame::{
    decode_frame, encode_call_accept, encode_call_end, encode_call_invite, encode_call_reject,
    encode_file_accept, encode_file_chunk, encode_file_offer, encode_file_reject, encode_text,
    Decoded, MediaType,
};
use crate::inbox::{ChatMessage, Direction, Inbox};
use crate::nicknames::{resolve_dial, NicknameStore};
use crate::roster::{ChatError, PeerStatus, Roster};
use crate::video::{max_nal_len, VideoFrame, DEFAULT_MAX_DATAGRAM};
use crate::{map_trust, to_hex, Error};

/// How long the caller waits for `CallAccept` before giving up.
const INVITE_TIMEOUT: Duration = Duration::from_secs(30);

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
        trust: TrustState,
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
    FileOffer {
        peer_id_hex: String,
        name: String,
        size: u64,
        hash: [u8; 32],
    },
    FileAccept {
        peer_id_hex: String,
    },
    FileReject {
        peer_id_hex: String,
    },
    FileChunk {
        peer_id_hex: String,
        offset: u64,
        data: Vec<u8>,
    },
    SendProgress {
        peer_id_hex: String,
        transferred: u64,
    },
    SendFailed {
        peer_id_hex: String,
    },
    Disconnected {
        peer_id_hex: String,
    },
    CallInvite {
        peer_id_hex: String,
        media: MediaType,
    },
    CallAccept {
        peer_id_hex: String,
    },
    CallReject {
        peer_id_hex: String,
    },
    CallEnd {
        peer_id_hex: String,
    },
    Datagram {
        peer_id_hex: String,
        bytes: Vec<u8>,
    },
}

/// One row in the sidebar: nickname or Peer ID prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarItem {
    pub peer_id_hex: String,
    pub label: String,
    pub unread: u32,
}

/// Outcome of the in-flight (or just-finished) file transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferStatus {
    Offered,
    Transferring,
    Complete,
    Rejected,
    Failed,
}

/// Progress of the file transfer on the selected Peer, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProgress {
    pub name: String,
    pub size: u64,
    pub transferred: u64,
    pub direction: Direction,
    pub status: TransferStatus,
}

/// An inbound TOFU file offer awaiting accept/reject, for any Peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOffer {
    pub peer_id_hex: String,
    pub name: String,
    pub size: u64,
}

/// Phase of the single process-wide call, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallPhase {
    Outgoing,
    Incoming,
    Active,
}

/// Why an outgoing invite did not connect. Shown in the chat view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallResult {
    Rejected,
    TimedOut,
}

/// Snapshot of the process-wide call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallView {
    pub peer_id_hex: String,
    pub media: MediaType,
    pub phase: CallPhase,
}

/// An inbound call invite awaiting accept/reject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingInvite {
    pub peer_id_hex: String,
    pub media: MediaType,
}

enum CallState {
    Outgoing {
        peer_id_hex: String,
        media: MediaType,
        deadline: Instant,
    },
    Incoming {
        peer_id_hex: String,
        media: MediaType,
    },
    Active {
        peer_id_hex: String,
        media: MediaType,
    },
}

impl CallState {
    fn peer(&self) -> &str {
        match self {
            Self::Outgoing { peer_id_hex, .. }
            | Self::Incoming { peer_id_hex, .. }
            | Self::Active { peer_id_hex, .. } => peer_id_hex,
        }
    }

    fn view(&self) -> CallView {
        match self {
            Self::Outgoing {
                peer_id_hex, media, ..
            } => CallView {
                peer_id_hex: peer_id_hex.clone(),
                media: *media,
                phase: CallPhase::Outgoing,
            },
            Self::Incoming { peer_id_hex, media } => CallView {
                peer_id_hex: peer_id_hex.clone(),
                media: *media,
                phase: CallPhase::Incoming,
            },
            Self::Active { peer_id_hex, media } => CallView {
                peer_id_hex: peer_id_hex.clone(),
                media: *media,
                phase: CallPhase::Active,
            },
        }
    }
}

struct Transfer {
    name: String,
    size: u64,
    transferred: u64,
    direction: Direction,
    status: TransferStatus,
    hash: [u8; 32],
    bytes: Vec<u8>,
    buffer: Vec<u8>,
    /// Next `FileChunk` offset to enqueue. Outbound only.
    next_offset: u64,
}

struct IncomingOffer {
    name: String,
    size: u64,
    hash: [u8; 32],
}

const CHUNK: usize = 64 * 1024;

/// Immutable view of the node for the GUI thread.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub local_peer_id_hex: String,
    pub sidebar: Vec<SidebarItem>,
    pub selected: Option<String>,
    pub selected_status: Option<PeerStatus>,
    pub selected_error: Option<ChatError>,
    pub messages: Vec<ChatMessage>,
    pub transfer: Option<FileProgress>,
    pub pending_offer: Option<PendingOffer>,
    pub call: Option<CallView>,
    pub pending_invite: Option<PendingInvite>,
    pub call_result: Option<CallResult>,
}

/// Headless node: nicknames + sessions + dial/accept + text.
pub struct Node {
    local_peer_id_hex: String,
    nicknames: NicknameStore,
    roster: Roster,
    inbox: Inbox,
    keys: ChatKeys,
    live: HashMap<String, mpsc::UnboundedSender<Vec<u8>>>,
    transfers: HashMap<String, Transfer>,
    pending: HashMap<String, IncomingOffer>,
    trust: HashMap<String, TrustState>,
    download_dir: std::path::PathBuf,
    call: Option<CallState>,
    media: Option<LiveMedia>,
    dgram_tx: HashMap<String, mpsc::UnboundedSender<Vec<u8>>>,
    max_dgram: HashMap<String, usize>,
    call_result: Option<(String, CallResult)>,
    workers: Vec<JoinHandle<()>>,
    commands: mpsc::UnboundedSender<Command>,
    #[cfg(test)]
    commands_rx: Option<mpsc::UnboundedReceiver<Command>>,
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

        let download_dir = dirs::download_dir().unwrap_or_else(|| dir.to_path_buf());

        Ok(Self {
            local_peer_id_hex,
            nicknames,
            roster: Roster::new(),
            inbox: Inbox::new(),
            keys,
            live: HashMap::new(),
            transfers: HashMap::new(),
            pending: HashMap::new(),
            trust: HashMap::new(),
            download_dir,
            call: None,
            media: None,
            dgram_tx: HashMap::new(),
            max_dgram: HashMap::new(),
            call_result: None,
            workers: Vec::new(),
            commands: cmd_tx,
            #[cfg(test)]
            commands_rx: None,
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
                    trust,
                } => self.attach_session(peer_id_hex, session, trust),
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
                IoEvent::FileOffer {
                    peer_id_hex,
                    name,
                    size,
                    hash,
                } => self.handle_file_offer(&peer_id_hex, name, size, hash),
                IoEvent::FileAccept { peer_id_hex } => self.handle_file_accept(&peer_id_hex),
                IoEvent::FileReject { peer_id_hex } => self.handle_file_reject(&peer_id_hex),
                IoEvent::FileChunk {
                    peer_id_hex,
                    offset,
                    data,
                } => self.handle_file_chunk(&peer_id_hex, offset, &data),
                IoEvent::SendProgress {
                    peer_id_hex,
                    transferred,
                } => self.handle_send_progress(&peer_id_hex, transferred),
                IoEvent::SendFailed { peer_id_hex } => {
                    self.inbox.mark_last_failed(&peer_id_hex);
                    self.handle_disconnect(&peer_id_hex);
                }
                IoEvent::Disconnected { peer_id_hex } => {
                    self.handle_disconnect(&peer_id_hex);
                }
                IoEvent::CallInvite { peer_id_hex, media } => {
                    self.handle_call_invite(&peer_id_hex, media);
                }
                IoEvent::CallAccept { peer_id_hex } => self.handle_call_accept(&peer_id_hex),
                IoEvent::CallReject { peer_id_hex } => self.handle_call_reject(&peer_id_hex),
                IoEvent::CallEnd { peer_id_hex } => self.handle_call_end(&peer_id_hex),
                IoEvent::Datagram { peer_id_hex, bytes } => {
                    self.handle_datagram(&peer_id_hex, &bytes);
                }
            }
        }
        if self.expire_invite() {
            changed = true;
        }
        changed
    }

    /// Snapshot for painting the sidebar and chat pane.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        let mut snap = snapshot(
            &self.nicknames,
            &self.roster,
            &self.inbox,
            &self.transfers,
            &self.pending,
            &self.local_peer_id_hex,
        );
        snap.call = self.call.as_ref().map(CallState::view);
        snap.pending_invite = match &self.call {
            Some(CallState::Incoming { peer_id_hex, media }) => Some(PendingInvite {
                peer_id_hex: peer_id_hex.clone(),
                media: *media,
            }),
            _ => None,
        };
        snap.call_result = snap.selected.as_deref().and_then(|peer| {
            self.call_result
                .as_ref()
                .and_then(|(p, r)| (p == peer).then_some(*r))
        });
        snap
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
        if self.roster.selected() != Some(peer_id_hex) {
            self.call_result = None;
        }
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
        if self
            .call_result
            .as_ref()
            .is_some_and(|(p, _)| p == peer_id_hex)
        {
            self.call_result = None;
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

    /// Offer a local file to a connected Peer. SHA-256 is computed before `FileOffer`.
    ///
    /// # Errors
    ///
    /// * [`Error::NotConnected`] if there is no live Session
    /// * [`Error::Io`] if the file cannot be read
    pub fn send_file(&mut self, peer_id_hex: &str, path: &Path) -> Result<(), Error> {
        let tx = self.live.get(peer_id_hex).ok_or(Error::NotConnected)?;
        let bytes = std::fs::read(path).map_err(|_| Error::Io)?;
        let size = u64::try_from(bytes.len()).map_err(|_| Error::Io)?;
        let hash = sha256(&bytes);
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(Error::Io)?
            .to_owned();
        let frame = encode_file_offer(&name, size, hash);
        if tx.send(frame).is_err() {
            self.live.remove(peer_id_hex);
            return Err(Error::NotConnected);
        }
        self.transfers.insert(
            peer_id_hex.to_owned(),
            Transfer {
                name,
                size,
                transferred: 0,
                direction: Direction::Outgoing,
                status: TransferStatus::Offered,
                hash,
                bytes,
                buffer: Vec::new(),
                next_offset: 0,
            },
        );
        Ok(())
    }

    /// Accept a pending TOFU file offer.
    pub fn accept_file(&mut self, peer_id_hex: &str) {
        if let Some(offer) = self.pending.remove(peer_id_hex) {
            self.send_to_live(peer_id_hex, encode_file_accept());
            let size = offer.size;
            self.transfers.insert(
                peer_id_hex.to_owned(),
                Transfer {
                    name: offer.name,
                    size,
                    transferred: 0,
                    direction: Direction::Incoming,
                    status: TransferStatus::Transferring,
                    hash: offer.hash,
                    bytes: Vec::new(),
                    buffer: Vec::new(),
                    next_offset: 0,
                },
            );
            if size == 0 {
                // Zero-byte file: no chunks will ever arrive.
                self.finish_incoming_transfer(peer_id_hex);
            }
        }
    }

    /// Reject a pending TOFU file offer.
    pub fn reject_file(&mut self, peer_id_hex: &str) {
        if self.pending.remove(peer_id_hex).is_some() {
            self.send_to_live(peer_id_hex, encode_file_reject());
        }
    }

    /// Invite the connected Peer to an audio call.
    ///
    /// # Errors
    ///
    /// * [`Error::NotConnected`] if there is no live Session
    /// * [`Error::Busy`] if a call is already in progress
    pub fn invite_audio(&mut self, peer_id_hex: &str) -> Result<(), Error> {
        self.invite(peer_id_hex, MediaType::Audio)
    }

    /// Invite the connected Peer to an audio+video call.
    ///
    /// # Errors
    ///
    /// * [`Error::NotConnected`] if there is no live Session
    /// * [`Error::Busy`] if a call is already in progress
    pub fn invite_video(&mut self, peer_id_hex: &str) -> Result<(), Error> {
        self.invite(peer_id_hex, MediaType::AudioVideo)
    }

    fn invite(&mut self, peer_id_hex: &str, media: MediaType) -> Result<(), Error> {
        if self.call.is_some() {
            return Err(Error::Busy);
        }
        let _ = self.live.get(peer_id_hex).ok_or(Error::NotConnected)?;
        self.send_to_live(peer_id_hex, encode_call_invite(media));
        self.call_result = None;
        self.call = Some(CallState::Outgoing {
            peer_id_hex: peer_id_hex.to_owned(),
            media,
            deadline: Instant::now() + INVITE_TIMEOUT,
        });
        Ok(())
    }

    /// Accept a pending inbound invite. Opens the mic only after this.
    pub fn accept_call(&mut self, peer_id_hex: &str) {
        let Some(CallState::Incoming {
            peer_id_hex: p,
            media,
        }) = &self.call
        else {
            return;
        };
        if p != peer_id_hex {
            return;
        }
        let media = *media;
        self.send_to_live(peer_id_hex, encode_call_accept());
        self.enter_active(peer_id_hex, media);
    }

    /// Reject a pending inbound invite.
    pub fn reject_call(&mut self, peer_id_hex: &str) {
        if matches!(&self.call, Some(CallState::Incoming { peer_id_hex: p, .. }) if p == peer_id_hex)
        {
            self.send_to_live(peer_id_hex, encode_call_reject());
            self.call = None;
        }
    }

    /// Hang up the current call (outgoing, incoming, or active).
    pub fn hangup(&mut self) {
        let Some(call) = self.call.take() else {
            return;
        };
        let peer = call.peer().to_owned();
        self.send_to_live(&peer, encode_call_end());
        self.stop_media();
        self.call_result = None;
    }

    /// Queue a frame on the Peer's live session, if any.
    fn send_to_live(&self, peer_id_hex: &str, frame: Vec<u8>) {
        if let Some(tx) = self.live.get(peer_id_hex) {
            let _ = tx.send(frame);
        }
    }

    fn handle_file_offer(&mut self, peer_id_hex: &str, name: String, size: u64, hash: [u8; 32]) {
        match self
            .trust
            .get(peer_id_hex)
            .copied()
            .unwrap_or(TrustState::Unknown)
        {
            TrustState::Verified => {
                self.send_to_live(peer_id_hex, encode_file_accept());
                self.transfers.insert(
                    peer_id_hex.to_owned(),
                    Transfer {
                        name,
                        size,
                        transferred: 0,
                        direction: Direction::Incoming,
                        status: TransferStatus::Transferring,
                        hash,
                        bytes: Vec::new(),
                        buffer: Vec::new(),
                        next_offset: 0,
                    },
                );
                if size == 0 {
                    // Zero-byte file: no chunks will ever arrive.
                    self.finish_incoming_transfer(peer_id_hex);
                }
            }
            TrustState::Unknown => {
                self.send_to_live(peer_id_hex, encode_file_reject());
            }
            TrustState::Tofu => {
                self.pending
                    .insert(peer_id_hex.to_owned(), IncomingOffer { name, size, hash });
            }
        }
    }

    fn handle_file_accept(&mut self, peer_id_hex: &str) {
        if let Some(xfer) = self.transfers.get_mut(peer_id_hex) {
            if xfer.direction == Direction::Outgoing && xfer.status == TransferStatus::Offered {
                xfer.status = TransferStatus::Transferring;
                if xfer.bytes.is_empty() {
                    // Zero-byte file: nothing to queue, nothing on the wire.
                    xfer.status = TransferStatus::Complete;
                    return;
                }
                xfer.next_offset = 0;
                self.enqueue_next_chunk(peer_id_hex);
            }
        }
    }

    /// Enqueue at most one 64KiB `FileChunk`. The next waits for `SendProgress`.
    fn enqueue_next_chunk(&mut self, peer_id_hex: &str) {
        let Some(xfer) = self.transfers.get_mut(peer_id_hex) else {
            return;
        };
        if xfer.direction != Direction::Outgoing || xfer.status != TransferStatus::Transferring {
            return;
        }
        let offset = usize::try_from(xfer.next_offset).unwrap_or(usize::MAX);
        if offset >= xfer.bytes.len() {
            return;
        }
        let end = (offset + CHUNK).min(xfer.bytes.len());
        let frame = encode_file_chunk(xfer.next_offset, &xfer.bytes[offset..end]);
        xfer.next_offset = u64::try_from(end).unwrap_or(u64::MAX);
        self.send_to_live(peer_id_hex, frame);
    }

    fn handle_send_progress(&mut self, peer_id_hex: &str, transferred: u64) {
        let enqueue = {
            let Some(xfer) = self.transfers.get_mut(peer_id_hex) else {
                return;
            };
            if xfer.direction != Direction::Outgoing || xfer.status != TransferStatus::Transferring
            {
                return;
            }
            xfer.transferred = transferred;
            if transferred >= xfer.size {
                xfer.status = TransferStatus::Complete;
                xfer.bytes = Vec::new();
                false
            } else {
                true
            }
        };
        if enqueue {
            self.enqueue_next_chunk(peer_id_hex);
        }
    }

    fn handle_file_reject(&mut self, peer_id_hex: &str) {
        if let Some(xfer) = self.transfers.get_mut(peer_id_hex) {
            if xfer.direction == Direction::Outgoing {
                xfer.status = TransferStatus::Rejected;
            }
        }
    }

    fn handle_file_chunk(&mut self, peer_id_hex: &str, offset: u64, data: &[u8]) {
        let Some(xfer) = self.transfers.get_mut(peer_id_hex) else {
            return;
        };
        if xfer.direction != Direction::Incoming || xfer.status != TransferStatus::Transferring {
            return;
        }
        if offset != xfer.transferred {
            return; // Out-of-order or duplicate chunk; no resume in v1.
        }
        xfer.buffer.extend_from_slice(data);
        xfer.transferred += data.len() as u64;
        if xfer.transferred >= xfer.size {
            self.finish_incoming_transfer(peer_id_hex);
        }
    }

    /// Verify the assembled file and write it to the Downloads directory.
    /// SHA-256 mismatch or write failure marks the transfer Failed; a bad
    /// file is never treated as complete.
    fn finish_incoming_transfer(&mut self, peer_id_hex: &str) {
        let Some(xfer) = self.transfers.get_mut(peer_id_hex) else {
            return;
        };
        if xfer.direction != Direction::Incoming || xfer.status != TransferStatus::Transferring {
            return;
        }
        let ok = sha256(&xfer.buffer) == xfer.hash
            && std::fs::write(
                unique_download_path(&self.download_dir, safe_filename(&xfer.name)),
                &xfer.buffer,
            )
            .is_ok();
        xfer.status = if ok {
            TransferStatus::Complete
        } else {
            TransferStatus::Failed
        };
    }

    fn handle_disconnect(&mut self, peer_id_hex: &str) {
        self.roster
            .disconnected(peer_id_hex.to_owned(), "Connection lost.".into());
        if let Some(xfer) = self.transfers.get_mut(peer_id_hex) {
            if xfer.status == TransferStatus::Offered || xfer.status == TransferStatus::Transferring
            {
                xfer.status = TransferStatus::Failed;
            }
        }
        // An unanswered offer dies with the session; no resume in v1.
        self.pending.remove(peer_id_hex);
        self.live.remove(peer_id_hex);
        self.dgram_tx.remove(peer_id_hex);
        self.max_dgram.remove(peer_id_hex);
        if self.call.as_ref().is_some_and(|c| c.peer() == peer_id_hex) {
            self.stop_media();
            self.call = None;
        }
    }

    fn handle_call_invite(&mut self, peer_id_hex: &str, media: MediaType) {
        match self
            .trust
            .get(peer_id_hex)
            .copied()
            .unwrap_or(TrustState::Unknown)
        {
            TrustState::Unknown => {
                self.send_to_live(peer_id_hex, encode_call_reject());
            }
            TrustState::Verified | TrustState::Tofu => {
                if self.call.is_some() {
                    self.send_to_live(peer_id_hex, encode_call_reject());
                    return;
                }
                self.call_result = None;
                self.call = Some(CallState::Incoming {
                    peer_id_hex: peer_id_hex.to_owned(),
                    media,
                });
            }
        }
    }

    fn handle_call_accept(&mut self, peer_id_hex: &str) {
        let Some(CallState::Outgoing {
            peer_id_hex: p,
            media,
            ..
        }) = &self.call
        else {
            return;
        };
        if p != peer_id_hex {
            return;
        }
        let media = *media;
        self.enter_active(peer_id_hex, media);
    }

    fn handle_call_reject(&mut self, peer_id_hex: &str) {
        if matches!(&self.call, Some(CallState::Outgoing { peer_id_hex: p, .. }) if p == peer_id_hex)
        {
            self.call = None;
            self.call_result = Some((peer_id_hex.to_owned(), CallResult::Rejected));
        }
    }

    fn handle_call_end(&mut self, peer_id_hex: &str) {
        if self.call.as_ref().is_some_and(|c| c.peer() == peer_id_hex) {
            self.stop_media();
            self.call = None;
            self.call_result = None;
        }
    }

    fn handle_datagram(&mut self, peer_id_hex: &str, bytes: &[u8]) {
        if !matches!(&self.call, Some(CallState::Active { peer_id_hex: p, .. }) if p == peer_id_hex)
        {
            return;
        }
        if let Some(media) = self.media.as_mut() {
            media.push_datagram(bytes);
        }
    }

    fn enter_active(&mut self, peer_id_hex: &str, media: MediaType) {
        self.stop_media();
        if let Some(tx) = self.dgram_tx.get(peer_id_hex).cloned() {
            let max = self
                .max_dgram
                .get(peer_id_hex)
                .copied()
                .unwrap_or(DEFAULT_MAX_DATAGRAM);
            self.media = LiveMedia::start(tx, media, max_nal_len(max));
        }
        self.call = Some(CallState::Active {
            peer_id_hex: peer_id_hex.to_owned(),
            media,
        });
        self.call_result = None;
    }

    /// Latest decoded remote video frame, if any. Consumes the slot.
    #[must_use]
    pub fn take_video_frame(&self) -> Option<VideoFrame> {
        self.media.as_ref().and_then(LiveMedia::take_video_frame)
    }

    fn stop_media(&mut self) {
        self.media.take();
    }

    fn expire_invite(&mut self) -> bool {
        let Some(CallState::Outgoing {
            peer_id_hex,
            deadline,
            ..
        }) = &self.call
        else {
            return false;
        };
        if Instant::now() < *deadline {
            return false;
        }
        let peer = peer_id_hex.clone();
        self.send_to_live(&peer, encode_call_end());
        self.call = None;
        self.call_result = Some((peer, CallResult::TimedOut));
        true
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

    fn attach_session(&mut self, peer_id_hex: String, session: Session, trust: TrustState) {
        self.ensure_history(&peer_id_hex);
        let (tx, rx) = mpsc::unbounded_channel();
        let (dgram_tx, dgram_rx) = mpsc::unbounded_channel();
        self.live.insert(peer_id_hex.clone(), tx);
        self.dgram_tx.insert(peer_id_hex.clone(), dgram_tx);
        self.max_dgram.insert(
            peer_id_hex.clone(),
            session.max_datagram_size().unwrap_or(DEFAULT_MAX_DATAGRAM),
        );
        self.roster.connected(peer_id_hex.clone());
        self.trust.insert(peer_id_hex.clone(), trust);
        let io_tx = self.io_tx.clone();
        self.workers.push(tokio::spawn(async move {
            session_loop(peer_id_hex, session, rx, dgram_rx, io_tx).await;
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

    #[cfg(test)]
    pub(crate) fn test_node(dir: &Path, password: &str) -> Result<Self, Error> {
        let identity = crate::unlock_key(dir, password)?;
        let local_peer_id_hex = to_hex(identity.peer_id().as_bytes());
        let nicknames = NicknameStore::load(dir)?;
        let keys = ChatKeys::unlock(dir, password)?;
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (_evt_tx, evt_rx) = mpsc::unbounded_channel();
        let (io_tx, io_rx) = mpsc::unbounded_channel();
        Ok(Self {
            local_peer_id_hex,
            nicknames,
            roster: Roster::new(),
            inbox: Inbox::new(),
            keys,
            live: HashMap::new(),
            transfers: HashMap::new(),
            pending: HashMap::new(),
            trust: HashMap::new(),
            download_dir: dir.to_path_buf(),
            call: None,
            media: None,
            dgram_tx: HashMap::new(),
            max_dgram: HashMap::new(),
            call_result: None,
            workers: Vec::new(),
            commands: cmd_tx,
            commands_rx: Some(cmd_rx),
            events: evt_rx,
            io_events: io_rx,
            io_tx,
            accept: tokio::spawn(async {}),
            dialer: tokio::spawn(async {}),
        })
    }

    /// Peer ID hex of the next queued dial command, if any. Test seam.
    #[cfg(test)]
    pub(crate) fn next_dial_command(&mut self) -> Option<String> {
        match self.commands_rx.as_mut()?.try_recv() {
            Ok(Command::Dial(_, hex)) => Some(hex),
            Err(_) => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_trust(&mut self, peer_id_hex: &str, state: TrustState) {
        self.trust.insert(peer_id_hex.to_owned(), state);
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
    pub(crate) fn attach_dgram_sink(
        &mut self,
        peer_id_hex: String,
        outbound: mpsc::UnboundedSender<Vec<u8>>,
        max_datagram: usize,
    ) {
        self.dgram_tx.insert(peer_id_hex.clone(), outbound);
        self.max_dgram.insert(peer_id_hex, max_datagram);
    }

    #[cfg(test)]
    pub(crate) fn set_invite_deadline_past(&mut self) {
        if let Some(CallState::Outgoing { deadline, .. }) = &mut self.call {
            *deadline = Instant::now()
                .checked_sub(Duration::from_secs(1))
                .unwrap_or_else(Instant::now);
        }
    }

    #[cfg(test)]
    pub(crate) fn push_incoming_bytes(&mut self, peer_id_hex: &str, bytes: &[u8]) {
        let mut rest = bytes;
        while !rest.is_empty() {
            match decode_frame(rest) {
                Ok((decoded, n)) => {
                    match decoded {
                        Decoded::Text { content, timestamp } => {
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
                        }
                        Decoded::FileOffer { name, size, hash } => {
                            self.handle_file_offer(peer_id_hex, name, size, hash);
                        }
                        Decoded::FileAccept => self.handle_file_accept(peer_id_hex),
                        Decoded::FileReject => self.handle_file_reject(peer_id_hex),
                        Decoded::FileChunk { offset, data } => {
                            self.handle_file_chunk(peer_id_hex, offset, &data);
                        }
                        Decoded::CallInvite { media } => {
                            self.handle_call_invite(peer_id_hex, media);
                        }
                        Decoded::CallAccept => self.handle_call_accept(peer_id_hex),
                        Decoded::CallReject => self.handle_call_reject(peer_id_hex),
                        Decoded::CallEnd => self.handle_call_end(peer_id_hex),
                        Decoded::Ignored => {}
                    }
                    rest = &rest[n..];
                }
                Err(_) => break,
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn push_incoming_dgram(&mut self, peer_id_hex: &str, bytes: &[u8]) {
        self.handle_datagram(peer_id_hex, bytes);
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
        self.dgram_tx.clear();
        self.max_dgram.clear();
        self.stop_media();
    }
}

fn snapshot(
    nicknames: &NicknameStore,
    roster: &Roster,
    inbox: &Inbox,
    transfers: &HashMap<String, Transfer>,
    pending: &HashMap<String, IncomingOffer>,
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
    let transfer = selected.as_deref().and_then(|peer| {
        transfers.get(peer).map(|t| FileProgress {
            name: t.name.clone(),
            size: t.size,
            transferred: t.transferred,
            direction: t.direction,
            status: t.status,
        })
    });
    let pending_offer = pending.iter().next().map(|(peer, p)| PendingOffer {
        peer_id_hex: peer.clone(),
        name: p.name.clone(),
        size: p.size,
    });
    Snapshot {
        local_peer_id_hex: local.to_owned(),
        sidebar,
        selected,
        selected_status,
        selected_error,
        messages,
        transfer,
        pending_offer,
        call: None,
        pending_invite: None,
        call_result: None,
    }
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).into()
}

/// Strip path separators so an offer cannot write outside Downloads.
fn safe_filename(name: &str) -> &str {
    name.rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .unwrap_or("download")
}

/// If `name` already exists in `dir`, use `stem (1).ext`, `stem (2).ext`, …
fn unique_download_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("download");
    let ext = path.extension().and_then(|s| s.to_str());
    // ponytail: sequential exists() scan; fine for Downloads, not a hot path
    for n in 1u32.. {
        let numbered = match ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = dir.join(&numbered);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
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
                let trust = trust_of(&endpoint, &session.remote_peer_id());
                let _ = events.send(Event::SessionReady {
                    peer_id_hex: hex,
                    session,
                    trust,
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
                let trust = trust_of(&endpoint, &session.remote_peer_id());
                if events
                    .send(Event::SessionReady {
                        peer_id_hex: hex,
                        session,
                        trust,
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

/// Trust of a connected Peer; Unknown on any store error (offer then auto-rejects).
fn trust_of(endpoint: &Endpoint, peer: &PeerId) -> TrustState {
    endpoint.trust_state(peer).unwrap_or(TrustState::Unknown)
}

async fn session_loop(
    peer_id_hex: String,
    mut session: Session,
    mut outbound: mpsc::UnboundedReceiver<Vec<u8>>,
    mut outbound_dgram: mpsc::UnboundedReceiver<Vec<u8>>,
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
                        report_send_progress(&peer_id_hex, &bytes, &events);
                    }
                    None => break,
                }
            }
            dgram = outbound_dgram.recv() => {
                match dgram {
                    Some(bytes) => {
                        let _ = session.send_datagram(&bytes);
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
            // ponytail: 5 ms poll; Session can't be mut-borrowed by recv and
            // recv_datagram in one select. Dedicated dgram task if P2PCore splits Session.
            () = tokio::time::sleep(Duration::from_millis(5)) => {}
        }
        match drain_datagrams(&peer_id_hex, &session, &events).await {
            Ok(()) => {}
            Err(()) => break,
        }
    }
    let _ = events.send(IoEvent::Disconnected {
        peer_id_hex: peer_id_hex.clone(),
    });
}

/// Drain already-buffered datagrams. Session cannot be borrowed by
/// `recv` and `recv_datagram` in the same `select!`.
async fn drain_datagrams(
    peer_id_hex: &str,
    session: &Session,
    events: &mpsc::UnboundedSender<IoEvent>,
) -> Result<(), ()> {
    loop {
        tokio::select! {
            biased;
            result = session.recv_datagram() => {
                match result {
                    Ok(bytes) => {
                        let _ = events.send(IoEvent::Datagram {
                            peer_id_hex: peer_id_hex.to_owned(),
                            bytes,
                        });
                    }
                    Err(_) => return Err(()),
                }
            }
            () = std::future::ready(()) => return Ok(()),
        }
    }
}

/// Sender-side progress: report bytes actually on the wire, not bytes queued.
fn report_send_progress(peer_id_hex: &str, bytes: &[u8], events: &mpsc::UnboundedSender<IoEvent>) {
    if let Ok((Decoded::FileChunk { offset, data }, _)) = decode_frame(bytes) {
        let _ = events.send(IoEvent::SendProgress {
            peer_id_hex: peer_id_hex.to_owned(),
            transferred: offset + data.len() as u64,
        });
    }
}

fn drain_frames(peer_id_hex: &str, buf: &mut Vec<u8>, events: &mpsc::UnboundedSender<IoEvent>) {
    while let Ok((decoded, n)) = decode_frame(buf) {
        if let Some(event) = io_event(peer_id_hex, decoded) {
            let _ = events.send(event);
        }
        buf.drain(..n);
    }
}

fn io_event(peer_id_hex: &str, decoded: Decoded) -> Option<IoEvent> {
    let peer_id_hex = peer_id_hex.to_owned();
    match decoded {
        Decoded::Text { content, timestamp } => Some(IoEvent::Incoming {
            peer_id_hex,
            content,
            timestamp,
        }),
        Decoded::FileOffer { name, size, hash } => Some(IoEvent::FileOffer {
            peer_id_hex,
            name,
            size,
            hash,
        }),
        Decoded::FileAccept => Some(IoEvent::FileAccept { peer_id_hex }),
        Decoded::FileReject => Some(IoEvent::FileReject { peer_id_hex }),
        Decoded::FileChunk { offset, data } => Some(IoEvent::FileChunk {
            peer_id_hex,
            offset,
            data,
        }),
        Decoded::CallInvite { media } => Some(IoEvent::CallInvite { peer_id_hex, media }),
        Decoded::CallAccept => Some(IoEvent::CallAccept { peer_id_hex }),
        Decoded::CallReject => Some(IoEvent::CallReject { peer_id_hex }),
        Decoded::CallEnd => Some(IoEvent::CallEnd { peer_id_hex }),
        Decoded::Ignored => None,
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
        let snap = snapshot(
            &nicks,
            &roster,
            &inbox,
            &HashMap::new(),
            &HashMap::new(),
            "me",
        );
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
        let snap = snapshot(
            &nicks,
            &roster,
            &inbox,
            &HashMap::new(),
            &HashMap::new(),
            "me",
        );
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
            _ => panic!("expected text"),
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

    #[tokio::test]
    async fn send_file_emits_offer_and_shows_progress() {
        let dir = temp_path();
        let src = dir.join("notes.txt");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(&src, b"hello world!").expect("write");
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, mut rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.send_file(&peer, &src).expect("offer");
        let frame = rx.try_recv().expect("bytes");
        let (decoded, _) = decode_frame(&frame).expect("decode");
        match decoded {
            Decoded::FileOffer { name, size, hash } => {
                assert_eq!(name, "notes.txt");
                assert_eq!(size, 12);
                assert_eq!(
                    crate::to_hex(&hash),
                    "7509e5bda0c762d2bac7f90d758b5b2263fa01ccbc542ab5e3df163be08e6ca9"
                );
            }
            other => panic!("expected FileOffer, got {other:?}"),
        }
        let snap = node.snapshot();
        let xfer = snap.transfer.expect("progress");
        assert_eq!(xfer.name, "notes.txt");
        assert_eq!(xfer.size, 12);
        assert_eq!(xfer.transferred, 0);
        assert_eq!(xfer.direction, Direction::Outgoing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn verified_peer_auto_accepts_file_offer() {
        let dir = temp_path();
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, mut rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.set_trust(&peer, p2p_trust::TrustState::Verified);
        let hash = [0xab; 32];
        node.push_incoming_bytes(&peer, &encode_file_offer("notes.txt", 12, hash));
        let frame = rx.try_recv().expect("accept");
        let (decoded, _) = decode_frame(&frame).expect("decode");
        assert_eq!(decoded, Decoded::FileAccept);
        let xfer = node.snapshot().transfer.expect("progress");
        assert_eq!(xfer.name, "notes.txt");
        assert_eq!(xfer.size, 12);
        assert_eq!(xfer.transferred, 0);
        assert_eq!(xfer.direction, Direction::Incoming);
        assert_eq!(xfer.status, TransferStatus::Transferring);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unknown_peer_rejects_file_offer() {
        let dir = temp_path();
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, mut rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.set_trust(&peer, p2p_trust::TrustState::Unknown);
        let hash = [0xab; 32];
        node.push_incoming_bytes(&peer, &encode_file_offer("notes.txt", 12, hash));
        let frame = rx.try_recv().expect("reject");
        let (decoded, _) = decode_frame(&frame).expect("decode");
        assert_eq!(decoded, Decoded::FileReject);
        assert!(node.snapshot().transfer.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn file_accept_triggers_chunk_sending() {
        let dir = temp_path();
        let src = dir.join("test.bin");
        std::fs::create_dir_all(&dir).expect("dir");
        let content = vec![0xaa; 128 * 1024]; // 128 KiB → 2 chunks
        std::fs::write(&src, &content).expect("write");
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, mut rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.send_file(&peer, &src).expect("offer");
        let _ = rx.try_recv().expect("offer frame");
        node.push_incoming_bytes(&peer, &encode_file_accept());
        let (decoded, _) = decode_frame(&rx.try_recv().expect("chunk 0")).expect("decode");
        match decoded {
            Decoded::FileChunk { offset, data } => {
                assert_eq!(offset, 0);
                assert_eq!(data.len(), 64 * 1024);
            }
            other => panic!("expected FileChunk, got {other:?}"),
        }
        assert!(
            rx.try_recv().is_err(),
            "second chunk must not occupy the queue before the first is written"
        );
        // Sender progress arrives as session_loop reports bytes on the wire.
        let _ = node.io_tx.send(IoEvent::SendProgress {
            peer_id_hex: peer.clone(),
            transferred: 64 * 1024,
        });
        node.poll();
        assert_eq!(
            node.snapshot().transfer.expect("xfer").transferred,
            64 * 1024
        );
        let (decoded, _) = decode_frame(&rx.try_recv().expect("chunk 1")).expect("decode");
        match decoded {
            Decoded::FileChunk { offset, data } => {
                assert_eq!(offset, 64 * 1024);
                assert_eq!(data.len(), 64 * 1024);
            }
            other => panic!("expected FileChunk, got {other:?}"),
        }
        let _ = node.io_tx.send(IoEvent::SendProgress {
            peer_id_hex: peer.clone(),
            transferred: 128 * 1024,
        });
        node.poll();
        let xfer = node.snapshot().transfer.expect("xfer");
        assert_eq!(xfer.status, TransferStatus::Complete);
        assert_eq!(xfer.transferred, 128 * 1024);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn text_can_interleave_between_file_chunks() {
        let dir = temp_path();
        let src = dir.join("test.bin");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(&src, vec![0xaa; 128 * 1024]).expect("write");
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, mut rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.send_file(&peer, &src).expect("offer");
        let _ = rx.try_recv().expect("offer frame");
        node.push_incoming_bytes(&peer, &encode_file_accept());

        let (decoded, _) = decode_frame(&rx.try_recv().expect("chunk 0")).expect("decode");
        match decoded {
            Decoded::FileChunk { offset, data } => {
                assert_eq!(offset, 0);
                assert_eq!(data.len(), 64 * 1024);
            }
            other => panic!("expected FileChunk, got {other:?}"),
        }
        assert!(
            rx.try_recv().is_err(),
            "second chunk must not occupy the queue before the first is written"
        );

        node.send_text(&peer, "hello").expect("text");
        let _ = node.io_tx.send(IoEvent::SendProgress {
            peer_id_hex: peer.clone(),
            transferred: 64 * 1024,
        });
        node.poll();

        let (decoded, _) = decode_frame(&rx.try_recv().expect("text")).expect("decode");
        match decoded {
            Decoded::Text { content, .. } => assert_eq!(content, "hello"),
            other => panic!("expected Text between chunks, got {other:?}"),
        }
        let (decoded, _) = decode_frame(&rx.try_recv().expect("chunk 1")).expect("decode");
        match decoded {
            Decoded::FileChunk { offset, data } => {
                assert_eq!(offset, 64 * 1024);
                assert_eq!(data.len(), 64 * 1024);
            }
            other => panic!("expected FileChunk, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn call_end_can_interleave_between_file_chunks() {
        let dir = temp_path();
        let src = dir.join("test.bin");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(&src, vec![0xaa; 128 * 1024]).expect("write");
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, mut rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.invite_audio(&peer).expect("invite");
        let (decoded, _) = decode_frame(&rx.try_recv().expect("invite")).expect("decode");
        assert!(matches!(decoded, Decoded::CallInvite { .. }));
        node.send_file(&peer, &src).expect("offer");
        let _ = rx.try_recv().expect("offer frame");
        node.push_incoming_bytes(&peer, &encode_file_accept());

        let (decoded, _) = decode_frame(&rx.try_recv().expect("chunk 0")).expect("decode");
        assert!(matches!(decoded, Decoded::FileChunk { offset: 0, .. }));
        assert!(
            rx.try_recv().is_err(),
            "second chunk must not occupy the queue before the first is written"
        );

        node.hangup();
        let _ = node.io_tx.send(IoEvent::SendProgress {
            peer_id_hex: peer.clone(),
            transferred: 64 * 1024,
        });
        node.poll();

        let (decoded, _) = decode_frame(&rx.try_recv().expect("end")).expect("decode");
        assert_eq!(decoded, Decoded::CallEnd);
        let (decoded, _) = decode_frame(&rx.try_recv().expect("chunk 1")).expect("decode");
        match decoded {
            Decoded::FileChunk { offset, data } => {
                assert_eq!(offset, 64 * 1024);
                assert_eq!(data.len(), 64 * 1024);
            }
            other => panic!("expected FileChunk, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn outbound_zero_byte_file_completes_without_chunks() {
        let dir = temp_path();
        let src = dir.join("empty.txt");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(&src, b"").expect("write");
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, mut rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.send_file(&peer, &src).expect("offer");
        let (decoded, _) = decode_frame(&rx.try_recv().expect("offer")).expect("decode");
        match decoded {
            Decoded::FileOffer { name, size, .. } => {
                assert_eq!(name, "empty.txt");
                assert_eq!(size, 0);
            }
            other => panic!("expected FileOffer, got {other:?}"),
        }
        node.push_incoming_bytes(&peer, &encode_file_accept());
        assert!(
            rx.try_recv().is_err(),
            "zero-byte file must not emit FileChunk"
        );
        let xfer = node.snapshot().transfer.expect("xfer");
        assert_eq!(xfer.status, TransferStatus::Complete);
        assert_eq!(xfer.transferred, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn file_reject_marks_status() {
        let dir = temp_path();
        let src = dir.join("test.txt");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(&src, b"data").expect("write");
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, mut rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.send_file(&peer, &src).expect("offer");
        let _ = rx.try_recv().expect("offer");
        node.push_incoming_bytes(&peer, &encode_file_reject());
        node.poll();
        let xfer = node.snapshot().transfer.expect("xfer");
        assert_eq!(xfer.status, TransferStatus::Rejected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn receive_chunks_and_verify_hash() {
        let dir = temp_path();
        std::fs::create_dir_all(&dir).expect("dir");
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, _rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.set_trust(&peer, p2p_trust::TrustState::Verified);
        let data = b"hello world!";
        let hash = sha256(data);
        node.push_incoming_bytes(
            &peer,
            &encode_file_offer("recv.txt", data.len() as u64, hash),
        );
        node.poll();
        node.push_incoming_bytes(&peer, &encode_file_chunk(0, data));
        node.poll();
        let xfer = node.snapshot().transfer.expect("xfer");
        assert_eq!(xfer.status, TransferStatus::Complete);
        let downloaded = dir.join("recv.txt");
        assert!(downloaded.exists());
        assert_eq!(std::fs::read(&downloaded).expect("read"), data);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn receive_same_name_writes_numbered_copy_without_overwriting() {
        let dir = temp_path();
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("notes.txt"), b"original").expect("seed");
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, _rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.set_trust(&peer, p2p_trust::TrustState::Verified);
        let data = b"new content";
        let hash = sha256(data);
        node.push_incoming_bytes(
            &peer,
            &encode_file_offer("notes.txt", data.len() as u64, hash),
        );
        node.poll();
        node.push_incoming_bytes(&peer, &encode_file_chunk(0, data));
        node.poll();
        let xfer = node.snapshot().transfer.expect("xfer");
        assert_eq!(xfer.status, TransferStatus::Complete);
        assert_eq!(
            std::fs::read(dir.join("notes.txt")).expect("orig"),
            b"original"
        );
        assert_eq!(
            std::fs::read(dir.join("notes (1).txt")).expect("copy"),
            data
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn receive_same_name_skips_existing_numbered_copy() {
        let dir = temp_path();
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("notes.txt"), b"original").expect("seed");
        std::fs::write(dir.join("notes (1).txt"), b"taken").expect("seed-1");
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, _rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.set_trust(&peer, p2p_trust::TrustState::Verified);
        let data = b"new content";
        let hash = sha256(data);
        node.push_incoming_bytes(
            &peer,
            &encode_file_offer("notes.txt", data.len() as u64, hash),
        );
        node.poll();
        node.push_incoming_bytes(&peer, &encode_file_chunk(0, data));
        node.poll();
        let xfer = node.snapshot().transfer.expect("xfer");
        assert_eq!(xfer.status, TransferStatus::Complete);
        assert_eq!(
            std::fs::read(dir.join("notes.txt")).expect("orig"),
            b"original"
        );
        assert_eq!(
            std::fs::read(dir.join("notes (1).txt")).expect("kept"),
            b"taken"
        );
        assert_eq!(
            std::fs::read(dir.join("notes (2).txt")).expect("copy"),
            data
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn receive_chunks_bad_hash_fails() {
        let dir = temp_path();
        std::fs::create_dir_all(&dir).expect("dir");
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, _rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.set_trust(&peer, p2p_trust::TrustState::Verified);
        let data = b"corrupted";
        let wrong_hash = [0xff; 32];
        node.push_incoming_bytes(
            &peer,
            &encode_file_offer("bad.txt", data.len() as u64, wrong_hash),
        );
        node.poll();
        node.push_incoming_bytes(&peer, &encode_file_chunk(0, data));
        node.poll();
        let xfer = node.snapshot().transfer.expect("xfer");
        assert_eq!(xfer.status, TransferStatus::Failed);
        let downloaded = dir.join("bad.txt");
        assert!(!downloaded.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn disconnect_during_transfer_fails() {
        let dir = temp_path();
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, _rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.set_trust(&peer, p2p_trust::TrustState::Verified);
        let hash = [0xab; 32];
        node.push_incoming_bytes(&peer, &encode_file_offer("test.txt", 1000, hash));
        node.poll();
        let _ = node.io_tx.send(IoEvent::Disconnected {
            peer_id_hex: peer.clone(),
        });
        node.poll();
        let xfer = node.snapshot().transfer.expect("xfer");
        assert_eq!(xfer.status, TransferStatus::Failed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn safe_filename_strips_path_separators() {
        assert_eq!(safe_filename("notes.txt"), "notes.txt");
        assert_eq!(safe_filename("../../etc/passwd"), "passwd");
        assert_eq!(safe_filename("C:\\Windows\\notes.txt"), "notes.txt");
        assert_eq!(safe_filename(".."), "download");
        assert_eq!(safe_filename(""), "download");
    }

    #[tokio::test]
    async fn zero_byte_file_completes_without_chunks() {
        let dir = temp_path();
        std::fs::create_dir_all(&dir).expect("dir");
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, mut rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.set_trust(&peer, p2p_trust::TrustState::Verified);
        let hash = sha256(b"");
        node.push_incoming_bytes(&peer, &encode_file_offer("empty.txt", 0, hash));
        node.poll();
        let frame = rx.try_recv().expect("accept");
        assert_eq!(decode_frame(&frame).expect("decode").0, Decoded::FileAccept);
        let xfer = node.snapshot().transfer.expect("xfer");
        assert_eq!(xfer.status, TransferStatus::Complete);
        assert!(dir.join("empty.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn tofu_peer_shows_pending_offer() {
        let dir = temp_path();
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, _rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.set_trust(&peer, p2p_trust::TrustState::Tofu);
        let hash = [0xab; 32];
        node.push_incoming_bytes(&peer, &encode_file_offer("test.txt", 100, hash));
        node.poll();
        let snap = node.snapshot();
        assert!(snap.transfer.is_none());
        let pending = snap.pending_offer.expect("pending");
        assert_eq!(pending.peer_id_hex, peer);
        assert_eq!(pending.name, "test.txt");
        assert_eq!(pending.size, 100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn accept_tofu_offer_starts_transfer() {
        let dir = temp_path();
        std::fs::create_dir_all(&dir).expect("dir");
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, mut rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.set_trust(&peer, p2p_trust::TrustState::Tofu);
        let data = b"content";
        let hash = sha256(data);
        node.push_incoming_bytes(
            &peer,
            &encode_file_offer("file.txt", data.len() as u64, hash),
        );
        node.poll();
        assert!(node.snapshot().pending_offer.is_some());
        node.accept_file(&peer);
        let frame = rx.try_recv().expect("accept frame");
        let (decoded, _) = decode_frame(&frame).expect("decode");
        assert_eq!(decoded, Decoded::FileAccept);
        node.push_incoming_bytes(&peer, &encode_file_chunk(0, data));
        node.poll();
        let snap = node.snapshot();
        assert!(snap.pending_offer.is_none());
        let xfer = snap.transfer.expect("xfer");
        assert_eq!(xfer.status, TransferStatus::Complete);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reject_tofu_offer_sends_reject() {
        let dir = temp_path();
        let mut node = Node::test_node(&dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, mut rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        node.set_trust(&peer, p2p_trust::TrustState::Tofu);
        let hash = [0xab; 32];
        node.push_incoming_bytes(&peer, &encode_file_offer("file.txt", 100, hash));
        node.poll();
        assert!(node.snapshot().pending_offer.is_some());
        node.reject_file(&peer);
        let frame = rx.try_recv().expect("reject frame");
        let (decoded, _) = decode_frame(&frame).expect("decode");
        assert_eq!(decoded, Decoded::FileReject);
        let snap = node.snapshot();
        assert!(snap.pending_offer.is_none());
        assert!(snap.transfer.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn connected_node(dir: &std::path::Path) -> (Node, String, mpsc::UnboundedReceiver<Vec<u8>>) {
        let mut node = Node::test_node(dir, "correct-horse").expect("node");
        let peer = valid_peer_hex();
        let (tx, rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(peer.clone(), tx);
        node.select(&peer);
        (node, peer, rx)
    }

    #[tokio::test]
    async fn invite_audio_sends_call_invite() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.invite_audio(&peer).expect("invite");
        let frame = rx.try_recv().expect("bytes");
        let (decoded, _) = decode_frame(&frame).expect("decode");
        assert_eq!(
            decoded,
            Decoded::CallInvite {
                media: MediaType::Audio
            }
        );
        let snap = node.snapshot();
        let call = snap.call.expect("outgoing");
        assert_eq!(call.peer_id_hex, peer);
        assert_eq!(call.phase, CallPhase::Outgoing);
        assert_eq!(call.media, MediaType::Audio);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn inbound_audio_invite_shows_pending() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.set_trust(&peer, TrustState::Tofu);
        node.push_incoming_bytes(&peer, &encode_call_invite(MediaType::Audio));
        let snap = node.snapshot();
        let pending = snap.pending_invite.expect("pending");
        assert_eq!(pending.peer_id_hex, peer);
        assert_eq!(pending.media, MediaType::Audio);
        assert_eq!(snap.call.expect("incoming").phase, CallPhase::Incoming);
        assert!(rx.try_recv().is_err(), "must not auto-accept or occupy mic");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn untrusted_inbound_invite_is_rejected() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.set_trust(&peer, TrustState::Unknown);
        node.push_incoming_bytes(&peer, &encode_call_invite(MediaType::Audio));
        let frame = rx.try_recv().expect("reject");
        assert_eq!(decode_frame(&frame).expect("decode").0, Decoded::CallReject);
        assert!(node.snapshot().pending_invite.is_none());
        assert!(node.snapshot().call.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remote_reject_returns_caller_to_chat() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.invite_audio(&peer).expect("invite");
        let _ = rx.try_recv().expect("invite frame");
        node.push_incoming_bytes(&peer, &encode_call_reject());
        let snap = node.snapshot();
        assert!(snap.call.is_none());
        assert_eq!(snap.call_result, Some(CallResult::Rejected));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn invite_timeout_returns_timed_out() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.invite_audio(&peer).expect("invite");
        let _ = rx.try_recv().expect("invite");
        node.set_invite_deadline_past();
        node.poll();
        let frame = rx.try_recv().expect("end");
        assert_eq!(decode_frame(&frame).expect("decode").0, Decoded::CallEnd);
        let snap = node.snapshot();
        assert!(snap.call.is_none());
        assert_eq!(snap.call_result, Some(CallResult::TimedOut));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn second_invite_fails_when_busy() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.invite_audio(&peer).expect("first");
        let _ = rx.try_recv();
        assert_eq!(node.invite_audio(&peer), Err(Error::Busy));
        let other = valid_peer_hex();
        let (tx, _rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(other.clone(), tx);
        assert_eq!(node.invite_audio(&other), Err(Error::Busy));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn inbound_invite_while_busy_is_rejected() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.set_trust(&peer, TrustState::Verified);
        node.invite_audio(&peer).expect("first");
        let _ = rx.try_recv();
        let other = valid_peer_hex();
        let (tx, mut other_rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(other.clone(), tx);
        node.set_trust(&other, TrustState::Verified);
        node.push_incoming_bytes(&other, &encode_call_invite(MediaType::Audio));
        let frame = other_rx.try_recv().expect("reject");
        assert_eq!(decode_frame(&frame).expect("decode").0, Decoded::CallReject);
        assert_eq!(node.snapshot().call.expect("still first").peer_id_hex, peer);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn accept_then_hangup_sends_end() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.set_trust(&peer, TrustState::Verified);
        node.push_incoming_bytes(&peer, &encode_call_invite(MediaType::Audio));
        node.accept_call(&peer);
        let frame = rx.try_recv().expect("accept");
        assert_eq!(decode_frame(&frame).expect("decode").0, Decoded::CallAccept);
        assert_eq!(
            node.snapshot().call.expect("active").phase,
            CallPhase::Active
        );
        node.hangup();
        let frame = rx.try_recv().expect("end");
        assert_eq!(decode_frame(&frame).expect("decode").0, Decoded::CallEnd);
        assert!(node.snapshot().call.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remote_end_clears_local_call() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.invite_audio(&peer).expect("invite");
        let _ = rx.try_recv();
        node.push_incoming_bytes(&peer, &encode_call_accept());
        assert_eq!(
            node.snapshot().call.expect("active").phase,
            CallPhase::Active
        );
        node.push_incoming_bytes(&peer, &encode_call_end());
        assert!(node.snapshot().call.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn text_still_works_during_call() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.invite_audio(&peer).expect("invite");
        let _ = rx.try_recv();
        node.push_incoming_bytes(&peer, &encode_call_accept());
        node.send_text(&peer, "still typing").expect("text");
        let frame = rx.try_recv().expect("text frame");
        match decode_frame(&frame).expect("decode").0 {
            Decoded::Text { content, .. } => assert_eq!(content, "still typing"),
            other => panic!("expected text, got {other:?}"),
        }
        node.push_incoming_bytes(&peer, &encode_text("got it", 1));
        let snap = node.snapshot();
        assert_eq!(snap.messages.last().expect("msg").content, "got it");
        assert_eq!(snap.call.expect("still active").phase, CallPhase::Active);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn invite_video_sends_call_invite() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.invite_video(&peer).expect("invite");
        let frame = rx.try_recv().expect("bytes");
        let (decoded, _) = decode_frame(&frame).expect("decode");
        assert_eq!(
            decoded,
            Decoded::CallInvite {
                media: MediaType::AudioVideo
            }
        );
        let snap = node.snapshot();
        let call = snap.call.expect("outgoing");
        assert_eq!(call.peer_id_hex, peer);
        assert_eq!(call.phase, CallPhase::Outgoing);
        assert_eq!(call.media, MediaType::AudioVideo);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn inbound_video_invite_shows_pending() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.set_trust(&peer, TrustState::Tofu);
        node.push_incoming_bytes(&peer, &encode_call_invite(MediaType::AudioVideo));
        let snap = node.snapshot();
        let pending = snap.pending_invite.expect("pending");
        assert_eq!(pending.peer_id_hex, peer);
        assert_eq!(pending.media, MediaType::AudioVideo);
        assert_eq!(snap.call.expect("incoming").phase, CallPhase::Incoming);
        assert!(
            rx.try_recv().is_err(),
            "must not auto-accept or open camera"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn accept_video_then_hangup_sends_end() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.set_trust(&peer, TrustState::Verified);
        node.push_incoming_bytes(&peer, &encode_call_invite(MediaType::AudioVideo));
        node.accept_call(&peer);
        let frame = rx.try_recv().expect("accept");
        assert_eq!(decode_frame(&frame).expect("decode").0, Decoded::CallAccept);
        let call = node.snapshot().call.expect("active");
        assert_eq!(call.phase, CallPhase::Active);
        assert_eq!(call.media, MediaType::AudioVideo);
        node.hangup();
        let frame = rx.try_recv().expect("end");
        assert_eq!(decode_frame(&frame).expect("decode").0, Decoded::CallEnd);
        assert!(node.snapshot().call.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn second_video_invite_fails_when_busy() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.invite_video(&peer).expect("first");
        let _ = rx.try_recv();
        assert_eq!(node.invite_audio(&peer), Err(Error::Busy));
        assert_eq!(node.invite_video(&peer), Err(Error::Busy));
        let other = valid_peer_hex();
        let (tx, _rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(other.clone(), tx);
        assert_eq!(node.invite_video(&other), Err(Error::Busy));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn inbound_video_invite_while_busy_is_rejected() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.set_trust(&peer, TrustState::Verified);
        node.invite_audio(&peer).expect("first");
        let _ = rx.try_recv();
        let other = valid_peer_hex();
        let (tx, mut other_rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(other.clone(), tx);
        node.set_trust(&other, TrustState::Verified);
        node.push_incoming_bytes(&other, &encode_call_invite(MediaType::AudioVideo));
        let frame = other_rx.try_recv().expect("reject");
        assert_eq!(decode_frame(&frame).expect("decode").0, Decoded::CallReject);
        assert_eq!(node.snapshot().call.expect("still first").peer_id_hex, peer);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reject_inbound_video_sends_reject() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.set_trust(&peer, TrustState::Tofu);
        node.push_incoming_bytes(&peer, &encode_call_invite(MediaType::AudioVideo));
        node.reject_call(&peer);
        let frame = rx.try_recv().expect("reject");
        assert_eq!(decode_frame(&frame).expect("decode").0, Decoded::CallReject);
        assert!(node.snapshot().call.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn video_datagrams_decode_to_frame() {
        use crate::video::{max_nal_len, VideoEncoder, DEFAULT_MAX_DATAGRAM, HEIGHT, WIDTH};
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        let (dtx, _drx) = mpsc::unbounded_channel();
        node.attach_dgram_sink(peer.clone(), dtx, DEFAULT_MAX_DATAGRAM);
        node.set_trust(&peer, TrustState::Verified);
        node.push_incoming_bytes(&peer, &encode_call_invite(MediaType::AudioVideo));
        node.accept_call(&peer);
        let _ = rx.try_recv();
        let mut encoder = VideoEncoder::new(max_nal_len(DEFAULT_MAX_DATAGRAM)).expect("enc");
        let mut rgb = vec![0u8; (WIDTH * HEIGHT * 3) as usize];
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let i = ((y * WIDTH + x) * 3) as usize;
                rgb[i] = 180;
                rgb[i + 1] = 20;
                rgb[i + 2] = 20;
            }
        }
        let dgrams = encoder.encode_rgb(&rgb, WIDTH, HEIGHT, 1).expect("encode");
        for d in &dgrams {
            node.push_incoming_dgram(&peer, d);
        }
        let frame = node.take_video_frame().expect("decoded frame");
        assert_eq!(frame.width, WIDTH);
        assert_eq!(frame.height, HEIGHT);
        node.hangup();
        assert!(node.take_video_frame().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reject_inbound_sends_reject() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.set_trust(&peer, TrustState::Tofu);
        node.push_incoming_bytes(&peer, &encode_call_invite(MediaType::Audio));
        node.reject_call(&peer);
        let frame = rx.try_recv().expect("reject");
        assert_eq!(decode_frame(&frame).expect("decode").0, Decoded::CallReject);
        assert!(node.snapshot().call.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn disconnect_marks_peer_failed_and_keeps_history() {
        let dir = temp_path();
        let (mut node, peer, _rx) = connected_node(&dir);
        node.send_text(&peer, "history").expect("send");
        let _ = node.io_tx.send(IoEvent::Disconnected {
            peer_id_hex: peer.clone(),
        });
        node.poll();
        let snap = node.snapshot();
        assert_eq!(snap.selected_status, Some(PeerStatus::Failed));
        let err = snap.selected_error.expect("error");
        assert_eq!(err.message, "Connection lost.");
        assert!(snap.sidebar.iter().any(|i| i.peer_id_hex == peer));
        assert!(snap.messages.iter().any(|m| m.content == "history"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn redial_after_disconnect_sends_new_connection() {
        let dir = temp_path();
        let (mut node, peer, _rx) = connected_node(&dir);
        let _ = node.io_tx.send(IoEvent::Disconnected {
            peer_id_hex: peer.clone(),
        });
        node.poll();
        node.dial(&peer).expect("redial");
        assert_eq!(node.next_dial_command(), Some(peer.clone()));
        assert_eq!(
            node.snapshot().selected_status,
            Some(PeerStatus::Connecting)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn redial_by_nickname_after_disconnect() {
        let dir = temp_path();
        let (mut node, peer, _rx) = connected_node(&dir);
        node.set_nickname(&peer, "alice").expect("nickname");
        let _ = node.io_tx.send(IoEvent::Disconnected {
            peer_id_hex: peer.clone(),
        });
        node.poll();
        node.dial("alice").expect("redial");
        assert_eq!(node.next_dial_command(), Some(peer));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn dial_connected_or_connecting_peer_sends_no_second_command() {
        let dir = temp_path();
        let (mut node, peer, _rx) = connected_node(&dir);
        node.dial(&peer).expect("connected: switch view only");
        assert!(node.next_dial_command().is_none());
        let other = valid_peer_hex();
        node.dial(&other).expect("fresh dial");
        assert_eq!(node.next_dial_command(), Some(other.clone()));
        node.dial(&other).expect("connecting: switch view only");
        assert!(node.next_dial_command().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn disconnect_clears_pending_tofu_offer() {
        let dir = temp_path();
        let (mut node, peer, _rx) = connected_node(&dir);
        node.set_trust(&peer, TrustState::Tofu);
        let hash = [0xab; 32];
        node.push_incoming_bytes(&peer, &encode_file_offer("test.txt", 100, hash));
        node.poll();
        assert!(node.snapshot().pending_offer.is_some());
        let _ = node.io_tx.send(IoEvent::Disconnected {
            peer_id_hex: peer.clone(),
        });
        node.poll();
        assert!(node.snapshot().pending_offer.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn disconnect_stops_active_call() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.invite_audio(&peer).expect("invite");
        let _ = rx.try_recv();
        node.push_incoming_bytes(&peer, &encode_call_accept());
        assert_eq!(
            node.snapshot().call.expect("active").phase,
            CallPhase::Active
        );
        let _ = node.io_tx.send(IoEvent::Disconnected {
            peer_id_hex: peer.clone(),
        });
        node.poll();
        assert!(node.snapshot().call.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn call_result_is_only_for_that_peer_and_clears_on_select() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.invite_audio(&peer).expect("invite");
        let _ = rx.try_recv();
        node.push_incoming_bytes(&peer, &encode_call_reject());
        assert_eq!(node.snapshot().call_result, Some(CallResult::Rejected));
        let other = valid_peer_hex();
        let (tx, _other_rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(other.clone(), tx);
        node.select(&other);
        assert!(node.snapshot().call_result.is_none());
        node.select(&peer);
        assert!(
            node.snapshot().call_result.is_none(),
            "coming back must not revive the old result"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn send_text_clears_call_result() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.invite_audio(&peer).expect("invite");
        let _ = rx.try_recv();
        node.push_incoming_bytes(&peer, &encode_call_reject());
        assert_eq!(node.snapshot().call_result, Some(CallResult::Rejected));
        node.send_text(&peer, "hi").expect("text");
        assert!(node.snapshot().call_result.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn hangup_sends_end_when_another_peer_is_selected() {
        let dir = temp_path();
        let (mut node, peer, mut rx) = connected_node(&dir);
        node.invite_audio(&peer).expect("invite");
        let _ = rx.try_recv();
        node.push_incoming_bytes(&peer, &encode_call_accept());
        let other = valid_peer_hex();
        let (tx, _other_rx) = mpsc::unbounded_channel();
        node.attach_byte_sink(other.clone(), tx);
        node.select(&other);
        assert_eq!(
            node.snapshot().call.expect("still active").peer_id_hex,
            peer
        );
        node.hangup();
        let frame = rx.try_recv().expect("end");
        assert_eq!(decode_frame(&frame).expect("decode").0, Decoded::CallEnd);
        assert!(node.snapshot().call.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
