import Foundation
import Network

struct ClientSnapshot: Identifiable, Equatable {
    let id: ObjectIdentifier
    var name: String
    var peer: String
    var rttMs: Double
    var dropped: Int
    var bufferMs: Int
    var underruns: Int
    var lost: Int
    var speed: Double
    var connectedAt: Date
    /// Whether this client can do two-way audio. Older phone apps cannot.
    var duplex = false
    /// The client's current mode; only meaningful when `duplex`.
    var mode = StreamMode()
}

/// One connected sink. Owns its own send queue so a phone that stalls cannot
/// slow down another phone, or the capture callback.
final class ClientSession {
    private let connection: NWConnection
    private let queue: DispatchQueue
    private let token: String
    private let format: StreamFormat
    private let serverName: String

    private var handshakeBuffer = Data()
    private var readBuffer = Data()
    private(set) var authorised = false

    private var pending: [Data] = []
    private var sending = false
    private let maxQueued: Int

    private var seq: UInt32 = 0
    private var pingSeq: UInt32 = 0
    private var upExpectedSeq: UInt32?
    private(set) var upLost = 0
    private(set) var upFormat: (rate: Int, channels: Int)?
    private var pingTimer: DispatchSourceTimer?

    private(set) var snapshot: ClientSnapshot

    var onActivity: ((ActivityEntry.Kind, String) -> Void)?
    var onBytesSent: ((Int) -> Void)?
    var onReady: ((ClientSession) -> Void)?
    var onUpdate: ((ClientSession) -> Void)?
    var onClosed: ((ClientSession) -> Void)?
    /// The client reported a new mode (duplex only). Called on the server queue.
    var onModeChanged: ((ClientSession) -> Void)?
    /// Audio the client sent up. Called on the server queue.
    var onUpAudio: ((ClientSession, Data) -> Void)?
    /// The client refused a mode change asked for from this Mac.
    var onRefused: ((ClientSession, String) -> Void)?

    init(connection: NWConnection, queue: DispatchQueue, token: String,
         format: StreamFormat, serverName: String, maxQueued: Int = 24) {
        self.connection = connection
        self.queue = queue
        self.token = token
        self.format = format
        self.serverName = serverName
        self.maxQueued = maxQueued
        self.snapshot = ClientSnapshot(
            id: ObjectIdentifier(connection), name: "?",
            peer: Self.describe(connection.endpoint),
            rttMs: 0, dropped: 0, bufferMs: 0, underruns: 0, lost: 0, speed: 1,
            connectedAt: Date()
        )
    }

    private static func describe(_ endpoint: NWEndpoint) -> String {
        if case let .hostPort(host, port) = endpoint {
            return "\(host):\(port)"
        }
        return "\(endpoint)"
    }

    func start() {
        connection.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            switch state {
            case .failed, .cancelled:
                self.teardown()
            default:
                break
            }
        }
        connection.start(queue: queue)
        readHandshake()
        // Logged before the handshake on purpose: this line is the proof that
        // the phone's packets are reaching this machine at all.
        onActivity?(.arrived, "Connection from \(snapshot.peer)")

