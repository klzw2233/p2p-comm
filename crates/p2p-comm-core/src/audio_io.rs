//! Default-device capture/playback. Callbacks only copy PCM.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use tokio::sync::mpsc;

use crate::audio::{
    pack_audio, unpack_audio, AudioDecoder, AudioEncoder, FRAME_SAMPLES, SAMPLE_RATE,
};
use crate::frame::MediaType;
use crate::video::{VideoDecoder, VideoFrame};
use crate::video_io;

const MAX_QUEUE: usize = FRAME_SAMPLES * 10; // ~200 ms

/// Live default-device media for one call. Dropping stops capture/playback and the camera.
pub struct LiveMedia {
    stop: Arc<AtomicBool>,
    playback: Arc<Mutex<VecDeque<i16>>>,
    decoder: AudioDecoder,
    video_decoder: Option<VideoDecoder>,
    video_frame: Arc<Mutex<Option<VideoFrame>>>,
    input: Option<Stream>,
    output: Option<Stream>,
    encode: Option<std::thread::JoinHandle<()>>,
    video: Option<std::thread::JoinHandle<()>>,
}

impl LiveMedia {
    /// Open default mic + speaker. Camera opens only for [`MediaType::AudioVideo`].
    /// Audio I/O is best-effort; video decode still starts if the mic is missing.
    /// `None` only if Opus init failed, or an audio-only call has no devices.
    #[must_use]
    pub fn start(
        dgram_tx: mpsc::UnboundedSender<Vec<u8>>,
        media: MediaType,
        max_nal_len: usize,
    ) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let capture = Arc::new(Mutex::new(VecDeque::<i16>::new()));
        let playback = Arc::new(Mutex::new(VecDeque::<i16>::new()));
        let decoder = AudioDecoder::new().ok()?;

        let host = cpal::default_host();
        let input = open_input(&host, Arc::clone(&capture));
        let output = open_output(&host, Arc::clone(&playback));

        let encode = if input.is_some() {
            let stop_enc = Arc::clone(&stop);
            let cap_enc = Arc::clone(&capture);
            let audio_tx = dgram_tx.clone();
            std::thread::Builder::new()
                .name("p2p-audio-enc".into())
                .spawn(move || encode_loop(&stop_enc, &cap_enc, &audio_tx))
                .ok()
        } else {
            None
        };

        let (video_decoder, video_frame, video) = if media == MediaType::AudioVideo {
            let video_decoder = VideoDecoder::new().ok();
            let video_frame = Arc::new(Mutex::new(None));
            let video = video_decoder
                .as_ref()
                .and_then(|_| video_io::start_capture(Arc::clone(&stop), dgram_tx, max_nal_len));
            (video_decoder, video_frame, video)
        } else {
            (None, Arc::new(Mutex::new(None)), None)
        };

        if media == MediaType::Audio && input.is_none() && output.is_none() {
            return None;
        }

        Some(Self {
            stop,
            playback,
            decoder,
            video_decoder,
            video_frame,
            input,
            output,
            encode,
            video,
        })
    }

    /// Decode one incoming datagram onto the playback queue or latest video frame.
    pub fn push_datagram(&mut self, bytes: &[u8]) {
        if bytes.first() == Some(&crate::video::VIDEO_TYPE) {
            let Some(dec) = self.video_decoder.as_mut() else {
                return;
            };
            if let Some(frame) = dec.push_datagram(bytes) {
                if let Ok(mut slot) = self.video_frame.lock() {
                    *slot = Some(frame);
                }
            }
            return;
        }
        let Some((_, payload)) = unpack_audio(bytes) else {
            return;
        };
        let Ok(pcm) = self.decoder.decode(payload) else {
            return;
        };
        if let Ok(mut buf) = self.playback.lock() {
            buf.extend(pcm);
            while buf.len() > MAX_QUEUE {
                buf.pop_front();
            }
        }
    }

    /// Take the latest decoded remote video frame, if any.
    #[must_use]
    pub fn take_video_frame(&self) -> Option<VideoFrame> {
        self.video_frame.lock().ok()?.take()
    }
}

impl Drop for LiveMedia {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.input.take();
        self.output.take();
        if let Some(h) = self.encode.take() {
            let _ = h.join();
        }
        if let Some(h) = self.video.take() {
            // ponytail: detached join so hangup isn't blocked on camera.frame(); join-with-timeout if LED must go off first.
            let _ = h;
        }
    }
}

fn encode_loop(
    stop: &AtomicBool,
    capture: &Mutex<VecDeque<i16>>,
    dgram_tx: &mpsc::UnboundedSender<Vec<u8>>,
) {
    let Ok(mut encoder) = AudioEncoder::new() else {
        return;
    };
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(20));
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let mut frame = vec![0i16; FRAME_SAMPLES];
        if let Ok(mut buf) = capture.lock() {
            for sample in &mut frame {
                *sample = buf.pop_front().unwrap_or(0);
            }
        }
        let Ok(payload) = encoder.encode(&frame) else {
            continue;
        };
        let dgram = pack_audio(unix_millis(), &payload);
        if dgram_tx.send(dgram).is_err() {
            break;
        }
    }
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

