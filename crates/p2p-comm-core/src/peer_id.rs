use p2p_trust::PeerId;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// 64-character hex-encoded Peer ID (strong type wrapper).
///
/// Prevents mixing Peer IDs with regular strings at compile time.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PeerIdHex(String);

impl PeerIdHex {
    /// Hex-encode a `PeerId`. Infallible: 32 bytes always yield 64 hex chars.
    pub(crate) fn from_peer_id(peer: &PeerId) -> Self {
        Self(crate::to_hex(peer.as_bytes()))
    }

    /// Create a validated `PeerIdHex` from a String.
    ///
    /// # Errors
    ///
    /// Returns an error if the input is not exactly 64 hexadecimal characters.
    pub fn new(s: String) -> Result<Self, String> {
        if s.len() != 64 {
            return Err(format!("invalid peer_id_hex: expected 64 chars, got {}", s.len()));
        }
        if !s.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("invalid peer_id_hex: must contain only hex characters".into());
        }
        Ok(Self(s))
    }

    /// Returns a reference to the inner hex string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the first 8 characters for display purposes.
    #[must_use]
    pub fn short(&self) -> &str {
        &self.0[..8]
    }

    /// Consumes self and returns the inner String.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl FromStr for PeerIdHex {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s.to_string())
    }
}

impl fmt::Display for PeerIdHex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_peer_id_hex() {
        let valid = "a".repeat(64);
        let peer_id = PeerIdHex::new(valid.clone()).expect("valid");
        assert_eq!(peer_id.as_str(), &valid);
    }

    #[test]
    fn short_returns_first_8_chars() {
        let peer_id = PeerIdHex::new("0123456789abcdef".repeat(4)).expect("valid");
        assert_eq!(peer_id.short(), "01234567");
    }

    #[test]
    fn reject_non_64_chars() {
        let too_short = "a".repeat(63);
        let err = PeerIdHex::new(too_short).expect_err("too short");
        assert!(err.contains("expected 64 chars"));

        let too_long = "a".repeat(65);
        let err = PeerIdHex::new(too_long).expect_err("too long");
        assert!(err.contains("expected 64 chars"));
    }

    #[test]
    fn reject_non_hex() {
        let invalid = "g".repeat(64);
        let err = PeerIdHex::new(invalid).expect_err("non-hex");
        assert!(err.contains("hex characters"));
    }

    #[test]
    fn from_str_works() {
        let peer_id: PeerIdHex = "0123456789abcdef".repeat(4).parse().expect("parse");
        assert_eq!(peer_id.as_str().len(), 64);
    }

    #[test]
    fn display_shows_full_hex() {
        let input = "f".repeat(64);
        let peer_id = PeerIdHex::new(input.clone()).expect("valid");
        assert_eq!(format!("{peer_id}"), input);
    }
}
