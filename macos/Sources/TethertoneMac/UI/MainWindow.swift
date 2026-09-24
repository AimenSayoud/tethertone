import CoreAudio
import SwiftUI

enum AppSection: String, CaseIterable, Identifiable {
    case overview, audio, pairing, network, general

    var id: String { rawValue }

    var title: String {
        switch self {
        case .overview: return "Overview"
        case .audio: return "Audio"
        case .pairing: return "Pairing"
        case .network: return "Network"
        case .general: return "General"
        }
    }

    var symbol: String {
        switch self {
        case .overview: return "waveform"
        case .audio: return "speaker.wave.2"
        case .pairing: return "qrcode"
        case .network: return "network"
        case .general: return "gearshape"
        }
    }
}

struct MainWindow: View {
    @ObservedObject var model: ServerModel
    @State private var section: AppSection = .overview

    var body: some View {
        NavigationSplitView {
            List(AppSection.allCases, selection: $section) { item in
                Label(item.title, systemImage: item.symbol).tag(item)
            }
            .navigationSplitViewColumnWidth(min: 170, ideal: 190, max: 230)
            .safeAreaInset(edge: .bottom) { sidebarFooter }
        } detail: {
            Group {
                switch section {
                case .overview: OverviewPane(model: model)
                case .audio: AudioPane(model: model)
                case .pairing: PairingPane(model: model)
                case .network: NetworkPane(model: model)
                case .general: GeneralPane(model: model)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    Button(model.running ? "Stop" : "Start") {
                        model.running ? model.stop() : model.startAfterAuthorisation()
                    }
                }
            }
        }
        .frame(minWidth: 760, minHeight: 520)
    }

    private var sidebarFooter: some View {
        HStack(spacing: 7) {
            Circle()
                .fill(model.running ? (model.clients.isEmpty ? Color.orange : Color.green) : Color.secondary)
                .frame(width: 8, height: 8)
            Text(model.running ? (model.clients.isEmpty ? "Waiting" : "Streaming") : "Stopped")
                .font(.caption)
                .foregroundStyle(.secondary)
            Spacer()
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(.bar)
    }
}

// MARK: - shared building blocks

struct Pane<Content: View>: View {
    let title: String
    let subtitle: String
    @ViewBuilder var content: Content

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(title).font(.title2).fontWeight(.semibold)
                    Text(subtitle).font(.callout).foregroundStyle(.secondary)
                }
                content
            }
            .padding(28)
            .frame(maxWidth: 620, alignment: .leading)
        }
    }
}

struct Card<Content: View>: View {
    var title: String? = nil
    var footnote: String? = nil
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let title {
                Text(title).font(.headline)
            }
            content
            if let footnote {
                Text(footnote)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(18)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.quaternary.opacity(0.35), in: RoundedRectangle(cornerRadius: 12))
    }
}

// MARK: - overview

struct OverviewPane: View {
    @ObservedObject var model: ServerModel

