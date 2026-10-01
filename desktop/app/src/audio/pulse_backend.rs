//! Linux: capture the default sink's monitor — everything the computer plays —
//! and play the phone's audio, through the PulseAudio API. PipeWire serves the
//! same API (pipewire-pulse) on current distributions, so one small client
//! library covers both.

use std::sync::mpsc::{channel, sync_channel, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;

use libpulse_binding::def::BufferAttr;
use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::Direction;
use libpulse_simple_binding::Simple;
use tethertone_core::jitter::UpstreamBuffer;
use tethertone_core::StreamFormat;

use super::{f32_to_s16le, BlockSink, StreamHandle};

/// PulseAudio resamples to whatever is asked for, so the format is fixed.
pub const FORMAT: StreamFormat = StreamFormat { sample_rate: 48_000, channels: 2, block_frames: 480 };
const BLOCK_BYTES: usize = 480 * 2 * 2;

fn spec() -> Spec {
    Spec { format: Format::S16le, rate: FORMAT.sample_rate, channels: FORMAT.channels as u8 }
}

fn attr(fragment_bytes: u32, target_bytes: u32) -> BufferAttr {
    BufferAttr { maxlength: u32::MAX, tlength: target_bytes, prebuf: u32::MAX, minreq: u32::MAX, fragsize: fragment_bytes }
}

/// Opens `open` on a dedicated thread, then runs `pump` there until the handle
/// is dropped or `pump` returns false.
fn run<O, P>(name: &str, open: O, mut pump: P) -> Result<StreamHandle, String>
where
    O: FnOnce() -> Result<Simple, String> + Send + 'static,
    P: FnMut(&Simple) -> bool + Send + 'static,
{
    let (ready_tx, ready_rx) = sync_channel::<Result<(), String>>(1);
    let (stop, stopped) = channel::<()>();
    let thread = thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let stream = match open() {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            while matches!(stopped.try_recv(), Err(TryRecvError::Empty)) {
                if !pump(&stream) {
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    ready_rx.recv().map_err(|_| "audio thread ended unexpectedly".to_string())??;
    Ok(StreamHandle::new(stop, thread))
}

pub fn start_monitor(_format: StreamFormat, mut sink: BlockSink) -> Result<StreamHandle, String> {
    let mut block = vec![0u8; BLOCK_BYTES];
    run(
        "tethertone-capture",
        || {
            Simple::new(
                None,
                "Tethertone",
                Direction::Record,
                Some("@DEFAULT_MONITOR@"),
                "System audio",
                &spec(),
                None,
                Some(&attr(BLOCK_BYTES as u32, u32::MAX)),
            )
            .map_err(|e| format!("Could not capture system audio (PulseAudio/PipeWire): {e}"))
        },
        move |stream| match stream.read(&mut block) {
            Ok(()) => {
                sink(&block);
                true
            }
            Err(e) => {
                eprintln!("[audio] capture stopped: {e}");
                false
            }
        },
    )
}

pub fn start_playback(buffer: Arc<Mutex<UpstreamBuffer>>) -> Result<StreamHandle, String> {
    let mut samples = vec![0.0f32; 480 * 2];
    let mut bytes = Vec::with_capacity(BLOCK_BYTES);
    run(
        "tethertone-playback",
        || {
            // A short server-side buffer: our own jitter buffer does the smoothing.
            Simple::new(
                None,
                "Tethertone",
                Direction::Playback,
                None,
                "Phone audio",
                &spec(),
                None,
                Some(&attr(u32::MAX, (BLOCK_BYTES * 4) as u32)),
            )
            .map_err(|e| format!("Could not play the phone's audio (PulseAudio/PipeWire): {e}"))
        },
        move |stream| {
            buffer.lock().unwrap().pull(&mut samples, 2, FORMAT.sample_rate);
            f32_to_s16le(&samples, &mut bytes);
            stream.write(&bytes).is_ok() // blocks at the device's pace
        },
    )
}
