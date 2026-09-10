use serde::{Deserialize, Serialize};

use crate::Error;

/// Call media requested in [`WireMessage::CallInvite`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaType {
    Audio,
    AudioVideo,
}

/// Wire JSON. Unknown variants must not panic (`#[serde(other)]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WireMessage {
    Text {
        content: String,
        timestamp: u64,
    },
    FileOffer {
        name: String,
        size: u64,
        hash: String,
    },
    FileAccept,
    FileReject,
    FileChunk {
        offset: u64,
        data: String,
    },
    CallInvite {
        media: MediaType,
    },
    CallAccept,
    CallReject,
    CallEnd,
    #[serde(other)]
    Unknown,
}

/// Result of decoding one length-prefixed frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    Text {
        content: String,
        timestamp: u64,
    },
    FileOffer {
        name: String,
        size: u64,
        hash: [u8; 32],
    },
    FileAccept,
    FileReject,
    FileChunk {
        offset: u64,
        data: Vec<u8>,
    },
    CallInvite {
        media: MediaType,
    },
    CallAccept,
    CallReject,
    CallEnd,
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

/// Encode a `FileOffer`. `hash` is SHA-256, sent as 64 lowercase hex chars.
#[must_use]
pub fn encode_file_offer(name: &str, size: u64, hash: [u8; 32]) -> Vec<u8> {
    encode_json(&WireMessage::FileOffer {
        name: name.to_owned(),
        size,
        hash: crate::to_hex(&hash),
    })
}

/// Encode a `FileAccept` (no fields).
#[must_use]
pub fn encode_file_accept() -> Vec<u8> {
    encode_json(&WireMessage::FileAccept)
}

/// Encode a `FileReject` (no fields).
#[must_use]
pub fn encode_file_reject() -> Vec<u8> {
    encode_json(&WireMessage::FileReject)
}

/// Encode a `FileChunk`. `data` is standard base64 (padded), never a JSON number array.
#[must_use]
pub fn encode_file_chunk(offset: u64, data: &[u8]) -> Vec<u8> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    encode_json(&WireMessage::FileChunk {
        offset,
        data: STANDARD.encode(data),
    })
}

/// Encode a `CallInvite`.
#[must_use]
pub fn encode_call_invite(media: MediaType) -> Vec<u8> {
    encode_json(&WireMessage::CallInvite { media })
}

/// Encode a `CallAccept` (no fields).
#[must_use]
pub fn encode_call_accept() -> Vec<u8> {
    encode_json(&WireMessage::CallAccept)
}

/// Encode a `CallReject` (no fields).
#[must_use]
pub fn encode_call_reject() -> Vec<u8> {
    encode_json(&WireMessage::CallReject)
}

/// Encode a `CallEnd` (no fields).
#[must_use]
pub fn encode_call_end() -> Vec<u8> {
    encode_json(&WireMessage::CallEnd)
}

fn encode_json(msg: &WireMessage) -> Vec<u8> {
    let json = serde_json::to_vec(msg)
        .unwrap_or_else(|_| br#"{"type":"Text","content":"","timestamp":0}"#.to_vec());
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
        Ok(WireMessage::FileOffer { name, size, hash }) => match parse_hash_hex(&hash) {
            Some(hash) => Decoded::FileOffer { name, size, hash },
            None => Decoded::Ignored,
        },
        Ok(WireMessage::FileAccept) => Decoded::FileAccept,
        Ok(WireMessage::FileReject) => Decoded::FileReject,
        Ok(WireMessage::FileChunk { offset, data }) => match decode_b64(&data) {
            Some(data) => Decoded::FileChunk { offset, data },
            None => Decoded::Ignored,
        },
        Ok(WireMessage::CallInvite { media }) => Decoded::CallInvite { media },
        Ok(WireMessage::CallAccept) => Decoded::CallAccept,
        Ok(WireMessage::CallReject) => Decoded::CallReject,
        Ok(WireMessage::CallEnd) => Decoded::CallEnd,
        Ok(WireMessage::Unknown) | Err(_) => Decoded::Ignored,
    };
    Ok((decoded, total))
}

fn decode_b64(s: &str) -> Option<Vec<u8>> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    STANDARD.decode(s).ok()
}

