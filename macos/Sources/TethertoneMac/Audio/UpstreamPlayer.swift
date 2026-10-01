import AVFoundation
import CoreAudio
import Foundation

/// Plays audio a phone sends up: its microphone or what it is playing.
///
/// The mirror of the phone's playback path. Packets arrive in network bursts,
/// so a jitter buffer holds [targetMs] of audio; and the phone's capture clock
/// and this Mac's output clock disagree by tens of ppm, so the render callback
/// reads a fraction of a percent fast or slow to hold the buffer at target —
/// linear interpolation at ±0.2%, which nobody can hear, and never a click.
///
/// Output goes to any device: the speakers, or a loopback device such as
/// BlackHole so other apps can use the phone as a microphone.
final class UpstreamPlayer {

    let sampleRate: Int
    let channels: Int
    let targetMs: Int

    /// Peak of each pushed block, 0…1. Called on the pushing thread.
    var onLevel: ((Float) -> Void)?

    private let engine = AVAudioEngine()
    private var node: AVAudioSourceNode?
    private var running = false

    // Interleaved float ring. Single producer (network queue), single consumer
    // (render thread), guarded by one lock held only for copies.
    private let ring: UnsafeMutablePointer<Float>
    private let capacity: Int
    private var writeIndex = 0
    private var readIndex = 0
    private var count = 0
    private var fraction = 0.0
    private var speed = 1.0
    private var primed = false
    private var lock = os_unfair_lock_s()

    private(set) var underruns = 0
    private(set) var overruns = 0

    static let maxDeviation = 0.002
    static let deadZoneMs = 4

    init(sampleRate: Int, channels: Int, targetMs: Int = 60) {
        self.sampleRate = sampleRate
        // Mono (microphone) or stereo (captured playback); the render callback
        // is written for at most two so it never allocates on the audio thread.
        self.channels = max(1, min(2, channels))
        self.targetMs = targetMs
        self.capacity = sampleRate * self.channels * 2      // two seconds
        self.ring = .allocate(capacity: capacity)
        self.ring.initialize(repeating: 0, count: capacity)
    }

    deinit {
        stop()
        ring.deallocate()
    }

    var volume: Float = 1 {
        didSet { engine.mainMixerNode.outputVolume = max(0, min(1, volume)) }
    }

    /// Milliseconds of audio waiting to play.
    var bufferedMs: Int {
        os_unfair_lock_lock(&lock); defer { os_unfair_lock_unlock(&lock) }
        return count / channels * 1000 / sampleRate
    }

    var currentSpeed: Double {
        os_unfair_lock_lock(&lock); defer { os_unfair_lock_unlock(&lock) }
        return speed
    }

    func start(deviceID: AudioDeviceID?) throws {
        guard !running else { return }
        if let deviceID, let unit = engine.outputNode.audioUnit {
            var id = deviceID
            let status = AudioUnitSetProperty(
                unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0,
                &id, UInt32(MemoryLayout<AudioDeviceID>.size)
            )
            guard status == noErr else { throw CaptureError.deviceRejected(status) }
        }

        let format = AVAudioFormat(standardFormatWithSampleRate: Double(sampleRate),
                                   channels: AVAudioChannelCount(channels))!
        let node = AVAudioSourceNode(format: format) { [weak self] _, _, frameCount, bufferList in
            self?.render(frames: Int(frameCount), into: bufferList)
            return noErr
        }
        engine.attach(node)
        engine.connect(node, to: engine.mainMixerNode, format: format)
        engine.mainMixerNode.outputVolume = max(0, min(1, volume))
        engine.prepare()
        try engine.start()
        self.node = node
        running = true
        NSLog("[up] playing phone audio (%d Hz, %dch) through %@", sampleRate, channels,
              deviceID.flatMap(AudioDevices.name(of:)) ?? "the default output")
    }

    func stop() {
        guard running else { return }
        engine.stop()
        if let node { engine.detach(node) }
        node = nil
        running = false
    }

    /// Interleaved s16le PCM from the network.
    func push(_ pcm: Data) {
        var peak: Int16 = 0
        pcm.withUnsafeBytes { raw in
            let samples = raw.bindMemory(to: Int16.self)
            os_unfair_lock_lock(&lock)
            defer { os_unfair_lock_unlock(&lock) }
            for sample in samples {
                if count == capacity {                      // full: drop the oldest
                    readIndex = (readIndex + 1) % capacity
                    count -= 1
                }
                ring[writeIndex] = Float(sample) / 32768
                writeIndex = (writeIndex + 1) % capacity
                count += 1
                let magnitude = sample == .min ? .max : abs(sample)
                if magnitude > peak { peak = magnitude }
            }
            // A burst far beyond target would take minutes to drain by speed
            // alone; shed it now in one step.
            let targetSamples = targetMs * sampleRate / 1000 * channels
            if count > targetSamples * 4 {
                let drop = count - targetSamples
                readIndex = (readIndex + drop) % capacity
                count -= drop
                overruns += 1
            }
            speed = Self.speed(bufferedMs: count / channels * 1000 / sampleRate, targetMs: targetMs)
        }
        onLevel?(Float(peak) / 32768)
    }

    /// Proportional control on fill level, clamped to ±0.2% — the same law as
    /// the phone's `JitterBuffer.speedCorrection`.
    static func speed(bufferedMs: Int, targetMs: Int) -> Double {
        let error = bufferedMs - targetMs
        guard abs(error) >= deadZoneMs, targetMs > 0 else { return 1 }
        let normalised = max(-1, min(1, Double(error) / Double(targetMs)))
        return 1 + normalised * maxDeviation
    }

    private func render(frames: Int, into bufferList: UnsafeMutablePointer<AudioBufferList>) {
        let buffers = UnsafeMutableAudioBufferListPointer(bufferList)
        let left = buffers.count > 0 ? buffers[0].mData?.assumingMemoryBound(to: Float.self) : nil
        let right = channels > 1 && buffers.count > 1 ? buffers[1].mData?.assumingMemoryBound(to: Float.self) : nil

        os_unfair_lock_lock(&lock)
        defer { os_unfair_lock_unlock(&lock) }

        let targetSamples = targetMs * sampleRate / 1000 * channels
        if !primed && count >= targetSamples { primed = true }

        for frame in 0..<frames {
            // Interpolation needs this frame and the next one.
            guard primed, count >= channels * 2 else {
                if primed { underruns += 1; primed = false }
                left?[frame] = 0
                right?[frame] = 0
                continue
            }
            let next = (readIndex + channels) % capacity
            let t = Float(fraction)
            let a0 = ring[readIndex], b0 = ring[next]
            left?[frame] = a0 + (b0 - a0) * t
            if channels > 1 {
                let a1 = ring[(readIndex + 1) % capacity], b1 = ring[(next + 1) % capacity]
                right?[frame] = a1 + (b1 - a1) * t
            }
            fraction += speed
            while fraction >= 1 && count >= channels * 2 {
                readIndex = (readIndex + channels) % capacity
                count -= channels
                fraction -= 1
            }
        }
    }
}
