import AppKit
import Combine
import AVFoundation
import CoreAudio
import Foundation

/// Everything the app is doing, in one observable place: capture, server,
/// pairing and the numbers the menu shows.
///
/// Not `@MainActor` — the same object runs the headless mode, which has no
/// SwiftUI and no main actor to speak of. Instead every `@Published` mutation
/// is funnelled onto the main queue explicitly.
final class ServerModel: ObservableObject {

    @Published private(set) var running = false
    @Published private(set) var clients: [ClientSnapshot] = []
    @Published private(set) var level: Float = 0
    @Published private(set) var status = "Stopped"
    @Published private(set) var lastError: String?
    @Published private(set) var pairing: PairingInfo?
    @Published private(set) var qrImage: NSImage?
    @Published private(set) var inputDevices: [AudioInputDevice] = []
    @Published private(set) var usbAttached = false
    @Published private(set) var outputDevices: [AudioInputDevice] = []
    @Published private(set) var startedAt: Date?
    @Published private(set) var bytesSent: Int64 = 0
    @Published private(set) var activity: [ActivityEntry] = []

    // MARK: audio from the phone (duplex)

    /// Set while a phone is sending audio up: who, and in what format.
    @Published private(set) var upStream: UpStreamInfo?
    @Published private(set) var upLevel: Float = 0
    /// Output devices the phone's audio can go to, loopback devices included.
    @Published private(set) var upOutputDevices: [AudioInputDevice] = []
    /// The last thing that went wrong with the phone's audio, e.g. a declined request.
    @Published private(set) var upProblem: String?
    /// Set while the current settings would loop audio; up streams are refused meanwhile.
    @Published private(set) var upLoopWarning: String?
    /// nil plays through the system's default output.
    @Published var upOutputDeviceID: AudioDeviceID? {
        didSet {
            guard oldValue != upOutputDeviceID else { return }
            defaults.set(upOutputDeviceID.map(Int.init) ?? -1, forKey: Keys.upDevice)
            updateUpGate()
            restartUpstreamPlayer()
        }
    }
    @Published var upVolume: Double {
        didSet {
            defaults.set(upVolume, forKey: Keys.upVolume)
            upPlayerBox.player?.volume = Float(upVolume)
        }
    }
    @Published var loginItemEnabled: Bool = LoginItem.isEnabled {
        didSet {
            guard oldValue != loginItemEnabled else { return }
            do { try LoginItem.setEnabled(loginItemEnabled) }
            catch {
                loginItemEnabled = LoginItem.isEnabled
                report(error: "Could not change the login item: \(error.localizedDescription)")
            }
        }
    }

    @Published var monitorEnabled: Bool {
        didSet {
            guard oldValue != monitorEnabled else { return }
            defaults.set(monitorEnabled, forKey: Keys.monitor)
            if running { restart() }
        }
    }
    @Published var monitorDeviceID: AudioDeviceID? {
        didSet {
            guard oldValue != monitorDeviceID else { return }
            defaults.set(monitorDeviceID.map(Int.init) ?? -1, forKey: Keys.monitorDevice)
            if running && monitorEnabled { restart() }
        }
    }
    @Published var monitorVolume: Double {
        didSet {
            defaults.set(monitorVolume, forKey: Keys.monitorVolume)
            monitor?.volume = Float(monitorVolume)
        }
    }
    /// Menu-bar-only hides the Dock icon. Kept as a runtime activation policy
    /// rather than LSUIElement in Info.plist, so it can be switched without
    /// reinstalling the app.
    /// An address the app cannot discover for itself: a Tailscale name, an
    /// ngrok or cloudflared hostname, a DNS name pointing at a forwarded port.
    /// It is added to the pairing code so the phone races it alongside the
    /// local ones and uses it when nothing else answers.
    @Published var extraHost: String {
        didSet {
            guard oldValue != extraHost else { return }
            defaults.set(extraHost, forKey: Keys.extraHost)
            rebuildPairing(format: capture?.format ?? StreamFormat())
        }
    }
    @Published var menuBarOnly: Bool {
        didSet {
            guard oldValue != menuBarOnly else { return }
            defaults.set(menuBarOnly, forKey: Keys.menuBarOnly)
            applyActivationPolicy()
        }
    }
    @Published var blockMilliseconds: Int {
        didSet {
            guard oldValue != blockMilliseconds else { return }
            defaults.set(blockMilliseconds, forKey: Keys.blockMs)
            if running { restart() }
        }
    }

