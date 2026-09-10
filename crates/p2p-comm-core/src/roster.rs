use std::collections::BTreeMap;

use crate::PeerIdHex;

/// Connection state of one remote Peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerStatus {
    Connecting,
    Connected,
    Failed,
}

/// Error shown in the current chat view after a connect attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatError {
    pub message: String,
}

/// Process-local map of remote Peers that have a live or attempted Session.
///
/// At most one entry per Peer ID. Switching the selected Peer does not drop others.
pub struct Roster {
    peers: BTreeMap<PeerIdHex, PeerStatus>,
    selected: Option<PeerIdHex>,
    errors: BTreeMap<PeerIdHex, ChatError>,
}

impl Roster {
    #[must_use]
    pub fn new() -> Self {
        Self {
            peers: BTreeMap::new(),
            selected: None,
            errors: BTreeMap::new(),
        }
    }

    /// Start (or restart) a connect attempt. Selects that Peer.
    pub fn begin_connect(&mut self, peer_id_hex: PeerIdHex) {
        self.errors.remove(&peer_id_hex);
        self.peers
            .insert(peer_id_hex.clone(), PeerStatus::Connecting);
        self.selected = Some(peer_id_hex);
    }

    /// Mark a Peer connected. Inbound unknown Peers land here.
    pub fn connected(&mut self, peer_id_hex: PeerIdHex) {
        self.errors.remove(&peer_id_hex);
        self.peers
            .insert(peer_id_hex.clone(), PeerStatus::Connected);
        if self.selected.is_none() {
            self.selected = Some(peer_id_hex);
        }
    }

    /// Record a connect failure. Does not steal the selected chat view.
    pub fn connect_failed(&mut self, peer_id_hex: PeerIdHex, message: String) {
        if self.peers.get(&peer_id_hex) == Some(&PeerStatus::Connected) {
            return;
        }
        self.peers.insert(peer_id_hex.clone(), PeerStatus::Failed);
        self.errors
            .insert(peer_id_hex.clone(), ChatError { message });
        if self.selected.is_none() {
            self.selected = Some(peer_id_hex);
        }
    }

    /// Record that a connected Peer's Session is gone. Does not steal the
    /// selected chat view.
    ///
    /// No-op unless the Peer is currently Connected, so a duplicate or stale
    /// disconnect cannot clobber a Peer that is mid-dial or already failed.
    pub fn disconnected(&mut self, peer_id_hex: PeerIdHex, message: String) {
        if self.peers.get(&peer_id_hex) != Some(&PeerStatus::Connected) {
            return;
        }
        self.peers.insert(peer_id_hex.clone(), PeerStatus::Failed);
        self.errors.insert(peer_id_hex, ChatError { message });
    }

    /// Switch the chat view. Does not drop any Session.
    ///
    /// The Peer does not need a live Session (nickname-only contacts).
    pub fn select(&mut self, peer_id_hex: &PeerIdHex) {
        self.selected = Some(peer_id_hex.clone());
    }

    #[must_use]
    pub fn selected(&self) -> Option<&PeerIdHex> {
        self.selected.as_ref()
    }

    #[must_use]
    pub fn status(&self, peer_id_hex: &PeerIdHex) -> Option<PeerStatus> {
        self.peers.get(peer_id_hex).copied()
    }

    #[must_use]
    pub fn error(&self, peer_id_hex: &PeerIdHex) -> Option<&ChatError> {
        self.errors.get(peer_id_hex)
    }

    pub fn peer_ids(&self) -> impl Iterator<Item = &PeerIdHex> {
        self.peers.keys()
    }
}

impl Default for Roster {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_peer_id(s: &str) -> PeerIdHex {
        let hex = s.repeat(64 / s.len());
        hex.parse().expect("valid")
    }