        // A connection that never says hello is not a sink; drop it rather
        // than hold a slot open for it.
        queue.asyncAfter(deadline: .now() + 5) { [weak self] in
            guard let self, !self.authorised else { return }
            NSLog("[net] %@: no HELLO within 5s, dropping", self.snapshot.peer)
            self.onActivity?(.silent, "\(self.snapshot.peer) connected but never sent a handshake")
            self.close()
        }
    }

    // MARK: handshake

    private func readHandshake() {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 4096) { [weak self] data, _, isComplete, error in
            guard let self else { return }
            if let data, !data.isEmpty {
                self.handshakeBuffer.append(data)
                if self.handshakeBuffer.count > 8192 {
                    self.reject("hello too large"); return
                }
                if let newline = self.handshakeBuffer.firstIndex(of: 0x0A) {
                    let line = self.handshakeBuffer[..<newline]
                    self.handshakeBuffer.removeSubrange(...newline)
                    self.handleHello(Data(line))
                    return
                }
            }
            if isComplete || error != nil { self.teardown(); return }
            self.readHandshake()
        }
    }

    private func handleHello(_ line: Data) {
        guard let hello = try? JSONDecoder().decode(Hello.self, from: line) else {
            onActivity?(.rejected, "\(snapshot.peer) sent something that is not a Tethertone handshake")
            reject("malformed hello"); return
        }
        guard hello.v == Proto.version else {
            onActivity?(.rejected, "\(snapshot.peer) speaks protocol v\(hello.v), this server speaks v\(Proto.version)")
            reject("unsupported version \(hello.v)"); return
        }
        guard Pairing.tokenMatches(hello.token, token) else {
            NSLog("[net] %@: REJECTED (bad token)", snapshot.peer)
            onActivity?(.rejected,
                        "\(hello.name) was turned away — its token does not match. Re-scan the QR.")
            reject("invalid token"); return
        }

        authorised = true
        snapshot.name = String(hello.name.prefix(48))
        snapshot.connectedAt = Date()
        snapshot.duplex = hello.caps?.contains(Proto.capDuplex) ?? false

        let welcome = Welcome(v: Proto.version, ok: true, rate: format.sampleRate,
                              ch: format.channels, fmt: "pcm_s16le",
                              block: format.blockFrames, name: serverName,
                              caps: [Proto.capDuplex])
        guard var payload = try? JSONEncoder().encode(welcome) else {
            reject("internal error"); return
        }
        payload.append(0x0A)
        connection.send(content: payload, completion: .contentProcessed { _ in })

        NSLog("[net] %@ connected as %@", snapshot.peer, snapshot.name)
        onActivity?(.connected, "\(snapshot.name) connected from \(snapshot.peer)")
        startPings()
        readFrames()
        onReady?(self)
    }

    private func reject(_ reason: String) {
        let welcome = Welcome.reject(reason)
        var payload = (try? JSONEncoder().encode(welcome)) ?? Data()
        payload.append(0x0A)
        connection.send(content: payload, completion: .contentProcessed { [weak self] _ in
            self?.close()
        })
    }

    // MARK: audio out

    /// Called from the capture thread. Drop-oldest: latency is worth more than
    /// completeness, and a backlog we cannot send is latency we cannot recover.
    func enqueue(_ pcm: Data) {
        queue.async { [weak self] in
            guard let self, self.authorised else { return }
            // The client asked for nothing down: keep only the pings flowing.
            if self.snapshot.duplex && !self.snapshot.mode.down { return }
            if self.pending.count >= self.maxQueued {
                self.pending.removeFirst()
                self.snapshot.dropped += 1
            }
            self.pending.append(pcm)
            self.pump()
        }
    }

    private func pump() {
        guard !sending, !pending.isEmpty else { return }
        let payload = pending.removeFirst()
        sending = true
        let packet = frame(type: Proto.audio, payload: payload, seq: seq)
        seq &+= 1
        let byteCount = packet.count
        connection.send(content: packet, completion: .contentProcessed { [weak self] error in
            guard let self else { return }
            self.sending = false
            if error != nil { self.teardown(); return }
            self.onBytesSent?(byteCount)
            self.pump()
        })
    }

    private func startPings() {
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + 1, repeating: 1)
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            self.pingSeq &+= 1
            let payload = putU64BE(monotonicMicros())
            let packet = frame(type: Proto.ping, payload: payload, seq: self.pingSeq)
            self.connection.send(content: packet, completion: .contentProcessed { _ in })
        }
        timer.resume()
        pingTimer = timer
    }

    // MARK: frames in

    private func readFrames() {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 16384) { [weak self] data, _, isComplete, error in
            guard let self else { return }
            if let data, !data.isEmpty {
                self.readBuffer.append(data)
                self.drainFrames()
            }
            if isComplete || error != nil { self.teardown(); return }
            self.readFrames()
        }
    }

    /// Walks the buffer with an explicit cursor and compacts once at the end.
    ///
    /// The previous version indexed `Data` with absolute offsets and called
    /// `removeSubrange` per frame. That relies on `Data` always being 0-based —
    /// which a sliced `Data` is not — and it re-copied the whole buffer for
    /// every frame parsed.
    private func drainFrames() {
        var cursor = readBuffer.startIndex
        let end = readBuffer.endIndex

        while end - cursor >= Proto.headerSize {
            let headerBytes = readBuffer[cursor..<(cursor + Proto.headerSize)]
            guard let header = decodeHeader(Data(headerBytes)) else { break }
            let total = Proto.headerSize + header.length
            guard end - cursor >= total else { break }

            let payloadStart = cursor + Proto.headerSize
            let payload = Data(readBuffer[payloadStart..<(cursor + total)])
            cursor += total
            handle(header: header, payload: payload)
        }

        readBuffer = cursor == end ? Data() : Data(readBuffer[cursor..<end])
    }

    private func handle(header: FrameHeader, payload: Data) {
        switch header.type {
        case Proto.pong where payload.count == 8:
            var sent: UInt64 = 0
            for byte in payload { sent = (sent << 8) | UInt64(byte) }
            snapshot.rttMs = Double(monotonicMicros() &- sent) / 1000.0
            onUpdate?(self)
        case Proto.stats:
            if let stats = try? JSONDecoder().decode(ClientStats.self, from: payload) {
                snapshot.bufferMs = stats.bufMs
                snapshot.underruns = stats.under
                snapshot.lost = stats.lost
                snapshot.speed = stats.speed
                onUpdate?(self)
            }
        case Proto.bye:
            teardown()
        case Proto.control where snapshot.duplex:
            handleControl(payload)
        case Proto.audio where snapshot.duplex && snapshot.mode.up != .off:
            if let expected = upExpectedSeq, header.seq != expected {
                upLost += Int(header.seq &- expected)
            }
            upExpectedSeq = header.seq &+ 1
            onUpAudio?(self, payload)
        default:
            break      // unknown types are ignored (PROTOCOL.md §2)
        }
    }

    private func handleControl(_ payload: Data) {
        guard let message = try? JSONDecoder().decode(ControlMessage.self, from: payload) else { return }
        switch message.t {
        case ControlMessage.Kind.mode:
            let mode = message.streamMode
            if mode.up == .off {
                upFormat = nil
            } else {
                upFormat = (message.upRate ?? 48000, message.upCh ?? 1)
            }
            if mode.up != snapshot.mode.up { upExpectedSeq = nil; upLost = 0 }
            snapshot.mode = mode
            onModeChanged?(self)
            onUpdate?(self)
        case ControlMessage.Kind.refused:
            onRefused?(self, message.reason ?? "The phone declined.")
        default:
            break
        }
    }

    /// Queued behind any audio already being sent, so frames never interleave.
    func sendControl(_ message: ControlMessage) {
        queue.async { [weak self] in
            guard let self, self.authorised, self.snapshot.duplex,
                  let payload = try? JSONEncoder().encode(message) else { return }
            let packet = frame(type: Proto.control, payload: payload, seq: 0)
            self.connection.send(content: packet, completion: .contentProcessed { _ in })
        }
    }

    // MARK: lifecycle

    func close() {
        queue.async { [weak self] in self?.teardown() }
    }

    private var torndown = false

    private func teardown() {
        guard !torndown else { return }
        torndown = true
        pingTimer?.cancel()
        pingTimer = nil
        pending.removeAll()
        connection.cancel()
        if authorised {
            NSLog("[net] %@ disconnected", snapshot.peer)
            onActivity?(.disconnected, "\(snapshot.name) disconnected")
        }
        onClosed?(self)
    }
}