    @Published var selectedDeviceID: AudioDeviceID? {
        didSet {
            guard oldValue != selectedDeviceID else { return }
            defaults.set(selectedDeviceID.map(Int.init) ?? -1, forKey: Keys.device)
            if running { restart() }
        }
    }
    @Published var port: UInt16 {
        didSet {
            guard oldValue != port else { return }
            defaults.set(Int(port), forKey: Keys.port)
            if running { restart() }
        }
    }
    @Published var sourceKind: CaptureSourceKind {
        didSet {
            guard oldValue != sourceKind else { return }
            defaults.set(sourceKind.rawValue, forKey: Keys.source)
            if running { restart() }
        }
    }
    @Published var usbEnabled: Bool {
        didSet {
            guard oldValue != usbEnabled else { return }
            defaults.set(usbEnabled, forKey: Keys.usb)
            if running { restart() }
        }
    }

    private enum Keys {
        static let port = "port"
        static let device = "inputDevice"
        static let usb = "usbEnabled"
        static let source = "captureSource"
        static let wasRunning = "wasRunning"
        static let monitor = "monitorEnabled"
        static let monitorDevice = "monitorDevice"
        static let monitorVolume = "monitorVolume"
        static let blockMs = "blockMilliseconds"
        static let menuBarOnly = "menuBarOnly"
        static let extraHost = "extraHost"
        static let upDevice = "upOutputDevice"
        static let upVolume = "upVolume"
    }

    private let defaults = UserDefaults.standard
    private var server: BridgeServer?
    private var capture: AudioSource?
    private var monitor: MonitorPlayback?
    private var token: String
    private var reversedSerial: String?

    // The audio thread produces peaks ~100x a second. SwiftUI does not need
    // anywhere near that, and publishing at that rate is pure main-thread churn.
    private var pendingLevel: Float = 0
    private var pendingUpLevel: Float = 0
    /// Read from the server queue on every up-stream packet, replaced from main.
    private let upPlayerBox = UpPlayerBox()
    /// Headless testing: a synthetic tone instead of real capture (`--test-tone`).
    var useTestTone = false
    private var levelTimer: Timer?
    private var usbWatchTimer: Timer?
    private let adbQueue = DispatchQueue(label: "dev.tethertone.adb")

    init() {
        token = Pairing.loadToken()
        defaults.register(defaults: [
            Keys.port: 45678, Keys.usb: true, Keys.device: -1,
            // System capture works with nothing installed, so it is the
            // default; BlackHole is for people who prefer to avoid the
            // Screen Recording permission.
            Keys.source: CaptureSourceKind.system.rawValue,
            Keys.monitor: false, Keys.monitorDevice: -1, Keys.monitorVolume: 1.0,
            Keys.blockMs: 10, Keys.menuBarOnly: false,
            Keys.upDevice: -1, Keys.upVolume: 1.0,
        ])
        let storedUp = defaults.integer(forKey: Keys.upDevice)
        upOutputDeviceID = storedUp >= 0 ? AudioDeviceID(storedUp) : nil
        upVolume = defaults.double(forKey: Keys.upVolume)
        menuBarOnly = defaults.bool(forKey: Keys.menuBarOnly)
        extraHost = defaults.string(forKey: Keys.extraHost) ?? ""
        monitorEnabled = defaults.bool(forKey: Keys.monitor)
        let storedMonitor = defaults.integer(forKey: Keys.monitorDevice)
        monitorDeviceID = storedMonitor >= 0 ? AudioDeviceID(storedMonitor) : nil
        monitorVolume = defaults.double(forKey: Keys.monitorVolume)
        blockMilliseconds = defaults.integer(forKey: Keys.blockMs)
        sourceKind = CaptureSourceKind(rawValue: defaults.string(forKey: Keys.source) ?? "")
            ?? .system
        // UInt16(_:) traps on anything outside 0...65535, and `defaults write
        // … -int 99999` is enough to make the app crash on launch.
        let storedPort = defaults.integer(forKey: Keys.port)
        port = (1...65535).contains(storedPort) ? UInt16(storedPort) : 45678
        usbEnabled = defaults.bool(forKey: Keys.usb)
        let stored = defaults.integer(forKey: Keys.device)
        selectedDeviceID = stored >= 0 ? AudioDeviceID(stored) : nil
        refreshDevices()
    }

