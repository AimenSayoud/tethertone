//! The receiving end of the phone's audio: a jitter buffer with continuous
//! clock-drift correction.
//!
//! Same law as the phone's `JitterBuffer.speedCorrection` and the macOS
//! `UpstreamPlayer`: hold the buffer at its target by reading up to 0.2% fast
//! or slow, with linear interpolation — inaudible, and never a click. Pure
//! logic, no locking: the caller shares it between the network thread and the
//! audio callback behind a mutex held only for the copy.

pub const MAX_DEVIATION: f64 = 0.002;
pub const DEAD_ZONE_MS: i64 = 4;

/// Playback speed for a buffer holding `buffered_ms` against `target_ms`.
pub fn speed(buffered_ms: i64, target_ms: i64) -> f64 {
    let error = buffered_ms - target_ms;
    if error.abs() < DEAD_ZONE_MS || target_ms <= 0 {
        return 1.0;
    }
    let normalised = (error as f64 / target_ms as f64).clamp(-1.0, 1.0);
    1.0 + normalised * MAX_DEVIATION
}

pub struct UpstreamBuffer {
    pub sample_rate: u32,
    pub channels: usize,
    pub target_ms: i64,
    ring: Vec<f32>,
    write: usize,
    read: usize,
    count: usize,
    fraction: f64,
    speed: f64,
    primed: bool,
    pub underruns: u64,
    pub overruns: u64,
}

impl UpstreamBuffer {
    pub fn new(sample_rate: u32, channels: u16, target_ms: i64) -> Self {
        let channels = channels.clamp(1, 2) as usize;
        UpstreamBuffer {
            sample_rate,
            channels,
            target_ms,
            ring: vec![0.0; sample_rate as usize * channels * 2], // two seconds
            write: 0,
            read: 0,
            count: 0,
            fraction: 0.0,
            speed: 1.0,
            primed: false,
            underruns: 0,
            overruns: 0,
        }
    }

    pub fn buffered_ms(&self) -> i64 {
        (self.count / self.channels) as i64 * 1000 / self.sample_rate as i64
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    fn target_samples(&self) -> usize {
        (self.target_ms as usize * self.sample_rate as usize / 1000) * self.channels
    }

    /// Interleaved s16le PCM from the network. Returns its peak, 0..=1.
    pub fn push_s16le(&mut self, pcm: &[u8]) -> f32 {
        let capacity = self.ring.len();
        let mut peak = 0i32;
        for chunk in pcm.chunks_exact(2) {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
            peak = peak.max((sample as i32).abs());
            if self.count == capacity {
                self.read = (self.read + 1) % capacity; // full: drop the oldest
                self.count -= 1;
            }
            self.ring[self.write] = sample as f32 / 32768.0;
            self.write = (self.write + 1) % capacity;
            self.count += 1;
        }
        // A burst far beyond target would take minutes to drain by speed alone.
        let target = self.target_samples();
        if self.count > target * 4 {
            let drop = (self.count - target) / self.channels * self.channels;
            self.read = (self.read + drop) % capacity;
            self.count -= drop;
            self.overruns += 1;
        }
        self.speed = speed(self.buffered_ms(), self.target_ms);
        (peak as f32 / 32768.0).min(1.0)
    }

    /// Fills interleaved `out` (with `out_channels` channels at `out_rate`)
    /// for the audio device, resampling by `speed` and by the ratio between
    /// the phone's rate and the device's. Mono becomes both channels; stereo
    /// is averaged for a mono device.
    pub fn pull(&mut self, out: &mut [f32], out_channels: usize, out_rate: u32) {
        let capacity = self.ring.len();
        let step = self.speed * self.sample_rate as f64 / out_rate.max(1) as f64;
        let ch = self.channels;
        if !self.primed && self.count >= self.target_samples() {
            self.primed = true;
        }
        for frame in out.chunks_mut(out_channels.max(1)) {
            if !self.primed || self.count < ch * 2 {
                if self.primed {
                    self.underruns += 1;
                    self.primed = false;
                }
                frame.fill(0.0);
                continue;
            }
            let next = (self.read + ch) % capacity;
            let t = self.fraction as f32;
            let sample = |c: usize| {
                let a = self.ring[(self.read + c) % capacity];
                let b = self.ring[(next + c) % capacity];
                a + (b - a) * t
            };
            let (left, right) = if ch == 1 { (sample(0), sample(0)) } else { (sample(0), sample(1)) };
            match frame.len() {
                1 => frame[0] = (left + right) * 0.5,
                _ => {
                    frame[0] = left;
                    frame[1] = right;
                    for extra in frame.iter_mut().skip(2) {
                        *extra = 0.0;
                    }
                }
            }
            self.fraction += step;
            while self.fraction >= 1.0 && self.count >= ch * 2 {
                self.read = (self.read + ch) % capacity;
                self.count -= ch;
                self.fraction -= 1.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(samples: usize, value: i16) -> Vec<u8> {
        std::iter::repeat_n(value.to_le_bytes(), samples).flatten().collect()
    }

    #[test]
    fn drift_law_matches_the_other_implementations() {
        assert_eq!(speed(60, 60), 1.0);
        assert_eq!(speed(62, 60), 1.0);
        assert_eq!(speed(120, 60), 1.002);
        assert_eq!(speed(0, 60), 0.998);
        assert_eq!(speed(900, 60), 1.002);
    }

    #[test]
    fn plays_silence_until_the_target_is_buffered() {
        let mut b = UpstreamBuffer::new(48_000, 1, 60);
        b.push_s16le(&pcm(480, 1000)); // 10 ms
        let mut out = vec![1.0f32; 64];
        b.pull(&mut out, 2, 48_000);
        assert!(out.iter().all(|&s| s == 0.0));
        assert_eq!(b.underruns, 0, "not primed yet is not an underrun");
    }

    #[test]
    fn mono_is_spread_to_both_channels() {
        let mut b = UpstreamBuffer::new(48_000, 1, 10);
        b.push_s16le(&pcm(4800, 16384));
        let mut out = vec![0.0f32; 20];
        b.pull(&mut out, 2, 48_000);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6), "{out:?}");
    }

    #[test]
    fn running_dry_counts_one_underrun() {
        let mut b = UpstreamBuffer::new(48_000, 1, 10);
        b.push_s16le(&pcm(480, 100));
        let mut out = vec![0.0f32; 4000];
        b.pull(&mut out, 1, 48_000);
        assert_eq!(b.underruns, 1);
    }

    #[test]
    fn a_slower_device_consumes_proportionally_fewer_samples() {
        let mut b = UpstreamBuffer::new(48_000, 1, 10);
        b.push_s16le(&pcm(4800, 100)); // 100 ms
        let before = b.buffered_ms();
        let mut out = vec![0.0f32; 441]; // 10 ms at 44.1 kHz
        b.pull(&mut out, 1, 44_100);
        let consumed = before - b.buffered_ms();
        assert!((9..=11).contains(&consumed), "consumed {consumed} ms for 10 ms of output");
    }

    #[test]
    fn a_huge_burst_is_shed_back_to_target() {
        let mut b = UpstreamBuffer::new(48_000, 1, 60);
        b.push_s16le(&pcm(48_000, 1)); // one second at once
        assert_eq!(b.buffered_ms(), 60);
        assert_eq!(b.overruns, 1);
    }
}
