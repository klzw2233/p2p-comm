//! H.264 codec + video datagram framing. No camera I/O.

use openh264::decoder::Decoder;
use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod};
use openh264::formats::{RgbSliceU8, YUVBuffer, YUVSource};

pub const VIDEO_TYPE: u8 = 0x02;
pub const WIDTH: u32 = 640;
pub const HEIGHT: u32 = 480;
pub const FPS: u32 = 15;
pub const HEADER_LEN: usize = 9;
pub const DEFAULT_MAX_DATAGRAM: usize = 1200;

/// Latest decoded remote frame (packed RGB8).
#[derive(Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// Pack `[0x02][u64 BE timestamp][one H.264 NAL]`.
#[must_use]
pub fn pack_video(timestamp: u64, nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + nal.len());
    out.push(VIDEO_TYPE);
    out.extend_from_slice(&timestamp.to_be_bytes());
    out.extend_from_slice(nal);
    out
}

/// Unpack a video datagram. `None` if too short or not type `0x02`.
#[must_use]
pub fn unpack_video(bytes: &[u8]) -> Option<(u64, &[u8])> {
    if bytes.len() < HEADER_LEN || bytes[0] != VIDEO_TYPE {
        return None;
    }
    let ts: [u8; 8] = bytes[1..HEADER_LEN].try_into().ok()?;
    Some((u64::from_be_bytes(ts), &bytes[HEADER_LEN..]))
}

/// Payload budget for one NAL: `max_datagram_size - 9`.
#[must_use]
pub fn max_nal_len(max_datagram: usize) -> usize {
    max_datagram.saturating_sub(HEADER_LEN)
}

/// `OpenH264` encoder: 640×480, 15 fps, slices capped at `max_nal_len`.
pub struct VideoEncoder {
    inner: Encoder,
    max_nal_len: usize,
}

impl VideoEncoder {
    /// # Errors
    ///
    /// `OpenH264` encoder creation failed.
    pub fn new(max_nal_len: usize) -> Result<Self, openh264::Error> {
        let slice = u32::try_from(max_nal_len.max(1)).unwrap_or(u32::MAX);
        let config = EncoderConfig::new()
            .max_slice_len(slice)
            .max_frame_rate(FrameRate::from_hz(15.0))
            .bitrate(BitRate::from_bps(400_000))
            .intra_frame_period(IntraFramePeriod::from_num_frames(FPS));
        let inner = Encoder::with_api_config(openh264::OpenH264API::from_source(), config)?;
        Ok(Self { inner, max_nal_len })
    }

    /// Encode one RGB8 frame into datagrams. Oversized NALs are dropped.
    ///
    /// `rgb` is packed RGB8, even `width × height`. Odd sizes or a length mismatch yield no NALs.
    ///
    /// # Errors
    ///
    /// `OpenH264` encode failed, or `rgb` is the wrong length.
    pub fn encode_rgb(
        &mut self,
        rgb: &[u8],
        width: u32,
        height: u32,
        timestamp: u64,
    ) -> Result<Vec<Vec<u8>>, openh264::Error> {
        let (width, height, rgb) = even_rgb(rgb, width, height);
        if width == 0 || height == 0 {
            return Ok(Vec::new());
        }
        let src = RgbSliceU8::new(rgb, (width as usize, height as usize));
        let yuv = YUVBuffer::from_rgb_source(src);
        let stream = self.inner.encode(&yuv)?;
        let mut out = Vec::new();
        for layer_i in 0..stream.num_layers() {
            let Some(layer) = stream.layer(layer_i) else {
                continue;
            };
            for nal_i in 0..layer.nal_count() {
                let Some(nal) = layer.nal_unit(nal_i) else {
                    continue;
                };
                if nal.len() > self.max_nal_len {
                    continue;
                }
                out.push(pack_video(timestamp, nal));
            }
        }
        Ok(out)
    }
}

/// `OpenH264` decoder. Feed one datagram at a time; a complete picture yields RGB8.
pub struct VideoDecoder {
    inner: Decoder,
}

impl VideoDecoder {
    /// # Errors
    ///
    /// `OpenH264` decoder creation failed.
    pub fn new() -> Result<Self, openh264::Error> {
        Decoder::new().map(|inner| Self { inner })
    }

    /// Decode one video datagram. `None` if not video, not a complete picture, or decode failed.
    #[must_use]
    pub fn push_datagram(&mut self, bytes: &[u8]) -> Option<VideoFrame> {
        let (_, nal) = unpack_video(bytes)?;
        if nal.is_empty() {
            return None;
        }
        let yuv = self.inner.decode(nal).ok()??;
        let (w, h) = yuv.dimensions();
        let mut rgb = vec![0u8; yuv.rgb8_len()];
        yuv.write_rgb8(&mut rgb);
        Some(VideoFrame {
            width: u32::try_from(w).unwrap_or(0),
            height: u32::try_from(h).unwrap_or(0),
            rgb,
        })
    }
}

