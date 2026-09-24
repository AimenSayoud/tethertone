import AVFoundation
import CoreAudio
import Foundation

/// Plays the captured audio back out of the Mac's own speakers.
///
/// Only needed for the BlackHole path: routing system output into BlackHole
/// means the Mac itself goes silent, and the usual cure is to build a
/// Multi-Output Device by hand in Audio MIDI Setup. This does the same job
/// without the ritual. The ScreenCaptureKit path does not need it at all —
/// that one taps the mix rather than redirecting it, so the speakers keep
/// working on their own.
final class MonitorPlayback {

    private let engine = AVAudioEngine()
    private var source: AVAudioSourceNode?
    private var format: StreamFormat
    private var running = false

    /// Single producer (capture thread), single consumer (render callback).
    /// Sized for a quarter second, which is far more than the few milliseconds
    /// it normally holds — the headroom is only there so a scheduling hiccup
    /// does not turn into a gap.
    private var ring: UnsafeMutablePointer<Float>
    private let capacity: Int
    private var writeIndex = 0
    private var readIndex = 0
    private var lock = os_unfair_lock_s()

    var volume: Float = 1.0 {
        didSet { engine.mainMixerNode.outputVolume = max(0, min(1, volume)) }
    }

    init(format: StreamFormat) {
        self.format = format
        self.capacity = format.sampleRate * format.channels / 4
        self.ring = .allocate(capacity: capacity)
        self.ring.initialize(repeating: 0, count: capacity)
    }

    deinit {
        ring.deallocate()
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

        let streamFormat = AVAudioFormat(
            standardFormatWithSampleRate: Double(format.sampleRate),
            channels: AVAudioChannelCount(format.channels)
        )!

        let node = AVAudioSourceNode(format: streamFormat) { [weak self] _, _, frameCount, audioBufferList in
            guard let self else { return noErr }
            self.render(frames: Int(frameCount), into: audioBufferList)
            return noErr
        }

        engine.attach(node)
        engine.connect(node, to: engine.mainMixerNode, format: streamFormat)
        engine.mainMixerNode.outputVolume = max(0, min(1, volume))
        engine.prepare()
        try engine.start()

        source = node
        running = true
        NSLog("[audio] monitoring through %@",
              deviceID.flatMap(AudioDevices.name(of:)) ?? "the default output")
    }

    func stop() {
        guard running else { return }
        engine.stop()
        if let source { engine.detach(source) }
        source = nil
        running = false
        os_unfair_lock_lock(&lock)
        readIndex = 0
        writeIndex = 0
        os_unfair_lock_unlock(&lock)
    }

    /// Called from the capture thread with interleaved 16-bit PCM.
    func push(_ pcm: Data) {
        guard running else { return }
        pcm.withUnsafeBytes { raw in
            let samples = raw.bindMemory(to: Int16.self)
            os_unfair_lock_lock(&lock)
            defer { os_unfair_lock_unlock(&lock) }
            for sample in samples {
                let next = (writeIndex + 1) % capacity
                // Full: drop the newest rather than overwrite what is about to
                // play. Monitoring is a convenience; it must never stutter the
                // stream it shares a thread with.
                if next == readIndex { return }
                ring[writeIndex] = Float(sample) / 32768.0
                writeIndex = next
            }
        }
    }

    private func render(frames: Int, into audioBufferList: UnsafeMutablePointer<AudioBufferList>) {
        let buffers = UnsafeMutableAudioBufferListPointer(audioBufferList)
        let channels = format.channels

        os_unfair_lock_lock(&lock)
        defer { os_unfair_lock_unlock(&lock) }

        for frame in 0..<frames {
            for channel in 0..<channels {
                var value: Float = 0
                if readIndex != writeIndex {
                    value = ring[readIndex]
                    readIndex = (readIndex + 1) % capacity
                }
                if channel < buffers.count,
                   let data = buffers[channel].mData?.assumingMemoryBound(to: Float.self) {
                    data[frame] = value
                }
            }
        }
    }
}

enum AudioOutputs {
    /// Output devices. Loopback devices such as BlackHole are left out unless
    /// asked for: monitoring into one would feed capture back into itself, but
    /// they are exactly the target for "use the phone as a microphone".
    static func devices(includeLoopback: Bool = false) -> [AudioInputDevice] {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDevices,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var size: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(
            AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size) == noErr else { return [] }
        var ids = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
        guard AudioObjectGetPropertyData(
            AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &ids) == noErr else { return [] }

        return ids.compactMap { id in
            let channels = outputChannelCount(id)
            guard channels > 0 else { return nil }
            let name = AudioDevices.name(of: id) ?? "device \(id)"
            if !includeLoopback, name.localizedCaseInsensitiveContains("blackhole") { return nil }
            return AudioInputDevice(id: id, name: name, inputChannels: channels,
                                    sampleRate: AudioDevices.nominalRate(id))
        }
    }

    /// The device macOS currently sends system output to.
    static func defaultOutput() -> AudioDeviceID? {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDefaultOutputDevice,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var id = AudioDeviceID(0)
        var size = UInt32(MemoryLayout<AudioDeviceID>.size)
        guard AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &id) == noErr,
              id != 0 else { return nil }
        return id
    }

    private static func outputChannelCount(_ id: AudioDeviceID) -> Int {
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyStreamConfiguration,
            mScope: kAudioObjectPropertyScopeOutput,
            mElement: kAudioObjectPropertyElementMain
        )
        var size: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(id, &address, 0, nil, &size) == noErr, size > 0 else { return 0 }
        let raw = UnsafeMutableRawPointer.allocate(byteCount: Int(size),
                                                   alignment: MemoryLayout<AudioBufferList>.alignment)
        defer { raw.deallocate() }
        guard AudioObjectGetPropertyData(id, &address, 0, nil, &size, raw) == noErr else { return 0 }
        let list = UnsafeMutableAudioBufferListPointer(raw.assumingMemoryBound(to: AudioBufferList.self))
        return list.reduce(0) { $0 + Int($1.mNumberChannels) }
    }
}