fn open_input(host: &cpal::Host, buf: Arc<Mutex<VecDeque<i16>>>) -> Option<Stream> {
    let device = host.default_input_device()?;
    let supported = device.default_input_config().ok()?;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let err_fn = |err| eprintln!("p2p-comm input stream: {err}");
    let stream = match sample_format {
        SampleFormat::F32 => build_input::<f32>(&device, &config, buf, err_fn)?,
        SampleFormat::I16 => build_input::<i16>(&device, &config, buf, err_fn)?,
        SampleFormat::I32 => build_input::<i32>(&device, &config, buf, err_fn)?,
        SampleFormat::U16 => build_input::<u16>(&device, &config, buf, err_fn)?,
        _ => return None,
    };
    stream.play().ok()?;
    Some(stream)
}

fn open_output(host: &cpal::Host, buf: Arc<Mutex<VecDeque<i16>>>) -> Option<Stream> {
    let device = host.default_output_device()?;
    let supported = device.default_output_config().ok()?;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let err_fn = |err| eprintln!("p2p-comm output stream: {err}");
    let stream = match sample_format {
        SampleFormat::F32 => build_output::<f32>(&device, &config, buf, err_fn)?,
        SampleFormat::I16 => build_output::<i16>(&device, &config, buf, err_fn)?,
        SampleFormat::I32 => build_output::<i32>(&device, &config, buf, err_fn)?,
        SampleFormat::U16 => build_output::<u16>(&device, &config, buf, err_fn)?,
        _ => return None,
    };
    stream.play().ok()?;
    Some(stream)
}

fn build_input<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    buf: Arc<Mutex<VecDeque<i16>>>,
    err_fn: impl FnMut(cpal::Error) + Send + 'static,
) -> Option<Stream>
where
    T: SizedSample + Send + 'static,
    i16: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let rate = config.sample_rate;
    device
        .build_input_stream(
            *config,
            move |data: &[T], _| {
                let mono = to_mono_i16(data, channels);
                let pcm = resample(&mono, rate, SAMPLE_RATE);
                if let Ok(mut q) = buf.lock() {
                    q.extend(pcm);
                    while q.len() > MAX_QUEUE {
                        q.pop_front();
                    }
                }
            },
            err_fn,
            None,
        )
        .ok()
}

fn build_output<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    buf: Arc<Mutex<VecDeque<i16>>>,
    err_fn: impl FnMut(cpal::Error) + Send + 'static,
) -> Option<Stream>
where
    T: SizedSample + FromSample<i16> + Send + 'static,
{
    let channels = usize::from(config.channels);
    let rate = config.sample_rate;
    device
        .build_output_stream(
            *config,
            move |data: &mut [T], _| {
                fill_output(data, channels, rate, &buf);
            },
            err_fn,
            None,
        )
        .ok()
}

fn to_mono_i16<T>(data: &[T], channels: usize) -> Vec<i16>
where
    T: Sample,
    i16: FromSample<T>,
{
    if channels == 0 {
        return Vec::new();
    }
    if channels == 1 {
        return data.iter().copied().map(Sample::to_sample::<i16>).collect();
    }
    data.chunks(channels)
        .map(|frame| {
            let sum: i32 = frame
                .iter()
                .copied()
                .map(|s| i32::from(s.to_sample::<i16>()))
                .sum();
            i16::try_from(sum / i32::try_from(channels).unwrap_or(1)).unwrap_or(0)
        })
        .collect()
}

// ponytail: linear resample, swap for a proper resampler if 44.1↔48 quality matters.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn resample(mono: &[i16], from: u32, to: u32) -> Vec<i16> {
    if from == to || from == 0 || mono.is_empty() {
        return mono.to_vec();
    }
    let out_len = (mono.len() as u64).saturating_mul(u64::from(to)) / u64::from(from);
    let out_len = usize::try_from(out_len).unwrap_or(0);
    if out_len == 0 {
        return Vec::new();
    }
    let last = mono.len() - 1;
    let mut out = vec![0i16; out_len];
    for (i, dst) in out.iter_mut().enumerate() {
        let src_pos = i as f64 * f64::from(from) / f64::from(to);
        let idx = src_pos as usize;
        let frac = src_pos - idx as f64;
        let a = f64::from(mono[idx.min(last)]);
        let b = f64::from(mono[(idx + 1).min(last)]);
        *dst = (a.mul_add(1.0 - frac, b * frac)) as i16;
    }
    out
}

fn fill_output<T>(out: &mut [T], channels: usize, dst_rate: u32, buf: &Arc<Mutex<VecDeque<i16>>>)
where
    T: Sample + FromSample<i16>,
{
    if channels == 0 || out.is_empty() {
        return;
    }
    let frames = out.len() / channels;
    let needed = if dst_rate == 0 {
        frames
    } else {
        let n = (frames as u64).saturating_mul(u64::from(SAMPLE_RATE)) / u64::from(dst_rate);
        usize::try_from(n).unwrap_or(frames)
    };
    let mut src = Vec::with_capacity(needed);
    if let Ok(mut q) = buf.lock() {
        for _ in 0..needed {
            src.push(q.pop_front().unwrap_or(0));
        }
    } else {
        src.resize(needed, 0);
    }
    let pcm = resample(&src, SAMPLE_RATE, dst_rate);
    for (i, frame) in out.chunks_mut(channels).enumerate() {
        let sample = pcm.get(i).copied().unwrap_or(0);
        let v = T::from_sample(sample);
        for slot in frame {
            *slot = v;
        }
    }
}
