import SwiftUI

/// The menu bar popover: only what you reach for without opening the window.
/// Capture method, QR, devices and settings all live in the main window.
struct MenuContent: View {
    @ObservedObject var model: ServerModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Tethertone").font(.headline)
                    Text(model.status).font(.caption).foregroundStyle(.secondary)
                    Label(model.sourceKind.title, systemImage: model.sourceKind.symbol)
                        .font(.caption2).foregroundStyle(.tertiary)
                    if let up = model.upStream {
                        Label("Receiving from \(up.phoneName)", systemImage: "arrow.up.circle")
                            .font(.caption2).foregroundStyle(.green)
                    }
                }
                Spacer()
                Circle()
                    .fill(model.running ? (model.clients.isEmpty ? Color.orange : Color.green) : Color.secondary)
                    .frame(width: 9, height: 9)
                    .padding(.top, 5)
            }

            if let error = model.lastError {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .lineLimit(3)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Button {
                model.running ? model.stop() : model.startAfterAuthorisation()
            } label: {
                Label(model.running ? "Stop" : "Start",
                      systemImage: model.running ? "stop.fill" : "play.fill")
                    .frame(maxWidth: .infinity)
            }
            .controlSize(.large)
            .keyboardShortcut(.defaultAction)

            Button {
                showMainWindow()
            } label: {
                Label("Open Tethertone…", systemImage: "macwindow")
                    .frame(maxWidth: .infinity)
            }
            .help("QR code, capture method, devices and settings")

            Divider()

            Button("Quit Tethertone") { NSApplication.shared.terminate(nil) }
                .buttonStyle(.link)
                .font(.caption)
        }
        .padding(16)
        .frame(width: 260)
    }

    /// In menu-bar-only mode the app has no Dock icon, so it must become a
    /// regular app again before a window can take focus.
    private func showMainWindow() {
        if NSApp.activationPolicy() != .regular {
            NSApp.setActivationPolicy(.regular)
        }
        openWindow(id: "main")
        NSApp.activate(ignoringOtherApps: true)
    }
}

/// Peak meter in decibels — a linear one reads as dead for ordinary music,
/// which peaks around -18 dBFS.
struct LevelMeter: View {
    let level: Float
    private let floorDB: Float = -60

    private var fraction: CGFloat {
        guard level > 0.0001 else { return 0 }
        let db = max(floorDB, 20 * log10(level))
        return CGFloat((db - floorDB) / -floorDB)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            GeometryReader { geometry in
                ZStack(alignment: .leading) {
                    RoundedRectangle(cornerRadius: 3)
                        .fill(Color.secondary.opacity(0.18))
                    RoundedRectangle(cornerRadius: 3)
                        .fill(LinearGradient(colors: [.green, .green, .yellow, .red],
                                             startPoint: .leading, endPoint: .trailing))
                        .frame(width: max(0, geometry.size.width * fraction))
                }
            }
            .frame(height: 8)
            Text(level > 0.0001 ? String(format: "%.0f dBFS peak", 20 * log10(level)) : "silent")
                .font(.system(.caption2, design: .monospaced))
                .foregroundStyle(.secondary)
        }
    }
}