    #[test]
    fn one_entry_per_peer_and_select_does_not_drop() {
        let mut roster = Roster::new();
        let a = test_peer_id("a");
        let b = test_peer_id("b");
        let nick = test_peer_id("c");
        roster.begin_connect(a.clone());
        roster.connected(a.clone());
        roster.begin_connect(b.clone());
        roster.connected(b.clone());
        assert_eq!(roster.status(&a), Some(PeerStatus::Connected));
        assert_eq!(roster.status(&b), Some(PeerStatus::Connected));
        assert_eq!(roster.selected(), Some(&b));
        roster.select(&a);
        assert_eq!(roster.selected(), Some(&a));
        assert_eq!(roster.status(&b), Some(PeerStatus::Connected));
        roster.select(&nick);
        assert_eq!(roster.selected(), Some(&nick));
        assert_eq!(roster.status(&a), Some(PeerStatus::Connected));
    }

    #[test]
    fn inbound_peer_appears_without_replacing_selection() {
        let mut roster = Roster::new();
        let a = test_peer_id("a");
        let c = test_peer_id("c");
        roster.begin_connect(a.clone());
        roster.connected(a.clone());
        roster.connected(c.clone());
        assert_eq!(roster.selected(), Some(&a));
        assert_eq!(roster.status(&c), Some(PeerStatus::Connected));
    }

    #[test]
    fn connect_failure_keeps_peer_selected_with_error() {
        let mut roster = Roster::new();
        let a = test_peer_id("a");
        roster.begin_connect(a.clone());
        roster.connect_failed(a.clone(), "peer offline".into());
        assert_eq!(roster.selected(), Some(&a));
        assert_eq!(roster.status(&a), Some(PeerStatus::Failed));
        assert_eq!(
            roster.error(&a).map(|e| e.message.as_str()),
            Some("peer offline")
        );
        roster.begin_connect(a.clone());
        assert_eq!(roster.status(&a), Some(PeerStatus::Connecting));
        assert!(roster.error(&a).is_none());
    }

    #[test]
    fn connect_failure_does_not_steal_selection() {
        let mut roster = Roster::new();
        let a = test_peer_id("a");
        let b = test_peer_id("b");
        roster.begin_connect(a.clone());
        roster.connected(b.clone());
        roster.select(&b);
        roster.connect_failed(a.clone(), "peer offline".into());
        assert_eq!(roster.selected(), Some(&b));
        assert_eq!(roster.status(&a), Some(PeerStatus::Failed));
        assert_eq!(roster.status(&b), Some(PeerStatus::Connected));
    }

    #[test]
    fn connect_failure_ignored_if_already_connected() {
        let mut roster = Roster::new();
        let a = test_peer_id("a");
        roster.connected(a.clone());
        roster.connect_failed(a.clone(), "already connected".into());
        assert_eq!(roster.status(&a), Some(PeerStatus::Connected));
        assert!(roster.error(&a).is_none());
    }

    #[test]
    fn disconnect_demotes_connected_peer_with_error() {
        let mut roster = Roster::new();
        let a = test_peer_id("a");
        let b = test_peer_id("b");
        roster.begin_connect(a.clone());
        roster.connected(a.clone());
        roster.begin_connect(b.clone());
        roster.connected(b.clone());
        roster.select(&b);
        roster.disconnected(a.clone(), "Connection lost.".into());
        assert_eq!(roster.status(&a), Some(PeerStatus::Failed));
        assert_eq!(
            roster.error(&a).map(|e| e.message.as_str()),
            Some("Connection lost.")
        );
        assert_eq!(roster.selected(), Some(&b));
        assert!(roster.peer_ids().any(|p| p == &a));
    }

    #[test]
    fn disconnect_ignored_unless_connected() {
        let mut roster = Roster::new();
        let a = test_peer_id("a");
        let c = test_peer_id("c");
        roster.begin_connect(a.clone());
        roster.disconnected(a.clone(), "Connection lost.".into());
        assert_eq!(roster.status(&a), Some(PeerStatus::Connecting));
        assert!(roster.error(&a).is_none());
        roster.connect_failed(a.clone(), "peer offline".into());
        roster.disconnected(a.clone(), "Connection lost.".into());
        assert_eq!(roster.status(&a), Some(PeerStatus::Failed));
        assert_eq!(
            roster.error(&a).map(|e| e.message.as_str()),
            Some("peer offline")
        );
        let mut fresh = Roster::new();
        fresh.disconnected(c.clone(), "Connection lost.".into());
        assert_eq!(fresh.status(&c), None);
    }
}