/// Nearest-neighbor scale.
// ponytail: nearest-neighbor, bilinear if capture size is far from 640×480.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn scale_rgb(rgb: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return Vec::new();
    }
    if sw == dw && sh == dh {
        return rgb.to_vec();
    }
    let mut out = vec![0u8; (dw * dh * 3) as usize];
    for y in 0..dh {
        let sy = y * sh / dh;
        for x in 0..dw {
            let sx = x * sw / dw;
            let si = ((sy * sw + sx) * 3) as usize;
            let di = ((y * dw + x) * 3) as usize;
            if si + 2 < rgb.len() {
                out[di..di + 3].copy_from_slice(&rgb[si..si + 3]);
            }
        }
    }
    out
}

fn even_rgb(rgb: &[u8], width: u32, height: u32) -> (u32, u32, &[u8]) {
    if width % 2 == 0 && height % 2 == 0 && rgb.len() == (width * height * 3) as usize {
        (width, height, rgb)
    } else {
        (0, 0, &[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_unpack_video_datagram() {
        let nal = [1, 2, 3, 4];
        let dgram = pack_video(0x0102_0304_0506_0708, &nal);
        assert_eq!(dgram[0], 0x02);
        assert_eq!(
            &dgram[1..9],
            &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]
        );
        assert_eq!(&dgram[9..], &nal);
        let (ts, rest) = unpack_video(&dgram).expect("unpack");
        assert_eq!(ts, 0x0102_0304_0506_0708);
        assert_eq!(rest, &nal);
    }

    #[test]
    fn unpack_rejects_truncated_and_wrong_type() {
        assert!(unpack_video(&[0x02, 0, 1, 2]).is_none());
        let mut dgram = pack_video(1, &[9]);
        dgram[0] = 0x01;
        assert!(unpack_video(&dgram).is_none());
        assert!(unpack_video(&[]).is_none());
    }

    #[test]
    fn max_nal_len_is_datagram_minus_header() {
        assert_eq!(max_nal_len(1200), 1191);
        assert_eq!(max_nal_len(8), 0);
    }

    #[test]
    fn scale_rgb_identity_and_half() {
        let src = vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120];
        assert_eq!(scale_rgb(&src, 2, 2, 2, 2), src);
        let half = scale_rgb(&src, 2, 2, 1, 1);
        assert_eq!(half, vec![10, 20, 30]);
        assert!(scale_rgb(&src, 0, 2, 2, 2).is_empty());
    }

    #[test]
    fn synthetic_rgb_h264_roundtrip() {
        let mut encoder = VideoEncoder::new(max_nal_len(DEFAULT_MAX_DATAGRAM)).expect("encoder");
        let mut decoder = VideoDecoder::new().expect("decoder");
        let mut rgb = vec![0u8; (WIDTH * HEIGHT * 3) as usize];
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let i = ((y * WIDTH + x) * 3) as usize;
                if x < WIDTH / 2 {
                    rgb[i] = 200;
                    rgb[i + 1] = 20;
                    rgb[i + 2] = 20;
                } else {
                    rgb[i] = 20;
                    rgb[i + 1] = 20;
                    rgb[i + 2] = 200;
                }
            }
        }
        let dgrams = encoder.encode_rgb(&rgb, WIDTH, HEIGHT, 1).expect("encode");
        assert!(!dgrams.is_empty(), "encoder produced no NALs");
        for d in &dgrams {
            assert_eq!(d[0], VIDEO_TYPE);
            assert!(
                d.len() <= DEFAULT_MAX_DATAGRAM,
                "datagram {} exceeds MTU",
                d.len()
            );
        }
        let mut picture = None;
        for d in &dgrams {
            if let Some(frame) = decoder.push_datagram(d) {
                picture = Some(frame);
            }
        }
        let frame = picture.expect("decoder produced a picture");
        assert_eq!(frame.width, WIDTH);
        assert_eq!(frame.height, HEIGHT);
        assert_eq!(frame.rgb.len(), (WIDTH * HEIGHT * 3) as usize);
        let out = frame.rgb;
        // Independent of encoder internals: left half stays red-ish, right blue-ish.
        let mut left_r = 0u32;
        let mut left_b = 0u32;
        let mut right_r = 0u32;
        let mut right_b = 0u32;
        let mut n = 0u32;
        for y in HEIGHT / 4..HEIGHT * 3 / 4 {
            for x in WIDTH / 8..WIDTH * 3 / 8 {
                let i = ((y * WIDTH + x) * 3) as usize;
                left_r += u32::from(out[i]);
                left_b += u32::from(out[i + 2]);
                n += 1;
            }
            for x in WIDTH * 5 / 8..WIDTH * 7 / 8 {
                let i = ((y * WIDTH + x) * 3) as usize;
                right_r += u32::from(out[i]);
                right_b += u32::from(out[i + 2]);
            }
        }
        assert!(n > 0);
        assert!(left_r > left_b, "left half should stay red-ish");
        assert!(right_b > right_r, "right half should stay blue-ish");
    }
}