fn parse_hash_hex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks_exact(2).enumerate() {
        let hex = std::str::from_utf8(chunk).ok()?;
        out[i] = u8::from_str_radix(hex, 16).ok()?;
    }
    Some(out)
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
        let json = br#"{"type":"NotARealType"}"#;
        let mut frame = Vec::new();
        let len = u32::try_from(json.len()).expect("tiny");
        frame.extend_from_slice(&len.to_le_bytes());
        frame.extend_from_slice(json);
        let (decoded, n) = decode_frame(&frame).expect("decode");
        assert_eq!(n, frame.len());
        assert_eq!(decoded, Decoded::Ignored);
    }

    #[test]
    fn unknown_variant_with_fields_is_ignored() {
        // Future version might add {"type": "FutureFeature", "data": 42}.
        // Old clients must ignore it gracefully rather than panic.
        let json = br#"{"type":"FutureFeature","magic":42,"nested":{"x":1}}"#;
        let mut frame = Vec::new();
        let len = u32::try_from(json.len()).expect("tiny");
        frame.extend_from_slice(&len.to_le_bytes());
        frame.extend_from_slice(json);
        let (decoded, n) = decode_frame(&frame).expect("decode");
        assert_eq!(n, frame.len());
        assert_eq!(decoded, Decoded::Ignored);
    }

    #[test]
    fn call_invite_audio_roundtrip() {
        let frame = encode_call_invite(MediaType::Audio);
        let json = std::str::from_utf8(&frame[4..]).expect("utf8");
        assert_eq!(json, r#"{"type":"CallInvite","media":"Audio"}"#);
        let (decoded, n) = decode_frame(&frame).expect("decode");
        assert_eq!(n, frame.len());
        assert_eq!(
            decoded,
            Decoded::CallInvite {
                media: MediaType::Audio
            }
        );
    }

    #[test]
    fn call_accept_reject_end_roundtrip() {
        let accept = encode_call_accept();
        assert_eq!(
            std::str::from_utf8(&accept[4..]).expect("utf8"),
            r#"{"type":"CallAccept"}"#
        );
        assert_eq!(
            decode_frame(&accept).expect("accept").0,
            Decoded::CallAccept
        );

        let reject = encode_call_reject();
        assert_eq!(
            std::str::from_utf8(&reject[4..]).expect("utf8"),
            r#"{"type":"CallReject"}"#
        );
        assert_eq!(
            decode_frame(&reject).expect("reject").0,
            Decoded::CallReject
        );

        let end = encode_call_end();
        assert_eq!(
            std::str::from_utf8(&end[4..]).expect("utf8"),
            r#"{"type":"CallEnd"}"#
        );
        assert_eq!(decode_frame(&end).expect("end").0, Decoded::CallEnd);
    }

    #[test]
    fn call_invite_audiovideo_roundtrip() {
        let frame = encode_call_invite(MediaType::AudioVideo);
        let json = std::str::from_utf8(&frame[4..]).expect("utf8");
        assert_eq!(json, r#"{"type":"CallInvite","media":"AudioVideo"}"#);
        let (decoded, n) = decode_frame(&frame).expect("decode");
        assert_eq!(n, frame.len());
        assert_eq!(
            decoded,
            Decoded::CallInvite {
                media: MediaType::AudioVideo
            }
        );
    }

    #[test]
    fn truncated_frame_is_invalid() {
        assert_eq!(decode_frame(&[1, 0]).unwrap_err(), Error::InvalidFrame);
        let mut frame = encode_text("hi", 1);
        frame.pop();
        assert_eq!(decode_frame(&frame).unwrap_err(), Error::InvalidFrame);
    }

    #[test]
    fn file_offer_roundtrip() {
        let hash = [0xab; 32];
        let frame = encode_file_offer("notes.txt", 12, hash);
        let json = std::str::from_utf8(&frame[4..]).expect("utf8");
        assert_eq!(
            json,
            r#"{"type":"FileOffer","name":"notes.txt","size":12,"hash":"abababababababababababababababababababababababababababababababab"}"#
        );
        let (decoded, n) = decode_frame(&frame).expect("decode");
        assert_eq!(n, frame.len());
        assert_eq!(
            decoded,
            Decoded::FileOffer {
                name: "notes.txt".into(),
                size: 12,
                hash,
            }
        );
    }

    #[test]
    fn file_accept_and_reject_roundtrip() {
        let accept = encode_file_accept();
        assert_eq!(
            std::str::from_utf8(&accept[4..]).expect("utf8"),
            r#"{"type":"FileAccept"}"#
        );
        let (decoded, n) = decode_frame(&accept).expect("decode accept");
        assert_eq!(n, accept.len());
        assert_eq!(decoded, Decoded::FileAccept);

        let reject = encode_file_reject();
        assert_eq!(
            std::str::from_utf8(&reject[4..]).expect("utf8"),
            r#"{"type":"FileReject"}"#
        );
        let (decoded, n) = decode_frame(&reject).expect("decode reject");
        assert_eq!(n, reject.len());
        assert_eq!(decoded, Decoded::FileReject);
    }

    #[test]
    fn file_chunk_data_is_base64() {
        // Independent of the encoder: python `base64.b64encode(bytes([0,1,2,254,255]))`.
        let data = vec![0, 1, 2, 254, 255];
        let frame = encode_file_chunk(65_536, &data);
        let json = std::str::from_utf8(&frame[4..]).expect("utf8");
        assert_eq!(
            json,
            r#"{"type":"FileChunk","offset":65536,"data":"AAEC/v8="}"#
        );
        assert!(!json.contains("[0,1,2"));
        let (decoded, n) = decode_frame(&frame).expect("decode");
        assert_eq!(n, frame.len());
        assert_eq!(
            decoded,
            Decoded::FileChunk {
                offset: 65_536,
                data,
            }
        );
    }
}
