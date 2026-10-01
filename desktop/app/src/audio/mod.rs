//! Capture (what the computer plays) and playback (what the phone sends up).
//!
//! Every backend runs its stream on a thread of its own and is stopped by
//! dropping the handle: some audio APIs' stream objects may not cross threads,
//! and this keeps the rest of the app free of that concern.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use tethertone_core::jitter::UpstreamBuffer;
use tethertone_core::StreamFormat;

mod tone;
#[cfg(not(target_os = "linux"))]
mod cpal_backend;
#[cfg(target_os = "linux")]
mod pulse_backend;

/// Where the audio sent to phones comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    /// Everything the computer plays: WASAPI loopback on Windows, the default
    /// sink's monitor on Linux (PulseAudio or PipeWire).
    System,
    /// A quiet 440 Hz tone. Needs no permission or audio device: for tests and CI.
    TestTone,
}

/// Called with exactly one block (`format.block_frames`) of interleaved s16le.
pub type BlockSink = Box<dyn FnMut(&[u8]) + Send>;

/// A running stream; dropping it stops the stream and joins its thread.
pub struct StreamHandle {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl StreamHandle {
    pub(crate) fn new(stop: Sender<()>, thread: JoinHandle<()>) -> Self {
        StreamHandle { stop: Some(stop), thread: Some(thread) }
    }
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        drop(self.stop.take()); // a closed channel is the stop signal
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The format a capture will deliver, known before it starts so the server can
/// advertise it in WELCOME.
pub fn capture_format(kind: CaptureKind) -> Result<StreamFormat, String> {
    match kind {
        CaptureKind::TestTone => Ok(tone::FORMAT),
        #[cfg(not(target_os = "linux"))]
        CaptureKind::System => cpal_backend::loopback_format(),
        #[cfg(target_os = "linux")]
        CaptureKind::System => Ok(pulse_backend::FORMAT),
    }
}

pub fn start_capture(kind: CaptureKind, format: StreamFormat, sink: BlockSink) -> Result<StreamHandle, String> {
    match kind {
        CaptureKind::TestTone => tone::start(sink),
        #[cfg(not(target_os = "linux"))]
        CaptureKind::System => cpal_backend::start_loopback(format, sink),
        #[cfg(target_os = "linux")]
        CaptureKind::System => pulse_backend::start_monitor(format, sink),
    }
}

/// Consumes the phone's audio at real-time pace without an audio device:
/// test mode and CI, where nothing may be heard and there may be no device.
pub fn start_null_playback(buffer: Arc<Mutex<UpstreamBuffer>>) -> Result<StreamHandle, String> {
    use std::sync::mpsc::{channel, RecvTimeoutError};
    use std::time::{Duration, Instant};
    let (stop, stopped) = channel::<()>();
    let thread = std::thread::Builder::new()
        .name("tethertone-null-playback".into())
        .spawn(move || {
            let mut out = vec![0.0f32; 480 * 2];
            let mut next = Instant::now();
            loop {
                buffer.lock().unwrap().pull(&mut out, 2, 48_000);
                next += Duration::from_millis(10);
                match stopped.recv_timeout(next.saturating_duration_since(Instant::now())) {
                    Err(RecvTimeoutError::Timeout) => continue,
                    _ => return,
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(StreamHandle::new(stop, thread))
}

/// Plays the phone's audio on the default output device.
pub fn start_playback(buffer: Arc<Mutex<UpstreamBuffer>>) -> Result<StreamHandle, String> {
    #[cfg(not(target_os = "linux"))]
    return cpal_backend::start_playback(buffer);
    #[cfg(target_os = "linux")]
    return pulse_backend::start_playback(buffer);
}

/// Turns arbitrary-sized interleaved s16le into exact blocks.
pub(crate) struct Blocker {
    pending: Vec<u8>,
    block_bytes: usize,
}

impl Blocker {
    pub fn new(format: StreamFormat) -> Self {
        let block_bytes = format.block_frames as usize * format.channels as usize * 2;
        Blocker { pending: Vec::with_capacity(block_bytes * 2), block_bytes }
    }

    pub fn push(&mut self, pcm: &[u8], sink: &mut BlockSink) {
        self.pending.extend_from_slice(pcm);
        let mut start = 0;
        while self.pending.len() - start >= self.block_bytes {
            sink(&self.pending[start..start + self.block_bytes]);
            start += self.block_bytes;
        }
        self.pending.drain(..start);
    }
}

/// f32 samples to s16le bytes, clamped.
pub(crate) fn f32_to_s16le(samples: &[f32], out: &mut Vec<u8>) {
    out.clear();
    for &s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
}