    // MARK: devices

    func refreshDevices() {
        let devices = AudioDevices.inputDevices()
        let outputs = AudioOutputs.devices()
        let upOutputs = AudioOutputs.devices(includeLoopback: true)
        onMain {
            self.outputDevices = outputs
            self.upOutputDevices = upOutputs
            if let current = self.upOutputDeviceID, !upOutputs.contains(where: { $0.id == current }) {
                self.upOutputDeviceID = nil
            }
            self.inputDevices = devices
            if let current = self.selectedDeviceID, !devices.contains(where: { $0.id == current }) {
                self.selectedDeviceID = nil
            }
        }
    }

    var sourceDescription: String {
        switch sourceKind {
        case .system: return "System audio"
        case .device: return effectiveDevice?.name ?? "no device"
        }
    }

    var effectiveDevice: AudioInputDevice? {
        if let id = selectedDeviceID, let match = inputDevices.first(where: { $0.id == id }) { return match }
        return AudioDevices.preferredInput()
    }

    // MARK: lifecycle

    /// BlackHole is an *input* device, so capturing it needs microphone
    /// permission like any other. Asking up front turns a silent stream into a
    /// system prompt the user can actually answer. System capture asks for a
    /// different permission, and ScreenCaptureKit raises that prompt itself.
    func startAfterAuthorisation() {
        guard sourceKind == .device else { start(); return }
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized:
            start()
        case .notDetermined:
            AVCaptureDevice.requestAccess(for: .audio) { [weak self] granted in
                DispatchQueue.main.async {
                    if granted { self?.start() }
                    else { self?.report(error: "Microphone access denied — it is needed to read BlackHole.") }
                }
            }
        default:
            report(error: "Microphone access is off. Enable it in System Settings › Privacy & Security › Microphone.")
        }
    }

    func start() {
        guard !running else { return }

        let source: AudioSource
        if useTestTone {
            source = TestToneSource()
        } else {
        switch sourceKind {
        case .system:
            source = SystemAudioCapture()
        case .device:
            guard let device = effectiveDevice else {
                report(error: "No audio input device found. Install BlackHole, or pick a device.")
                return
            }
            source = DeviceCapture(deviceID: device.id, blockFrames: blockFrames(for: device.sampleRate))
        }
        }

        let server = BridgeServer(token: token, serverName: Pairing.machineName, format: source.format)

        // Monitoring only makes sense for the device path: system capture taps
        // the mix instead of redirecting it, so the speakers never went quiet.
        let wantsMonitor = monitorEnabled && sourceKind == .device
        let monitor = wantsMonitor ? MonitorPlayback(format: source.format) : nil
        monitor?.volume = Float(monitorVolume)

        source.onBlock = { [weak server] block in
            server?.broadcast(block)
            monitor?.push(block)
        }
        source.onLevel = { [weak self] peak in self?.pendingLevel = max(self?.pendingLevel ?? 0, peak) }
        // ScreenCaptureKit finishes starting long after start() returns, so a
        // permission refusal arrives here rather than as a thrown error.
        // Hop to main before tearing down. ScreenCaptureKit reports a refusal
        // from its own task, often while start() is still running here — and a
        // stop() racing a start() left the adb tunnel up with no server behind
        // it, which looks exactly like a working setup that silently does not.
        source.onFailure = { [weak self] error in
            DispatchQueue.main.async {
                self?.report(error: error.localizedDescription)
                self?.log(.rejected, "Capture stopped: \(error.localizedDescription)")
                self?.stop()
            }
        }

        server.onClientsChanged = { [weak self] snapshots in
            self?.clients = snapshots
            self?.updateStatus()
        }
        server.onActivity = { [weak self] kind, text in self?.log(kind, text) }
        server.onUpStreamStarted = { [weak self] rate, channels, name in
            self?.startUpstream(rate: rate, channels: channels, from: name)
        }
        server.onUpStreamStopped = { [weak self] in self?.stopUpstream() }
        server.onUpAudio = { [weak self] data in self?.upPlayerBox.player?.push(data) }
        server.onRefused = { [weak self] name, reason in
            self?.log(.info, "\(name) declined: \(reason)")
            self?.onMain { self?.upProblem = "\(name): \(reason)" }
        }
        server.onListenerFailed = { [weak self] error in
            self?.report(error: "Port \(self?.port ?? 0) is unavailable: \(error.localizedDescription)")
            self?.stop()
        }

        do {
            try source.start()
            // The capture device decides the real rate; the server must
            // advertise that, not what was guessed before it opened.
            server.format = source.format
            try server.start(port: port)
            if let monitor {
                do { try monitor.start(deviceID: monitorDeviceID) }
                catch {
                    // A monitor that will not open is not a reason to refuse to
                    // stream — the phone is the point, the speakers are a bonus.
                    report(error: "Monitoring is off: \(error.localizedDescription)")
                }
            }
        } catch {
            source.stop()
            monitor?.stop()
            report(error: error.localizedDescription)
            return
        }

        self.capture = source
        self.server = server
        self.monitor = monitor
        updateUpGate()
        configureUSB()
        rebuildPairing(format: source.format)
        startLevelTimer()
        startUSBWatch()

        defaults.set(true, forKey: Keys.wasRunning)
        log(.info, "Listening on port \(port) · \(sourceDescription) · "
            + "\(source.format.sampleRate) Hz \(source.format.channels)ch")
        if reversedSerial == nil {
            log(.info, "No phone on USB — pairing is Wi-Fi only, so the phone must be on the same network")
        }
        onMain {
            self.startedAt = Date()
            self.bytesSent = 0
            self.running = true
            self.lastError = nil
            self.updateStatus()
        }
    }

    /// Called once at launch. A menu-bar app that silently does nothing until
    /// it is opened is not much of a background app — but on the very first
    /// run nothing is started, so the permission prompt arrives as a result of
    /// the user pressing Start rather than out of nowhere.
    func startIfPreviouslyRunning() {
        guard defaults.bool(forKey: Keys.wasRunning) else { return }
        startAfterAuthorisation()
    }

    func stop() {
        defaults.set(false, forKey: Keys.wasRunning)
        if running { log(.info, "Server stopped") }
        levelTimer?.invalidate()
        levelTimer = nil
        usbWatchTimer?.invalidate()
        usbWatchTimer = nil
        capture?.stop()
        capture = nil
        monitor?.stop()
        monitor = nil
        stopUpstream()
        server?.stop()
        server = nil
        if let serial = reversedSerial {
            ADB.removeReverse(port: port, serial: serial)
            reversedSerial = nil
        }
        onMain {
            self.running = false
            self.startedAt = nil
            self.usbAttached = false
            self.level = 0
            self.clients = []
            self.updateStatus()
        }
    }

    /// Restart must not clear the "was running" flag — it is a reconfiguration,
    /// not the user switching the server off.
    func restart() {
        let wasRunning = running
        stop()
        start()
        if wasRunning { defaults.set(true, forKey: Keys.wasRunning) }
    }

    func rotateToken() {
        token = Pairing.loadToken(rotate: true)
        if running { restart() } else { rebuildPairing(format: capture?.format ?? StreamFormat()) }
    }

    func log(_ kind: ActivityEntry.Kind, _ text: String) {
        onMain {
            self.activity.insert(ActivityEntry(kind: kind, text: text), at: 0)
            if self.activity.count > 60 { self.activity.removeLast() }
        }
    }

    func clearActivity() {
        onMain { self.activity.removeAll() }
    }

    func applyActivationPolicy() {
        NSApp.setActivationPolicy(menuBarOnly ? .accessory : .regular)
        if !menuBarOnly { NSApp.activate(ignoringOtherApps: true) }
    }

    func disconnect(_ client: ClientSnapshot) {
        server?.disconnect(client.id)
    }

    // MARK: two-way audio

    /// Ask a phone to change direction. The phone applies it (its mode update
    /// then arrives in `clients`) or declines, which lands in `upProblem`.
    func requestMode(_ mode: StreamMode, for client: ClientSnapshot) {
        guard mode.isValid else { return }
        upProblem = nil
        server?.requestMode(client.id, mode)
    }

    /// The loop this Mac can see: playing the phone's audio into the very
    /// device being captured sends it straight back to the phone.
    private var upLoopReason: String? {
        guard sourceKind == .device, let captured = effectiveDevice else { return nil }
        let target = upOutputDeviceID ?? AudioOutputs.defaultOutput()
        guard target == captured.id else { return nil }
        return "Playing the phone's audio into \(captured.name), the device being captured, would send it "
            + "straight back to the phone. Choose another output under “Audio from the phone”."
    }

    private func updateUpGate() {
        let reason = upLoopReason
        server?.blockUpStreams(reason)
        onMain { self.upLoopWarning = reason }
    }

    private func startUpstream(rate: Int, channels: Int, from name: String) {
        upPlayerBox.player?.stop()
        let player = UpstreamPlayer(sampleRate: rate, channels: channels)
        player.volume = Float(upVolume)
        player.onLevel = { [weak self] peak in
            guard let self else { return }
            self.pendingUpLevel = max(self.pendingUpLevel, peak)
        }
        do {
            try player.start(deviceID: upOutputDeviceID)
        } catch {
            report(error: "Could not play the phone's audio: \(error.localizedDescription)")
            return
        }
        upPlayerBox.player = player
        let info = UpStreamInfo(phoneName: name, sampleRate: rate, channels: channels)
        log(.connected, "\(name) is sending audio (\(channels == 1 ? "mono" : "stereo"), \(rate / 1000) kHz)")
        onMain {
            self.upStream = info
            self.upProblem = nil
        }
    }

    private func stopUpstream() {
        let wasPlaying = upPlayerBox.player != nil
        upPlayerBox.player?.stop()
        upPlayerBox.player = nil
        if wasPlaying, let name = upStream?.phoneName { log(.info, "\(name) stopped sending audio") }
        onMain {
            self.upStream = nil
            self.upLevel = 0
        }
    }

    /// A new output device takes effect by reopening the player; the phone
    /// keeps streaming meanwhile.
    private func restartUpstreamPlayer() {
        guard let info = upStream else { return }
        startUpstream(rate: info.sampleRate, channels: info.channels, from: info.phoneName)
    }

    /// Live numbers for the window, read on demand.
    var upBufferMs: Int { upPlayerBox.player?.bufferedMs ?? 0 }
    var upUnderruns: Int { upPlayerBox.player?.underruns ?? 0 }
    var upSpeed: Double { upPlayerBox.player?.currentSpeed ?? 1 }

    private func blockFrames(for sampleRate: Int) -> Int {
        max(64, sampleRate * blockMilliseconds / 1000)
    }

    func copyPairingURI() {
        guard let uri = pairing?.uri else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(uri, forType: .string)
    }

    // MARK: pairing

    private func configureUSB() {
        reversedSerial = nil
        guard usbEnabled, ADB.executable != nil else { return }
        guard let serial = ADB.attachedDevices().first else { return }
        if ADB.reverse(port: port, serial: serial) {
            reversedSerial = serial
            NSLog("[adb] %@: reverse tcp:%d ready", serial, Int(port))
        }
    }

    /// Live permission state for the selected source, for the UI to show
    /// before the user presses Start and discovers it the hard way.
    var permissionReady: Bool {
        switch sourceKind {
        case .system: return SystemAudioCapture.isAuthorised
        case .device: return AVCaptureDevice.authorizationStatus(for: .audio) == .authorized
        }
    }

    var permissionName: String {
        sourceKind == .system ? "Screen & System Audio Recording" : "Microphone"
    }

    func openPermissionSettings() {
        PrivacySettings.open(sourceKind == .system ? .screenRecording : .microphone)
    }

    /// True when a cable is the only thing that will work right now.
    var wifiOnly: Bool { running && reversedSerial == nil }

    /// Routes the current pairing code actually offers the phone.
    var offeredRoutes: Set<TransportRoute> {
        Set((pairing?.hosts ?? []).map(classifyHost))
    }

    /// The route each connected phone is actually using.
    var routesInUse: Set<TransportRoute> {
        Set(clients.map { classifyHost(String($0.peer.split(separator: ":").first ?? "")) })
    }

    private func rebuildPairing(format: StreamFormat) {
        // Loopback first when the cable is live: the client races the hosts in
        // order and a small head start is enough for USB to win.
        var hosts: [String] = []
        if reversedSerial != nil { hosts.append("127.0.0.1") }
        hosts.append(contentsOf: Pairing.localIPv4Addresses().filter { !hosts.contains($0) })
        // Last, because it is the slowest route when the local ones work.
        let extra = extraHost.trimmingCharacters(in: .whitespacesAndNewlines)
        if !extra.isEmpty, !hosts.contains(extra) { hosts.append(extra) }

        let info = PairingInfo(hosts: hosts, port: Int(port), token: token,
                               rate: format.sampleRate, channels: format.channels,
                               name: Pairing.machineName)
        let image = QRCode.image(for: info.uri)
        onMain {
            self.pairing = info
            self.qrImage = image
            self.usbAttached = self.reversedSerial != nil
        }
    }

    // MARK: plumbing

    /// Plugging the cable in after pressing Start used to do nothing until the
    /// server was restarted, because the USB tunnel was only ever set up once.
    /// The pairing code has to change when the cable appears, so this watches
    /// for it — off the main thread, since every adb call spawns a process.
    private func startUSBWatch() {
        usbWatchTimer?.invalidate()
        guard usbEnabled, ADB.executable != nil else { return }
        let timer = Timer(timeInterval: 4, repeats: true) { [weak self] _ in
            guard let self, self.running else { return }
            self.adbQueue.async {
                let attached = ADB.attachedDevices().first
                guard attached != self.reversedSerial else { return }
                if attached == nil {
                    self.reversedSerial = nil
                    self.log(.info, "USB phone unplugged — pairing is Wi-Fi only again")
                } else {
                    self.configureUSB()
                    if self.reversedSerial != nil {
                        self.log(.info, "USB phone detected — re-scan the QR to use the cable")
                    }
                }
                self.rebuildPairing(format: self.capture?.format ?? StreamFormat())
            }
        }
        RunLoop.main.add(timer, forMode: .common)
        usbWatchTimer = timer
    }

    private func startLevelTimer() {
        levelTimer?.invalidate()
        let timer = Timer(timeInterval: 1.0 / 20.0, repeats: true) { [weak self] _ in
            guard let self else { return }
            let peak = self.pendingLevel
            self.pendingLevel = 0
            // Real bytes written to sockets. With nobody connected this stays
            // at zero, which is what the number should say.
            if let total = self.server?.totalBytesSent, total != self.bytesSent {
                self.bytesSent = total
            }
            // Decay, so a peak stays readable instead of flickering out.
            self.level = peak >= self.level ? peak : self.level * 0.7
            let up = self.pendingUpLevel
            self.pendingUpLevel = 0
            self.upLevel = up >= self.upLevel ? up : self.upLevel * 0.7
        }
        RunLoop.main.add(timer, forMode: .common)
        levelTimer = timer
    }

    private func updateStatus() {
        let count = clients.count
        if !running {
            status = lastError == nil ? "Stopped" : "Stopped — \(lastError!)"
        } else if count == 0 {
            status = "Waiting for a phone"
        } else {
            status = count == 1 ? "Streaming to 1 device" : "Streaming to \(count) devices"
        }
    }

    private func report(error: String) {
        NSLog("[error] %@", error)
        onMain {
            self.lastError = error
            self.updateStatus()
        }
    }

    private func onMain(_ work: @escaping () -> Void) {
        if Thread.isMainThread { work() } else { DispatchQueue.main.async(execute: work) }
    }
}

/// Who is sending audio up, and in what format.
struct UpStreamInfo: Equatable {
    let phoneName: String
    let sampleRate: Int
    let channels: Int
}

/// The current up-stream player, swapped on the main queue and read on the
/// server queue for every packet.
final class UpPlayerBox {
    private let lock = NSLock()
    private var _player: UpstreamPlayer?

    var player: UpstreamPlayer? {
        get { lock.lock(); defer { lock.unlock() }; return _player }
        set { lock.lock(); _player = newValue; lock.unlock() }
    }
}
