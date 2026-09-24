import Foundation

struct Hello: Codable {
    let v: Int
    let role: String?
    let token: String
    let name: String
    let caps: [String]?
}

struct Welcome: Codable {
    let v: Int
    let ok: Bool
    var rate: Int = 48000
    var ch: Int = 2
    var fmt: String = "pcm_s16le"
    var block: Int = 480
    var name: String = ""
    var err: String? = nil
    var caps: [String]? = nil

    static func reject(_ reason: String) -> Welcome {
        Welcome(v: Proto.version, ok: false, err: reason)
    }
}

/// Telemetry the sink reports back, shown per-client in the menu.
struct ClientStats: Codable {
    let bufMs: Int
    let under: Int
    let lost: Int
    let speed: Double
}

/// What a client sends up, if anything. Unknown values read as `.off`.
enum UpSource: String, Codable, CaseIterable, Identifiable {
    case off, mic, playback

    var id: String { rawValue }

    init(wire: String?) { self = UpSource(rawValue: wire ?? "") ?? .off }

    var title: String {
        switch self {
        case .off: return "Off"
        case .mic: return "Phone microphone"
        case .playback: return "Phone's audio"
        }
    }
}

/// Which way audio flows on one connection. Owned by the client (PROTOCOL.md §6).
struct StreamMode: Equatable {
    var down = true
    var up: UpSource = .off

    var isValid: Bool { down || up != .off }
}

/// Payload of a CONTROL frame. Only the fields a given `t` uses are set.
struct ControlMessage: Codable {
    var t: String
    var down: Bool? = nil
    var up: String? = nil
    var upRate: Int? = nil
    var upCh: Int? = nil
    var reason: String? = nil

    /// Values of `t`.
    enum Kind {
        static let mode = "mode", setMode = "setMode", refused = "refused", upDenied = "upDenied"
    }

    var streamMode: StreamMode { StreamMode(down: down ?? true, up: UpSource(wire: up)) }

    static func setMode(_ mode: StreamMode) -> ControlMessage {
        ControlMessage(t: Kind.setMode, down: mode.down, up: mode.up.rawValue)
    }

    static func upDenied(_ reason: String) -> ControlMessage {
        ControlMessage(t: Kind.upDenied, reason: reason)
    }
}
