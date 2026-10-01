//! A quiet 440 Hz tone in 10 ms blocks, paced to real time.

use std::sync::mpsc::{channel, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use tethertone_core::StreamFormat;

use super::{BlockSink, StreamHandle};

pub const FORMAT: StreamFormat = StreamFormat { sample_rate: 48_000, channels: 2, block_frames: 480 };

pub fn start(mut sink: BlockSink) -> Result<StreamHandle, String> {
    let (stop, stopped) = channel::<()>();
    let thread = thread::Builder::new()
        .name("tethertone-tone".into())
        .spawn(move || {
            let mut phase = 0.0f64;
            let step = 2.0 * std::f64::consts::PI * 440.0 / FORMAT.sample_rate as f64;
            let mut block = vec![0u8; FORMAT.block_frames as usize * 4];
            let mut next = Instant::now();
            loop {
                for frame in block.chunks_exact_mut(4) {
                    let v = ((phase.sin() * 3000.0) as i16).to_le_bytes();
                    phase = (phase + step) % (2.0 * std::f64::consts::PI);
                    frame[..2].copy_from_slice(&v);
                    frame[2..].copy_from_slice(&v);
                }
                sink(&block);
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
