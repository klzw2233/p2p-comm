//! Opus codec + audio datagram framing. No device I/O.

pub const SAMPLE_RATE: u32 = 48_000;
pub const FRAME_SAMPLES: usize = 960; // 20 ms at 48 kHz mono
pub const AUDIO_TYPE: u8 = 0x01;

const MAX_PACKET: usize = 4000;

/// Pack `[0x01][u64 BE timestamp][Opus payload]`.
#[must_use]
pub fn pack_audio(timestamp: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 8 + payload.len());
    out.push(AUDIO_TYPE);
    out.extend_from_slice(&timestamp.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Unpack an audio datagram. `None` if too short or not type `0x01`.
#[must_use]
pub fn unpack_audio(bytes: &[u8]) -> Option<(u64, &[u8])> {
    if bytes.len() < 9 || bytes[0] != AUDIO_TYPE {
        return None;
    }
    let ts: [u8; 8] = bytes[1..9].try_into().ok()?;
    Some((u64::from_be_bytes(ts), &bytes[9..]))
}

/// Opus encoder: 48 kHz, mono, `Voip`, 20 ms frames.
pub struct AudioEncoder {
    inner: opus::Encoder,
}

impl AudioEncoder {
    /// # Errors
    ///
    /// Opus encoder creation failed.
    pub fn new() -> Result<Self, opus::Error> {
        opus::Encoder::new(
            SAMPLE_RATE,
            opus::Channels::Mono,
            opus::Application::Voip,
        )
        .map(|inner| Self { inner })
    }

    /// Encode one 20 ms mono frame (`FRAME_SAMPLES` i16 samples).
    ///
    /// # Errors
    ///
    /// Opus encode failed, or `pcm` is the wrong length.
    pub fn encode(&mut self, pcm: &[i16]) -> Result<Vec<u8>, opus::Error> {
        let mut buf = [0u8; MAX_PACKET];
        let n = self.inner.encode(pcm, &mut buf)?;
        Ok(buf[..n].to_vec())
    }
}

/// Opus decoder: 48 kHz, mono.
pub struct AudioDecoder {
    inner: opus::Decoder,
}

impl AudioDecoder {
    /// # Errors
    ///
    /// Opus decoder creation failed.
    pub fn new() -> Result<Self, opus::Error> {
        opus::Decoder::new(SAMPLE_RATE, opus::Channels::Mono).map(|inner| Self { inner })
    }

    /// Decode one Opus packet into `FRAME_SAMPLES` i16 samples.
    /// Empty `payload` is packet loss (PLC).
    ///
    /// # Errors
    ///
    /// Opus decode failed.
    pub fn decode(&mut self, payload: &[u8]) -> Result<Vec<i16>, opus::Error> {
        let mut pcm = vec![0i16; FRAME_SAMPLES];
        let n = self.inner.decode(payload, &mut pcm, false)?;
        pcm.truncate(n);
        Ok(pcm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_unpack_audio_datagram() {
        let payload = [1, 2, 3, 4];
        let dgram = pack_audio(0x0102_0304_0506_0708, &payload);
        assert_eq!(dgram[0], 0x01);
        assert_eq!(&dgram[1..9], &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]);
        assert_eq!(&dgram[9..], &payload);
        let (ts, rest) = unpack_audio(&dgram).expect("unpack");
        assert_eq!(ts, 0x0102_0304_0506_0708);
        assert_eq!(rest, &payload);
    }

    #[test]
    fn unpack_rejects_truncated_and_wrong_type() {
        assert!(unpack_audio(&[0x01, 0, 1, 2]).is_none());
        let mut dgram = pack_audio(1, &[9]);
        dgram[0] = 0x02;
        assert!(unpack_audio(&dgram).is_none());
        assert!(unpack_audio(&[]).is_none());
    }

    #[test]
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn pcm_opus_pcm_roundtrip_keeps_energy() {
        let mut encoder = AudioEncoder::new().expect("encoder");
        let mut decoder = AudioDecoder::new().expect("decoder");
        let mut pcm = vec![0i16; FRAME_SAMPLES];
        for (i, sample) in pcm.iter_mut().enumerate() {
            let phase = (i as u32).wrapping_mul(440) % SAMPLE_RATE;
            let t = f64::from(phase) / f64::from(SAMPLE_RATE);
            *sample = ((t * 440.0 * 2.0 * std::f64::consts::PI).sin() * 8000.0) as i16;
        }
        let packet = encoder.encode(&pcm).expect("encode");
        assert!(!packet.is_empty());
        let out = decoder.decode(&packet).expect("decode");
        assert_eq!(out.len(), FRAME_SAMPLES);
        let energy: i64 = out.iter().map(|s| i64::from(*s) * i64::from(*s)).sum();
        assert!(energy > 1_000_000, "decoded energy {energy} too low");
    }
}
