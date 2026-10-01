import AppKit
import Foundation

/// `--headless` runs the server with no UI (useful over SSH, and for testing
/// against tools/probe.py); add `--test-tone` to stream a synthetic tone that
/// needs no capture permission. `--selftest` runs the protocol conformance checks.
@main
enum Entry {
    static func main() {
        if CommandLine.arguments.contains("--selftest") {
            SelfTest.run()
        }
        if CommandLine.arguments.contains("--headless") {
            Headless.run()
        } else {
            TethertoneApp.main()
        }
    }
}

enum Headless {
    static func run() {
        let model = ServerModel()
        model.useTestTone = CommandLine.arguments.contains("--test-tone")
        model.start()

        guard model.running, let pairing = model.pairing else {
            FileHandle.standardError.write(Data("failed to start\n".utf8))
            exit(1)
        }

        print("[audio] \(model.effectiveDevice?.name ?? "?") @ \(pairing.rate) Hz, \(pairing.channels)ch")
        print("[net]   reachable at: \(pairing.hosts.joined(separator: ", "))")
        print("[pair]  \(pairing.uri)")
        print("[server] ready — Ctrl+C to stop.")

        for signalNumber in [SIGINT, SIGTERM] {
            signal(signalNumber) { _ in
                print("\n[server] shutting down")
                exit(0)
            }
        }
        RunLoop.main.run()
    }
}
