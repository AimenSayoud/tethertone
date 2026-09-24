import Foundation

/// A quiet 440 Hz tone in 10 ms blocks, with no capture permission needed.
///
/// Used by `--headless --test-tone` so the server can be exercised end to end
/// — by tools/probe.py, the desktop sink or CI — on a machine where nobody can
/// click a Screen Recording or Microphone prompt.
final class TestToneSource: AudioSource {
    let format = StreamFormat(sampleRate: 48000, channels: 2, blockFrames: 480)
    var onBlock: ((Data) -> Void)?
    var onLevel: ((Float) -> Void)?
    var onFailure: ((Error) -> Void)?

    private var timer: DispatchSourceTimer?
    private var phase = 0.0
    private let queue = DispatchQueue(label: "dev.tethertone.testtone", qos: .userInteractive)

    func start() throws {
        let timer = DispatchSource.makeTimerSource(flags: .strict, queue: queue)
        timer.schedule(deadline: .now(), repeating: .milliseconds(10), leeway: .microseconds(500))
        timer.setEventHandler { [weak self] in self?.emit() }
        timer.resume()
        self.timer = timer
    }

    func stop() {
        timer?.cancel()
        timer = nil
    }

    private func emit() {
        var block = Data(count: format.blockBytes)
        let step = 2 * Double.pi * 440 / Double(format.sampleRate)
        block.withUnsafeMutableBytes { raw in
            let samples = raw.bindMemory(to: Int16.self)
            for frame in 0..<format.blockFrames {
                let value = Int16(sin(phase) * 3000)
                phase += step
                samples[frame * 2] = value
                samples[frame * 2 + 1] = value
            }
        }
        if phase > 2 * .pi * 1000 { phase = phase.truncatingRemainder(dividingBy: 2 * .pi) }
        onBlock?(block)
        onLevel?(3000 / 32768)
    }
}