    var body: some View {
        Pane(title: "Overview", subtitle: model.status) {
            CaptureMethodCard(model: model)
            DirectionCard(model: model)

            Card {
                HStack(spacing: 20) {
                    stat(model.running ? "\(model.clients.count)" : "—", "devices")
                    Divider().frame(height: 34)
                    stat(rateLabel, "rate")
                    Divider().frame(height: 34)
                    stat(uptime, "uptime")
                    Divider().frame(height: 34)
                    stat(String(format: "%.0f MB", Double(model.bytesSent) / 1_048_576), "sent")
                    Spacer()
                }
            }

            Card(title: "Output level",
                 footnote: model.running ? "Peak of what is being sent, in dBFS." :
                    "Start the server to see the audio it is capturing.") {
                LevelMeter(level: model.level)
            }

            if let error = model.lastError {
                Card(title: "Problem") {
                    Text(error).foregroundStyle(.red).fixedSize(horizontal: false, vertical: true)
                    if error.localizedCaseInsensitiveContains("microphone") {
                        Button("Open Microphone settings") { PrivacySettings.open(.microphone) }
                    } else if error.localizedCaseInsensitiveContains("screen") {
                        Button("Open Screen Recording settings") { PrivacySettings.open(.screenRecording) }
                    }
                }
            }

            ActivityCard(model: model)

            Card(title: "Connected devices",
                 footnote: "ppm is the clock-drift correction the phone is applying to hold its buffer steady.") {
                if model.clients.isEmpty {
                    Text(model.running ? "Nothing connected yet — scan the QR on the Pairing tab."
                                       : "The server is stopped.")
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(model.clients) { client in
                        HStack(alignment: .top) {
                            Image(systemName: "iphone.gen3").foregroundStyle(.tint)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(client.name).fontWeight(.medium)
                                Text(detail(for: client))
                                    .font(.system(.caption, design: .monospaced))
                                    .foregroundStyle(.secondary)
                            }
                            Spacer()
                            Button("Disconnect") { model.disconnect(client) }
                                .buttonStyle(.link)
                        }
                        .padding(.vertical, 3)
                    }
                }
            }
        }
    }

    private func stat(_ value: String, _ label: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(value).font(.system(.title3, design: .rounded)).fontWeight(.semibold)
            Text(label).font(.caption).foregroundStyle(.secondary)
        }
    }

    private var rateLabel: String {
        guard model.running, let rate = model.pairing?.rate else { return "—" }
        return "\(rate / 1000) kHz"
    }

    private var uptime: String {
        guard let since = model.startedAt else { return "—" }
        let seconds = Int(Date().timeIntervalSince(since))
        if seconds < 60 { return "\(seconds)s" }
        if seconds < 3600 { return "\(seconds / 60)m" }
        return "\(seconds / 3600)h"
    }

    private func detail(for client: ClientSnapshot) -> String {
        var parts = [String(format: "rtt %.0f ms", client.rttMs)]
        if client.bufferMs > 0 { parts.append("buffer \(client.bufferMs) ms") }
        if client.underruns > 0 { parts.append("under \(client.underruns)") }
        if client.lost > 0 { parts.append("lost \(client.lost)") }
        if client.dropped > 0 { parts.append("dropped \(client.dropped)") }
        if abs(client.speed - 1.0) > 0.00005 {
            parts.append(String(format: "%+.0f ppm", (client.speed - 1.0) * 1_000_000))
        }
        return parts.joined(separator: "   ")
    }
}

/// The answer to "is my phone even reaching this Mac?".
struct ActivityCard: View {
    @ObservedObject var model: ServerModel

    var body: some View {
        Card(title: "Activity",
             footnote: "If your phone says it is retrying and nothing appears here, its packets are "
                     + "not reaching this Mac — the two devices are probably not on the same network, "
                     + "or the network blocks device-to-device traffic. Use the USB cable instead.") {
            if model.activity.isEmpty {
                Text("Nothing yet.").foregroundStyle(.secondary)
            } else {
                VStack(alignment: .leading, spacing: 5) {
                    ForEach(model.activity.prefix(8)) { entry in
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Image(systemName: entry.kind.symbol)
                                .foregroundStyle(colour(for: entry.kind))
                                .font(.caption)
                                .frame(width: 14)
                            Text(entry.timestamp)
                                .font(.system(.caption, design: .monospaced))
                                .foregroundStyle(.tertiary)
                            Text(entry.text)
                                .font(.caption)
                                .fixedSize(horizontal: false, vertical: true)
                            Spacer(minLength: 0)
                        }
                    }
                }
                if model.activity.count > 8 {
                    Button("Clear") { model.clearActivity() }.buttonStyle(.link).font(.caption)
                }
            }
        }
    }

    private func colour(for kind: ActivityEntry.Kind) -> Color {
        switch kind {
        case .connected: return .green
        case .rejected: return .red
        case .silent: return .orange
        case .arrived: return .blue
        case .disconnected, .info: return .secondary
        }
    }
}

// MARK: - capture method

