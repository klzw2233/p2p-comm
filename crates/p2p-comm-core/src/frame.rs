use serde::{Deserialize, Serialize};

use crate::Error;

/// Wire JSON. Unknown variants must not panic (`#[serde(other)]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WireMessage {
    Text { content: String, timestamp: u64 },
    #[serde(other)]
    Unknown,
}

/// Result of decoding one length-prefixed frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    Text { content: String, timestamp: u64 },
    Ignored,
}

/// Encode a Text frame: `[4-byte LE length][UTF-8 JSON]`.
#[must_use]
pub fn encode_text(content: &str, timestamp: u64) -> Vec<u8> {
    encode_json(&WireMessage::Text {
        content: content.to_owned(),
        timestamp,
    })
}

fn encode_json(msg: &WireMessage) -> Vec<u8> {
    let json = serde_json::to_vec(msg).unwrap_or_else(|_| br#"{"type":"Text","content":"","timestamp":0}"#.to_vec());
    let len = u32::try_from(json.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(4 + json.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&json);
    out
}

/// Decode one complete frame. Incomplete input is [`Error::InvalidFrame`].
///
/// # Errors
///
/// * [`Error::InvalidFrame`] if the length prefix is truncated, claims more
///   bytes than remain, or the JSON is not UTF-8 / not an object.
pub fn decode_frame(bytes: &[u8]) -> Result<(Decoded, usize), Error> {
    if bytes.len() < 4 {
        return Err(Error::InvalidFrame);
    }
    let prefix: [u8; 4] = bytes[..4].try_into().map_err(|_| Error::InvalidFrame)?;
    let len = usize::try_from(u32::from_le_bytes(prefix)).map_err(|_| Error::InvalidFrame)?;
    let total = 4 + len;
    if bytes.len() < total {
        return Err(Error::InvalidFrame);
    }
    let payload = &bytes[4..total];
    let decoded = match serde_json::from_slice::<WireMessage>(payload) {
        Ok(WireMessage::Text { content, timestamp }) => Decoded::Text { content, timestamp },
        Ok(WireMessage::Unknown) | Err(_) => Decoded::Ignored,
    };
    Ok((decoded, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_roundtrip() {
        let frame = encode_text("hello", 1_725_000_000_000);
        let payload_len = u32::try_from(frame.len() - 4).expect("tiny");
        assert_eq!(&frame[..4], &payload_len.to_le_bytes());
        let json = std::str::from_utf8(&frame[4..]).expect("utf8");
        assert_eq!(
            json,
            r#"{"type":"Text","content":"hello","timestamp":1725000000000}"#
        );
        let (decoded, n) = decode_frame(&frame).expect("decode");
        assert_eq!(n, frame.len());
        assert_eq!(
            decoded,
            Decoded::Text {
                content: "hello".into(),
                timestamp: 1_725_000_000_000
            }
        );
    }

    #[test]
    fn unknown_variant_is_ignored() {
        let json = br#"{"type":"FileOffer","name":"a.bin"}"#;
        let mut frame = Vec::new();
        let len = u32::try_from(json.len()).expect("tiny");
        frame.extend_from_slice(&len.to_le_bytes());
        frame.extend_from_slice(json);
        let (decoded, n) = decode_frame(&frame).expect("decode");
        assert_eq!(n, frame.len());
        assert_eq!(decoded, Decoded::Ignored);
    }

    #[test]
    fn truncated_frame_is_invalid() {
        assert_eq!(decode_frame(&[1, 0]).unwrap_err(), Error::InvalidFrame);
        let mut frame = encode_text("hi", 1);
        frame.pop();
        assert_eq!(decode_frame(&frame).unwrap_err(), Error::InvalidFrame);
    }
}
