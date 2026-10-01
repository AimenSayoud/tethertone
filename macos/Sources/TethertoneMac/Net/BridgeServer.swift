import Foundation
import Network

enum ServerError: LocalizedError {
    case invalidPort(UInt16)

    var errorDescription: String? {
        switch self {
        case .invalidPort(let port): return "Port \(port) is not usable; pick 1-65535."
        }
    }
}

/// TCP server for the Tethertone protocol. Capture pushes blocks in through
/// `broadcast`; each session owns its own bounded queue from there on.
final class BridgeServer {

    let queue = DispatchQueue(label: "dev.tethertone.server")
    private var listener: NWListener?
    private var sessions: [ObjectIdentifier: ClientSession] = [:]

    private(set) var port: UInt16 = 45678
    var format: StreamFormat
    let token: String
    let serverName: String

    /// Always delivered on the main queue — it drives the UI.
    var onClientsChanged: (([ClientSnapshot]) -> Void)?
    var onListenerFailed: ((Error) -> Void)?
    /// Delivered on the main queue.
    var onActivity: ((ActivityEntry.Kind, String) -> Void)?

    // MARK: up stream (duplex)

    /// A phone started sending audio up: (sample rate, channels, phone name). Main queue.
    var onUpStreamStarted: ((Int, Int, String) -> Void)?
    /// The up stream ended. Main queue.
    var onUpStreamStopped: (() -> Void)?
    /// Up-stream audio. Called on the server queue — the hot path, no hop.
    var onUpAudio: ((Data) -> Void)?
    /// A phone declined a mode change asked for from this Mac: (phone name, reason). Main queue.
    var onRefused: ((String, String) -> Void)?

    /// Only one phone's audio is played at a time (PROTOCOL.md §6).
    private var upOwner: ObjectIdentifier?
    private var upOwnerFormat: (rate: Int, channels: Int)?
    /// When set, every up stream is refused with this reason (e.g. it would loop).
    private var upBlockedReason: String?

    /// Bytes actually written to sockets, not bytes captured — with nobody
    /// connected this stays at zero, which is the honest answer.
    private(set) var totalBytesSent: Int64 = 0

    init(token: String, serverName: String, format: StreamFormat) {
        self.token = token
        self.serverName = serverName
        self.format = format
    }

    var clientCount: Int { sessions.values.filter(\.authorised).count }

    func start(port: UInt16) throws {
        stop()
        self.port = port

        let parameters = NWParameters.tcp
        parameters.allowLocalEndpointReuse = true
        if let tcp = parameters.defaultProtocolStack.internetProtocol as? NWProtocolTCP.Options {
            // Audio packets are small and frequent; waiting for Nagle to fill a
            // segment would add tens of milliseconds for no bandwidth gain.
            tcp.noDelay = true
        }

        guard let endpointPort = NWEndpoint.Port(rawValue: port) else {
            throw ServerError.invalidPort(port)
        }
        let listener = try NWListener(using: parameters, on: endpointPort)
        listener.newConnectionHandler = { [weak self] connection in
            self?.accept(connection)
        }
        // The bind result arrives asynchronously, so "listening" is only true
        // once the listener says .ready — announcing it at start() time reports
        // success for a port that is about to fail with EADDRINUSE.
        listener.stateUpdateHandler = { [weak self] state in
            guard let self else { return }
            switch state {
            case .ready:
                NSLog("[net] listening on 0.0.0.0:%d", Int(self.port))
            case .failed(let error):
                DispatchQueue.main.async { self.onListenerFailed?(error) }
            default:
                break
            }
        }
        listener.start(queue: queue)
        self.listener = listener
    }

    func stop() {
        queue.sync {
            for session in sessions.values { session.close() }
            sessions.removeAll()
        }
        listener?.cancel()
        listener = nil
        publish()
    }