/// The one choice that decides what macOS asks for: Screen Recording
/// (ScreenCaptureKit) or Microphone (AVAudioEngine on BlackHole). Only one
/// runs at a time; switching while running restarts on the other.
struct CaptureMethodCard: View {
    @ObservedObject var model: ServerModel
    var showsDeviceDetails = false

    var body: some View {
        Card(title: "Capture method") {
            Picker("", selection: $model.sourceKind) {
                ForEach(CaptureSourceKind.allCases) { kind in
                    Label(kind.title, systemImage: kind.symbol).tag(kind)
                }
            }
            .labelsHidden()
            .pickerStyle(.segmented)

            Text(model.sourceKind.summary)
                .font(.caption).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            if model.sourceKind == .device {
                Picker("Input device", selection: Binding(
                    get: { model.selectedDeviceID ?? model.effectiveDevice?.id ?? 0 },
                    set: { model.selectedDeviceID = $0 }
                )) {
                    ForEach(model.inputDevices) { device in
                        Text("\(device.name)  ·  \(device.sampleRate / 1000) kHz").tag(device.id)
                    }
                }
                if showsDeviceDetails {
                    Button("Refresh devices") { model.refreshDevices() }
                        .buttonStyle(.link)
                }
                if !model.inputDevices.contains(where: \.isBlackHole) {
                    Text("BlackHole is not installed:  brew install blackhole-2ch")
                        .font(.system(.caption, design: .monospaced))
                        .foregroundStyle(.orange)
                }
            }

            if model.permissionReady {
                Label("\(model.permissionName) allowed", systemImage: "checkmark.circle.fill")
                    .font(.caption).foregroundStyle(.green)
            } else {
                VStack(alignment: .leading, spacing: 6) {
                    Label("macOS is not allowing \(model.permissionName) for Tethertone.",
                          systemImage: "exclamationmark.triangle.fill")
                        .font(.caption).foregroundStyle(.orange)
                    if showsDeviceDetails {
                        Text("If Tethertone already appears switched on in that list, switch it off and "
                             + "on again. macOS ties the permission to the app's signature, so updating "
                             + "the app invalidates the grant while still showing it as allowed.")
                            .font(.caption).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Button("Open \(model.permissionName) settings") { model.openPermissionSettings() }
                }
            }
        }
    }
}

// MARK: - direction (two-way audio)

/// Which way audio flows, as the user thinks of it.
enum Direction: String, CaseIterable, Identifiable {
    case toPhone, toMac, both

    var id: String { rawValue }

    var title: String {
        switch self {
        case .toPhone: return "To phone"
        case .toMac: return "To Mac"
        case .both: return "Both"
        }
    }

    init(_ mode: StreamMode) {
        if mode.up == .off { self = .toPhone } else if !mode.down { self = .toMac } else { self = .both }
    }

    func mode(source: UpSource) -> StreamMode {
        let up = source == .off ? UpSource.mic : source
        switch self {
        case .toPhone: return StreamMode(down: true, up: .off)
        case .toMac: return StreamMode(down: false, up: up)
        case .both: return StreamMode(down: true, up: up)
        }
    }
}

/// Direction per connected phone, plus where the phone's audio plays on this Mac.
///
/// The phone owns the mode — it holds the microphone and the capture consent —
/// so every control here is a request. The phone's answer comes back as its
/// new mode, or as a reason it could not, e.g. that its screen is needed to
/// grant a permission.
struct DirectionCard: View {
    @ObservedObject var model: ServerModel

