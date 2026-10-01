//! Windows: WASAPI loopback capture of the default output, and playback of
//! the phone's audio. (Builds on macOS too, for development; there loopback
//! is unavailable and the native macOS app is the real thing.)

use std::sync::mpsc::{channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use tethertone_core::jitter::UpstreamBuffer;
use tethertone_core::StreamFormat;

use super::{f32_to_s16le, BlockSink, Blocker, StreamHandle};

fn output_device() -> Result<cpal::Device, String> {
    cpal::default_host().default_output_device().ok_or_else(|| "No audio output device found.".to_string())
}

pub fn loopback_format() -> Result<StreamFormat, String> {
    if !cfg!(windows) {
        return Err("System audio capture is for Windows and Linux. On a Mac, use the native Tethertone app.".into());
    }
    let config = output_device()?.default_output_config().map_err(|e| e.to_string())?;
    let rate = config.sample_rate();
    Ok(StreamFormat { sample_rate: rate, channels: 2, block_frames: rate / 100 })
}

/// Runs `build` on a dedicated thread that owns the resulting streams until
/// the handle is dropped, and reports whether building succeeded.
fn run_streams<F>(name: &str, build: F) -> Result<StreamHandle, String>
where
    F: FnOnce() -> Result<Vec<Stream>, String> + Send + 'static,
{
    let (ready_tx, ready_rx) = sync_channel::<Result<(), String>>(1);
    let (stop, stopped) = channel::<()>();
    let thread = thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let streams = match build() {
                Ok(streams) => streams,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            for stream in &streams {
                if let Err(e) = stream.play() {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            }
            let _ = ready_tx.send(Ok(()));
            let _ = stopped.recv(); // until the handle is dropped
            drop(streams);
        })
        .map_err(|e| e.to_string())?;
    ready_rx.recv().map_err(|_| "audio thread ended unexpectedly".to_string())??;
    Ok(StreamHandle::new(stop, thread))
}

pub fn start_loopback(format: StreamFormat, sink: BlockSink) -> Result<StreamHandle, String> {
    run_streams("tethertone-capture", move || {
        let device = output_device()?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let config = supported.config();
        let device_channels = config.channels as usize;

        let mut blocker = Blocker::new(format);
        let mut sink = sink;
        let mut stereo: Vec<f32> = Vec::new();
        let mut bytes: Vec<u8> = Vec::new();
        // Any channel count becomes stereo: the first two channels.
        let mut deliver = move |frames: &mut dyn Iterator<Item = f32>| {
            stereo.clear();
            for (index, sample) in frames.enumerate() {
                if index % device_channels < 2 {
                    stereo.push(sample);
                    if device_channels == 1 {
                        stereo.push(sample);
                    }
                }
            }
            f32_to_s16le(&stereo, &mut bytes);
            blocker.push(&bytes, &mut sink);
        };

        let error = |e| eprintln!("[audio] capture error: {e}");
        // On Windows an input stream on an *output* device is loopback capture.
        let capture = match supported.sample_format() {
            SampleFormat::F32 => device.build_input_stream::<f32, _, _>(
                config,
                move |data, _| deliver(&mut data.iter().copied()),
                error,
                None,
            ),
            SampleFormat::I16 => device.build_input_stream::<i16, _, _>(
                config,
                move |data, _| deliver(&mut data.iter().map(|&s| s as f32 / 32768.0)),
                error,
                None,
            ),
            other => return Err(format!("Unsupported output sample format {other:?}.")),
        }
        .map_err(|e| format!("Could not capture system audio: {e}"))?;

        // WASAPI loopback delivers nothing while the device is silent, which a
        // phone would hear as underruns. Playing silence keeps it clocking.
        let keep_alive = device
            .build_output_stream::<f32, _, _>(supported.config(), |data, _| data.fill(0.0), |_| {}, None)
            .ok();
        Ok(std::iter::once(capture).chain(keep_alive).collect())
    })
}

pub fn start_playback(buffer: Arc<Mutex<UpstreamBuffer>>) -> Result<StreamHandle, String> {
    run_streams("tethertone-playback", move || {
        let device = output_device()?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let config = supported.config();
        let channels = config.channels as usize;
        let rate = config.sample_rate;
        let error = |e| eprintln!("[audio] playback error: {e}");
        let stream = match supported.sample_format() {
            SampleFormat::F32 => device.build_output_stream::<f32, _, _>(
                config,
                move |data, _| buffer.lock().unwrap().pull(data, channels, rate),
                error,
                None,
            ),
            SampleFormat::I16 => {
                let mut scratch: Vec<f32> = Vec::new();
                device.build_output_stream::<i16, _, _>(
                    config,
                    move |data, _| {
                        scratch.resize(data.len(), 0.0);
                        buffer.lock().unwrap().pull(&mut scratch, channels, rate);
                        for (out, s) in data.iter_mut().zip(&scratch) {
                            *out = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                        }
                    },
                    error,
                    None,
                )
            }
            other => return Err(format!("Unsupported output sample format {other:?}.")),
        }
        .map_err(|e| format!("Could not play the phone's audio: {e}"))?;
        Ok(vec![stream])
    })
}
