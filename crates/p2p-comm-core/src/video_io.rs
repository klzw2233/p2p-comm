//! Default-camera capture. Encoding happens off the GUI thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};
use nokhwa::Camera;
use tokio::sync::mpsc;

use crate::video::{scale_rgb, VideoEncoder, FPS, HEIGHT, WIDTH};

/// Spawn a capture+encode thread. Dropping `stop` is not enough; the caller
/// must set `stop` then join. Returns `None` if the thread fails to spawn.
/// Camera open happens *inside* the thread because `Camera` is not `Send`.
pub fn start_capture(
    stop: Arc<AtomicBool>,
    dgram_tx: mpsc::UnboundedSender<Vec<u8>>,
    max_nal_len: usize,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("p2p-video-enc".into())
        .spawn(move || capture_loop(&stop, &dgram_tx, max_nal_len))
        .ok()
}

fn open_camera() -> Option<Camera> {
    // Encode is always 640×480; native capture is scaled. Exact 640×480 is not
    // required of the device (`HighestResolution(640×480)` would fail otherwise).
    let requested =
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestResolution);
    Camera::new(CameraIndex::Index(0), requested).ok()
}

fn capture_loop(stop: &AtomicBool, dgram_tx: &mpsc::UnboundedSender<Vec<u8>>, max_nal_len: usize) {
    let Some(mut camera) = open_camera() else {
        return;
    };
    if camera.open_stream().is_err() {
        return;
    }
    let Ok(mut encoder) = VideoEncoder::new(max_nal_len) else {
        let _ = camera.stop_stream();
        return;
    };
    let interval = std::time::Duration::from_millis(1000 / u64::from(FPS.max(1)));
    while !stop.load(Ordering::Relaxed) {
        let started = std::time::Instant::now();
        match camera.frame() {
            Ok(buf) => {
                if let Ok(image) = buf.decode_image::<RgbFormat>() {
                    let (w, h) = image.dimensions();
                    let scaled = scale_rgb(image.as_raw(), w, h, WIDTH, HEIGHT);
                    if let Ok(dgrams) = encoder.encode_rgb(&scaled, WIDTH, HEIGHT, unix_millis()) {
                        for d in dgrams {
                            if dgram_tx.send(d).is_err() {
                                let _ = camera.stop_stream();
                                return;
                            }
                        }
                    }
                }
            }
            Err(_) => {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
            }
        }
        let slept = interval.saturating_sub(started.elapsed());
        if !slept.is_zero() {
            std::thread::sleep(slept);
        }
    }
    let _ = camera.stop_stream();
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}