    var body: some View {
        Card(title: "Direction",
             footnote: "Sending the phone's microphone while it also plays the Mac's audio works best "
                     + "with headphones on the phone; otherwise its speaker is heard by its own microphone.") {
            if model.clients.isEmpty {
                Text("Connect a phone to choose which way the audio goes.")
                    .foregroundStyle(.secondary)
            }
            ForEach(model.clients) { client in
                if client.duplex {
                    phoneControls(client)
                } else {
                    Label("\(client.name) has an older Tethertone. Update it on the phone for two-way audio.",
                          systemImage: "arrow.down.circle")
                        .font(.caption).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            if model.clients.contains(where: \.duplex) {
                Divider()
                destination
            }

            if let warning = model.upLoopWarning {
                Label(warning, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption).foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            } else if let problem = model.upProblem {
                Label(problem, systemImage: "exclamationmark.circle")
                    .font(.caption).foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    @ViewBuilder
    private func phoneControls(_ client: ClientSnapshot) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            if model.clients.count > 1 {
                Text(client.name).fontWeight(.medium)
            }
            Picker("Direction", selection: Binding(
                get: { Direction(client.mode) },
                set: { model.requestMode($0.mode(source: client.mode.up), for: client) }
            )) {
                ForEach(Direction.allCases) { Text($0.title).tag($0) }
            }
            .labelsHidden()
            .pickerStyle(.segmented)

            if client.mode.up != .off {
                Picker("From the phone", selection: Binding(
                    get: { client.mode.up },
                    set: { model.requestMode(StreamMode(down: client.mode.down, up: $0), for: client) }
                )) {
                    Text("Microphone").tag(UpSource.mic)
                    Text("Its playing audio").tag(UpSource.playback)
                }
            }
        }
    }

    @ViewBuilder
    private var destination: some View {
        Picker("Phone audio plays on", selection: Binding(
            get: { model.upOutputDeviceID.map(Int.init) ?? -1 },
            set: { model.upOutputDeviceID = $0 >= 0 ? AudioDeviceID($0) : nil }
        )) {
            Text("System output").tag(-1)
            ForEach(model.upOutputDevices) { device in Text(device.name).tag(Int(device.id)) }
        }
        Text("To use the phone as a microphone in other apps, choose BlackHole here and pick BlackHole "
             + "as the microphone in those apps.")
            .font(.caption).foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)

        if let up = model.upStream {
            VStack(alignment: .leading, spacing: 6) {
                Label("Receiving from \(up.phoneName) · \(up.channels == 1 ? "mono" : "stereo") \(up.sampleRate / 1000) kHz",
                      systemImage: "waveform")
                    .font(.caption).foregroundStyle(.green)
                LevelMeter(level: model.upLevel)
                TimelineView(.periodic(from: .now, by: 1)) { _ in
                    Text(String(format: "buffer %d ms · underruns %d · %+.0f ppm",
                                model.upBufferMs, model.upUnderruns, (model.upSpeed - 1) * 1_000_000))
                        .font(.system(.caption, design: .monospaced))
                        .foregroundStyle(.secondary)
                }
                HStack {
                    Image(systemName: "speaker.fill").foregroundStyle(.secondary)
                    Slider(value: $model.upVolume, in: 0...1)
                    Image(systemName: "speaker.wave.3.fill").foregroundStyle(.secondary)
                }
            }
        }
    }
}

// MARK: - audio

struct AudioPane: View {
    @ObservedObject var model: ServerModel

    var body: some View {
        Pane(title: "Audio", subtitle: "Where the sound comes from, and whether you hear it here too.") {
            CaptureMethodCard(model: model, showsDeviceDetails: true)
            DirectionCard(model: model)

            if model.sourceKind == .device {
                Card(title: "Hear it on this Mac too",
                     footnote: "Routing your output into BlackHole silences the Mac's own speakers. "
                             + "This plays a copy back so you do not have to build a Multi-Output Device "
                             + "in Audio MIDI Setup.") {
                    Toggle("Play a copy through my speakers", isOn: $model.monitorEnabled)
                    if model.monitorEnabled {
                        Picker("Output", selection: Binding(
                            get: { model.monitorDeviceID ?? model.outputDevices.first?.id ?? 0 },
                            set: { model.monitorDeviceID = $0 }
                        )) {
                            ForEach(model.outputDevices) { device in Text(device.name).tag(device.id) }
                        }
                        HStack {
                            Image(systemName: "speaker.fill").foregroundStyle(.secondary)
                            Slider(value: $model.monitorVolume, in: 0...1)
                            Image(systemName: "speaker.wave.3.fill").foregroundStyle(.secondary)
                            Text("\(Int(model.monitorVolume * 100))%")
                                .font(.caption).monospacedDigit()
                                .frame(width: 42, alignment: .trailing)
                        }
                    }
                }
            } else {
                Card(title: "Hear it on this Mac too") {
                    Text("Not needed with Screen Recording — it taps the mix rather than redirecting it, "
                         + "so your speakers keep working on their own.")
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }
}

// MARK: - pairing

struct PairingPane: View {
    @ObservedObject var model: ServerModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Pane(title: "Pairing", subtitle: "Scan this with the Tethertone app on your phone.") {
            Card {
                HStack(alignment: .top, spacing: 24) {
                    if let image = model.qrImage {
                        Image(nsImage: image)
                            .interpolation(.none)      // hard module edges, or it will not scan
                            .resizable()
                            .frame(width: 210, height: 210)
                            .padding(12)
                            .background(Color.white)
                            .clipShape(RoundedRectangle(cornerRadius: 10))
                    } else {
                        RoundedRectangle(cornerRadius: 10)
                            .fill(.quaternary)
                            .frame(width: 234, height: 234)
                            .overlay(Text("Start the server").foregroundStyle(.secondary))
                    }

                    VStack(alignment: .leading, spacing: 12) {
                        if let pairing = model.pairing {
                            labelled2("Addresses", pairing.hosts.joined(separator: "\n"))
                            labelled2("Port", "\(pairing.port)")
                            labelled2("Format", "\(pairing.rate) Hz · \(pairing.channels) ch")
                            if model.usbAttached {
                                Label("USB path live", systemImage: "cable.connector")
                                    .font(.caption).foregroundStyle(.green)
                            }
                        } else {
                            Text("No pairing details until the server is running.")
                                .foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button("Copy pairing link") { model.copyPairingURI() }
                        Button("Show large QR") { openWindow(id: "pairing") }
                            .disabled(model.qrImage == nil)
                    }
                }
            }

            RoutesCard(model: model)

            Card(title: "Security",
                 footnote: "The token authenticates the phone; it does not encrypt the audio. "
                         + "On a network you do not trust, use the USB path.") {
                Text("Every phone that has paired is trusted until the token changes.")
                    .foregroundStyle(.secondary)
                Button("Generate a new token") { model.rotateToken() }
                Text("This immediately invalidates every phone already paired.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private func labelled2(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(label.uppercased()).font(.system(size: 9, weight: .semibold)).foregroundStyle(.tertiary)
            Text(value).font(.system(.callout, design: .monospaced))
        }
    }
}

/// The three ways in, and which are actually on offer right now.
///
/// Almost every failed connection comes down to the user assuming a route that
/// is not available — the phone is on mobile data, or the cable is not in — so
/// this says plainly which ones the pairing code contains.
struct RoutesCard: View {
    @ObservedObject var model: ServerModel

    var body: some View {
        let offered = model.offeredRoutes
        let inUse = model.routesInUse

        Card(title: "Ways in",
             footnote: "The phone tries every address in the code at once and keeps whichever answers "
                     + "first, so the cable wins when it is plugged in.") {
            VStack(alignment: .leading, spacing: 10) {
                ForEach(TransportRoute.allCases) { route in
                    let active = inUse.contains(route)
                    let available = offered.contains(route)
                    HStack(alignment: .top, spacing: 10) {
                        Image(systemName: route.symbol)
                            .foregroundStyle(active ? Color.green : (available ? Color.primary : Color.secondary.opacity(0.55)))
                            .frame(width: 18)
                        VStack(alignment: .leading, spacing: 1) {
                            HStack(spacing: 6) {
                                Text(route.label)
                                    .fontWeight(active ? .semibold : .regular)
                                    .foregroundStyle(available ? Color.primary : Color.secondary)
                                Text(active ? "in use" : (available ? "offered" : "not available"))
                                    .font(.caption2)
                                    .padding(.horizontal, 6).padding(.vertical, 1)
                                    .background(
                                        (active ? Color.green : Color.secondary).opacity(active ? 0.22 : 0.12),
                                        in: Capsule()
                                    )
                                    .foregroundStyle(active ? Color.green : Color.secondary)
                            }
                            Text(available ? route.detail : route.missingAdvice)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        Spacer(minLength: 0)
                    }
                }
            }
        }
    }
}

// MARK: - network

struct NetworkPane: View {
    @ObservedObject var model: ServerModel

    var body: some View {
        Pane(title: "Network", subtitle: "How the audio gets to the phone.") {
            Card(title: "USB",
                 footnote: "adb reverse makes the phone's own localhost reach this Mac. It is lower "
                         + "latency than Wi-Fi and immune to congestion, and the phone races both, "
                         + "so the cable wins whenever it is plugged in.") {
                Toggle("Offer the USB path when a phone is plugged in", isOn: $model.usbEnabled)
                    .disabled(ADB.executable == nil)
                if ADB.executable == nil {
                    Text("adb was not found. Install it with:  brew install android-platform-tools")
                        .font(.system(.caption, design: .monospaced))
                        .foregroundStyle(.secondary)
                } else if model.usbAttached {
                    Label("A phone is attached and the tunnel is up", systemImage: "checkmark.circle.fill")
                        .font(.caption).foregroundStyle(.green)
                }
            }

            Card(title: "Packet size",
                 footnote: "Smaller packets mean lower latency and more of them. Raise this if the "
                         + "phone reports underruns on a congested network.") {
                Picker("", selection: $model.blockMilliseconds) {
                    Text("5 ms — tightest").tag(5)
                    Text("10 ms — default").tag(10)
                    Text("20 ms — most forgiving").tag(20)
                }
                .labelsHidden()
                .pickerStyle(.radioGroup)
            }

            Card(title: "Reaching this Mac from anywhere",
                 footnote: "Your Mac is behind NAT and a phone on mobile data is behind carrier NAT, "
                         + "so neither can dial the other directly over the internet. A mesh VPN like "
                         + "Tailscale gives both a stable address that works anywhere — install it and "
                         + "its address appears in the QR automatically. Otherwise put a tunnel "
                         + "hostname here (ngrok, cloudflared) and it will be offered to the phone.") {
                HStack {
                    TextField("host or hostname (optional)", text: $model.extraHost)
                        .textFieldStyle(.roundedBorder)
                    if !model.extraHost.isEmpty {
                        Button("Clear") { model.extraHost = "" }.buttonStyle(.link)
                    }
                }
                Text("Audio is not encrypted. Exposing this port publicly means anyone holding the "
                     + "token can listen — prefer a VPN over an open tunnel.")
                    .font(.caption).foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Card(title: "Port", footnote: "Change this only if something else already uses 45678.") {
                HStack {
                    TextField("", value: Binding(
                        get: { Int(model.port) },
                        set: { model.port = UInt16(max(1, min(65535, $0))) }
                    ), format: .number.grouping(.never))
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 100)
                    Spacer()
                }
            }
        }
    }
}

// MARK: - general

struct GeneralPane: View {
    @ObservedObject var model: ServerModel

    var body: some View {
        Pane(title: "General", subtitle: "How Tethertone behaves on this Mac.") {
            Card(title: "Startup", footnote: LoginItem.explanation) {
                Toggle("Launch at login", isOn: $model.loginItemEnabled)
                Text("The server also starts on its own if it was running when you last quit.")
                    .font(.caption).foregroundStyle(.secondary)
            }

            Card(title: "Appearance",
                 footnote: "In menu-bar-only mode there is no Dock icon. Reopen this window from the "
                         + "menu bar icon.") {
                Toggle("Menu bar only (hide the Dock icon)", isOn: $model.menuBarOnly)
            }

            Card(title: "About") {
                HStack(spacing: 14) {
                    Image(nsImage: NSApp.applicationIconImage)
                        .resizable().frame(width: 52, height: 52)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Tethertone").font(.headline)
                        Text("Version \(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "dev")").font(.caption).foregroundStyle(.secondary)
                        Text("Your Mac's audio, on your phone, over USB or Wi-Fi.")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
            }
        }
    }
}
