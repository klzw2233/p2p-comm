use std::collections::BTreeMap;

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
    peers: BTreeMap<String, PeerStatus>,
    selected: Option<String>,
    errors: BTreeMap<String, ChatError>,
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
    pub fn begin_connect(&mut self, peer_id_hex: String) {
        self.errors.remove(&peer_id_hex);
        self.peers
            .insert(peer_id_hex.clone(), PeerStatus::Connecting);
        self.selected = Some(peer_id_hex);
    }

    /// Mark a Peer connected. Inbound unknown Peers land here.
    pub fn connected(&mut self, peer_id_hex: String) {
        self.errors.remove(&peer_id_hex);
        self.peers
            .insert(peer_id_hex.clone(), PeerStatus::Connected);
        if self.selected.is_none() {
            self.selected = Some(peer_id_hex);
        }
    }

    /// Record a connect failure. Does not steal the selected chat view.
    pub fn connect_failed(&mut self, peer_id_hex: String, message: String) {
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
    pub fn disconnected(&mut self, peer_id_hex: String, message: String) {
        if self.peers.get(&peer_id_hex) != Some(&PeerStatus::Connected) {
            return;
        }
        self.peers.insert(peer_id_hex.clone(), PeerStatus::Failed);
        self.errors.insert(peer_id_hex, ChatError { message });
    }

    /// Switch the chat view. Does not drop any Session.
    ///
    /// The Peer does not need a live Session (nickname-only contacts).
    pub fn select(&mut self, peer_id_hex: &str) {
        self.selected = Some(peer_id_hex.to_owned());
    }

    #[must_use]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    #[must_use]
    pub fn status(&self, peer_id_hex: &str) -> Option<PeerStatus> {
        self.peers.get(peer_id_hex).copied()
    }

    #[must_use]
    pub fn error(&self, peer_id_hex: &str) -> Option<&ChatError> {
        self.errors.get(peer_id_hex)
    }

    pub fn peer_ids(&self) -> impl Iterator<Item = &str> {
        self.peers.keys().map(String::as_str)
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

    #[test]
    fn one_entry_per_peer_and_select_does_not_drop() {
        let mut roster = Roster::new();
        roster.begin_connect("aaaa".into());
        roster.connected("aaaa".into());
        roster.begin_connect("bbbb".into());
        roster.connected("bbbb".into());
        assert_eq!(roster.status("aaaa"), Some(PeerStatus::Connected));
        assert_eq!(roster.status("bbbb"), Some(PeerStatus::Connected));
        assert_eq!(roster.selected(), Some("bbbb"));
        roster.select("aaaa");
        assert_eq!(roster.selected(), Some("aaaa"));
        assert_eq!(roster.status("bbbb"), Some(PeerStatus::Connected));
        roster.select("nick-only");
        assert_eq!(roster.selected(), Some("nick-only"));
        assert_eq!(roster.status("aaaa"), Some(PeerStatus::Connected));
    }

    #[test]
    fn inbound_peer_appears_without_replacing_selection() {
        let mut roster = Roster::new();
        roster.begin_connect("aaaa".into());
        roster.connected("aaaa".into());
        roster.connected("cccc".into());
        assert_eq!(roster.selected(), Some("aaaa"));
        assert_eq!(roster.status("cccc"), Some(PeerStatus::Connected));
    }

    #[test]
    fn connect_failure_keeps_peer_selected_with_error() {
        let mut roster = Roster::new();
        roster.begin_connect("aaaa".into());
        roster.connect_failed("aaaa".into(), "peer offline".into());
        assert_eq!(roster.selected(), Some("aaaa"));
        assert_eq!(roster.status("aaaa"), Some(PeerStatus::Failed));
        assert_eq!(
            roster.error("aaaa").map(|e| e.message.as_str()),
            Some("peer offline")
        );
        roster.begin_connect("aaaa".into());
        assert_eq!(roster.status("aaaa"), Some(PeerStatus::Connecting));
        assert!(roster.error("aaaa").is_none());
    }

    #[test]
    fn connect_failure_does_not_steal_selection() {
        let mut roster = Roster::new();
        roster.begin_connect("aaaa".into());
        roster.connected("bbbb".into());
        roster.select("bbbb");
        roster.connect_failed("aaaa".into(), "peer offline".into());
        assert_eq!(roster.selected(), Some("bbbb"));
        assert_eq!(roster.status("aaaa"), Some(PeerStatus::Failed));
        assert_eq!(roster.status("bbbb"), Some(PeerStatus::Connected));
    }

    #[test]
    fn connect_failure_ignored_if_already_connected() {
        let mut roster = Roster::new();
        roster.connected("aaaa".into());
        roster.connect_failed("aaaa".into(), "already connected".into());
        assert_eq!(roster.status("aaaa"), Some(PeerStatus::Connected));
        assert!(roster.error("aaaa").is_none());
    }

    #[test]
    fn disconnect_demotes_connected_peer_with_error() {
        let mut roster = Roster::new();
        roster.begin_connect("aaaa".into());
        roster.connected("aaaa".into());
        roster.begin_connect("bbbb".into());
        roster.connected("bbbb".into());
        roster.select("bbbb");
        roster.disconnected("aaaa".into(), "Connection lost.".into());
        assert_eq!(roster.status("aaaa"), Some(PeerStatus::Failed));
        assert_eq!(
            roster.error("aaaa").map(|e| e.message.as_str()),
            Some("Connection lost.")
        );
        assert_eq!(roster.selected(), Some("bbbb"));
        assert!(roster.peer_ids().any(|p| p == "aaaa"));
    }

    #[test]
    fn disconnect_ignored_unless_connected() {
        let mut roster = Roster::new();
        roster.begin_connect("aaaa".into());
        roster.disconnected("aaaa".into(), "Connection lost.".into());
        assert_eq!(roster.status("aaaa"), Some(PeerStatus::Connecting));
        assert!(roster.error("aaaa").is_none());
        roster.connect_failed("aaaa".into(), "peer offline".into());
        roster.disconnected("aaaa".into(), "Connection lost.".into());
        assert_eq!(roster.status("aaaa"), Some(PeerStatus::Failed));
        assert_eq!(
            roster.error("aaaa").map(|e| e.message.as_str()),
            Some("peer offline")
        );
        let mut fresh = Roster::new();
        fresh.disconnected("cccc".into(), "Connection lost.".into());
        assert_eq!(fresh.status("cccc"), None);
    }
}