    private func accept(_ connection: NWConnection) {
        let session = ClientSession(connection: connection, queue: queue, token: token,
                                    format: format, serverName: serverName)
        // Retained here, not in onReady: onReady only fires once the handshake
        // has succeeded, so waiting for it meant the session was deallocated
        // the moment this function returned and the hello was never answered.
        sessions[ObjectIdentifier(connection)] = session

        session.onActivity = { [weak self] kind, text in
            DispatchQueue.main.async { self?.onActivity?(kind, text) }
        }
        session.onBytesSent = { [weak self] count in
            self?.totalBytesSent += Int64(count)
        }
        session.onReady = { [weak self] _ in self?.publish() }
        session.onModeChanged = { [weak self] session in self?.modeChanged(session) }
        session.onUpAudio = { [weak self] session, data in
            guard let self, self.upOwner == session.snapshot.id else { return }
            self.onUpAudio?(data)
        }
        session.onRefused = { [weak self] session, reason in
            let name = session.snapshot.name
            DispatchQueue.main.async { self?.onRefused?(name, reason) }
        }
        session.onUpdate = { [weak self] _ in self?.publish() }
        session.onClosed = { [weak self] session in
            guard let self else { return }
            self.sessions.removeValue(forKey: ObjectIdentifier(connection))
            if self.upOwner == session.snapshot.id { self.releaseUpStream() }
            self.publish()
        }
        session.start()
    }

    /// Claims, keeps or releases the up stream as a client's mode changes.
    /// Runs on the server queue.
    private func modeChanged(_ session: ClientSession) {
        let id = session.snapshot.id
        let wantsUp = session.snapshot.mode.up != .off

        guard wantsUp else {
            if upOwner == id { releaseUpStream() }
            return
        }
        if let reason = upBlockedReason {
            session.sendControl(.upDenied(reason))
            return
        }
        if let owner = upOwner, owner != id {
            let other = sessions[owner]?.snapshot.name ?? "Another phone"
            session.sendControl(.upDenied("\(other) is already sending audio to this computer."))
            return
        }
        guard let format = session.upFormat else { return }
        // Same owner and format: nothing to restart.
        if upOwner == id, let current = upOwnerFormat,
           current.rate == format.rate, current.channels == format.channels { return }
        upOwner = id
        upOwnerFormat = format
        let name = session.snapshot.name
        DispatchQueue.main.async { [weak self] in self?.onUpStreamStarted?(format.rate, format.channels, name) }
    }

    private func releaseUpStream() {
        upOwner = nil
        upOwnerFormat = nil
        DispatchQueue.main.async { [weak self] in self?.onUpStreamStopped?() }
    }

    /// Ask a phone to change direction. The phone decides; its answer arrives
    /// as a mode update or through `onRefused`.
    func requestMode(_ id: ObjectIdentifier, _ mode: StreamMode) {
        queue.async { [weak self] in self?.sessions[id]?.sendControl(.setMode(mode)) }
    }

    /// Refuse all up streams (and stop the current one) while `reason` is set.
    func blockUpStreams(_ reason: String?) {
        queue.async { [weak self] in
            guard let self else { return }
            self.upBlockedReason = reason
            if let reason, let owner = self.upOwner {
                self.sessions[owner]?.sendControl(.upDenied(reason))
                self.releaseUpStream()
            }
        }
    }

    func disconnect(_ id: ObjectIdentifier) {
        queue.async { [weak self] in self?.sessions[id]?.close() }
    }

    /// Hot path: called once per captured block, from the audio thread.
    func broadcast(_ pcm: Data) {
        queue.async { [weak self] in
            guard let self else { return }
            for session in self.sessions.values { session.enqueue(pcm) }
        }
    }

    /// Only authorised sessions are anybody's business: a half-open connection
    /// that has not said hello yet is not a device the user has paired.
    private func publish() {
        let snapshots = sessions.values
            .filter(\.authorised)
            .map(\.snapshot)
            .sorted { $0.connectedAt < $1.connectedAt }
        DispatchQueue.main.async { [weak self] in self?.onClientsChanged?(snapshots) }
    }
}
