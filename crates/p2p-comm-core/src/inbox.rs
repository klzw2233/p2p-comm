use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::chatlog::ChatKeys;
use crate::Error;

/// Who wrote the line, from this device's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Outgoing,
    Incoming,
}

/// One chat line shown in the pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub content: String,
    pub timestamp: u64,
    pub direction: Direction,
    #[serde(default)]
    pub failed: bool,
}

/// In-memory history + unread counts. Disk is [`ChatKeys`].
pub struct Inbox {
    by_peer: BTreeMap<String, Vec<ChatMessage>>,
    unread: BTreeMap<String, u32>,
}

impl Inbox {
    #[must_use]
    pub fn new() -> Self {
        Self {
            by_peer: BTreeMap::new(),
            unread: BTreeMap::new(),
        }
    }

    pub fn load_peer(&mut self, peer_id_hex: &str, messages: Vec<ChatMessage>) {
        self.by_peer.insert(peer_id_hex.to_owned(), messages);
    }

    /// Record an outgoing line. Persist if `keys` is `Some`.
    ///
    /// # Errors
    ///
    /// * [`Error::Io`] from the log
    pub fn sent(
        &mut self,
        peer_id_hex: &str,
        msg: ChatMessage,
        keys: Option<&ChatKeys>,
    ) -> Result<(), Error> {
        self.push(peer_id_hex, msg, keys)
    }

    /// Record an incoming line. Bumps unread if `selected` is a different Peer.
    ///
    /// # Errors
    ///
    /// * [`Error::Io`] from the log
    pub fn received(
        &mut self,
        peer_id_hex: &str,
        msg: ChatMessage,
        selected: Option<&str>,
        keys: Option<&ChatKeys>,
    ) -> Result<(), Error> {
        self.push(peer_id_hex, msg, keys)?;
        if selected != Some(peer_id_hex) {
            *self.unread.entry(peer_id_hex.to_owned()).or_insert(0) += 1;
        }
        Ok(())
    }

    fn push(
        &mut self,
        peer_id_hex: &str,
        msg: ChatMessage,
        keys: Option<&ChatKeys>,
    ) -> Result<(), Error> {
        let persist = keys.map(|k| k.append(peer_id_hex, &msg));
        self.by_peer
            .entry(peer_id_hex.to_owned())
            .or_default()
            .push(msg);
        persist.unwrap_or(Ok(()))
    }

    /// Mark the last outgoing line as failed (no extra disk write).
    pub fn mark_last_failed(&mut self, peer_id_hex: &str) {
        if let Some(list) = self.by_peer.get_mut(peer_id_hex) {
            if let Some(last) = list.last_mut() {
                last.failed = true;
            }
        }
    }

    pub fn clear_unread(&mut self, peer_id_hex: &str) {
        self.unread.remove(peer_id_hex);
    }

    #[must_use]
    pub fn messages(&self, peer_id_hex: &str) -> &[ChatMessage] {
        self.by_peer.get(peer_id_hex).map_or(&[], Vec::as_slice)
    }

    #[must_use]
    pub fn unread(&self, peer_id_hex: &str) -> u32 {
        self.unread.get(peer_id_hex).copied().unwrap_or(0)
    }
}

impl Default for Inbox {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outgoing(content: &str) -> ChatMessage {
        ChatMessage {
            content: content.to_owned(),
            timestamp: 1,
            direction: Direction::Outgoing,
            failed: false,
        }
    }

    fn incoming(content: &str) -> ChatMessage {
        ChatMessage {
            content: content.to_owned(),
            timestamp: 2,
            direction: Direction::Incoming,
            failed: false,
        }
    }

    #[test]
    fn send_fail_marks_last_outgoing() {
        let mut inbox = Inbox::new();
        inbox.sent("aaaa", outgoing("hi"), None).expect("sent");
        inbox.mark_last_failed("aaaa");
        assert!(inbox.messages("aaaa")[0].failed);
    }

    #[test]
    fn unread_when_not_selected_and_clears_on_select() {
        let mut inbox = Inbox::new();
        inbox
            .received("bbbb", incoming("yo"), Some("aaaa"), None)
            .expect("recv");
        assert_eq!(inbox.unread("bbbb"), 1);
        inbox.clear_unread("bbbb");
        assert_eq!(inbox.unread("bbbb"), 0);
    }

    #[test]
    fn selected_peer_does_not_bump_unread() {
        let mut inbox = Inbox::new();
        inbox
            .received("aaaa", incoming("yo"), Some("aaaa"), None)
            .expect("recv");
        assert_eq!(inbox.unread("aaaa"), 0);
        assert_eq!(inbox.messages("aaaa").len(), 1);
    }
}
