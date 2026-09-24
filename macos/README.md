# Tethertone for macOS

The server half of Tethertone: a native SwiftUI app that captures what the Mac
is playing and serves it to the Android app over USB and Wi-Fi.

```bash
./make-signing-identity.sh   # once per machine
./build.sh --install         # build and copy into /Applications
```

Requires macOS 14 or later. Builds with the Command Line Tools alone; Xcode is
optional. See [../docs/BUILDING.md](../docs/BUILDING.md) for every option.

## The app

**Menu bar icon** shows status and holds only the essentials: Start/Stop, the
current capture method, the last error, **Open Tethertone…** and Quit. The icon
fills in while a phone is receiving audio.

**Window** has everything else:

| Pane | Contents |
|---|---|
| Overview | capture method switch, **direction** switch, stats, level meter, activity log, connected devices |
| Audio | capture method and input device, **Hear it on this Mac too** monitor |
| Pairing | QR code, addresses, large-QR window, copy link, token rotation |
| Network | USB route, packet size (5/10/20 ms), remote hostname, port |
| General | launch at login, menu-bar-only mode, about |

Closing the window does not stop the server. **General › Menu bar only** hides
the Dock icon at runtime; no reinstall needed.

## Capture methods

| | Screen Recording | BlackHole |
|---|---|---|
| Code | `Audio/SystemAudioCapture.swift` | `Audio/DeviceCapture.swift` |
| API | ScreenCaptureKit | AVAudioEngine on a chosen input device |
| Permission | Screen & System Audio Recording | Microphone |
| Needs installing | nothing | [BlackHole](https://github.com/ExistentialAudio/BlackHole) |

Both deliver fixed 480-frame blocks (10 ms at 48 kHz) of 16-bit stereo PCM, at
48 kHz for Screen Recording and at the device's own rate for BlackHole. Switching while
running restarts capture on the other method; connected phones reconnect
automatically.

## Command line

```bash
Tethertone.app/Contents/MacOS/Tethertone --headless   # server only, prints the pairing URI
Tethertone.app/Contents/MacOS/Tethertone --selftest   # protocol conformance checks
```

## Distribution

`./package.sh` builds a universal `.dmg` and `.pkg` into `../dist/`. The `.pkg`
installs to /Applications, quits a running copy first, and relaunches the app
as the logged-in user.

Release builds are signed with a self-signed identity. That keeps macOS
permissions stable across updates, but the app is not notarized, so Gatekeeper
asks users to confirm the first launch. With an Apple Developer ID:

```bash
SIGNING_IDENTITY="Developer ID Application: Name (TEAMID)" \
INSTALLER_IDENTITY="Developer ID Installer: Name (TEAMID)" ./package.sh
```

After that, notarize with `xcrun notarytool` and staple the ticket.

## Source layout

```
Package.swift
build.sh / package.sh          build, bundle, sign / dmg + pkg
make-signing-identity.sh       stable self-signed code-signing identity
Tethertone.entitlements       hardened-runtime microphone entitlement
icon.py                        regenerates Tethertone.icns (needs Pillow)
installer/                     pkg scripts and installer pages
Sources/TethertoneMac/
  Entry.swift                  @main: app, --headless or --selftest
  SelfTest.swift               protocol vectors shared with the Kotlin tests
  Audio/
    AudioSource.swift          the capture protocol and Core Audio device lists
    SystemAudioCapture.swift   ScreenCaptureKit
    DeviceCapture.swift        AVAudioEngine
    MonitorPlayback.swift      optional local playback
  Net/
    BridgeServer.swift         NWListener and fan-out to clients
    ClientSession.swift        one client: handshake, send queue, ping, telemetry
    ADB.swift                  adb discovery and `adb reverse`
  Pairing/                     token, addresses, pairing URI, QR, routes
  Protocol/                    frame codec and handshake messages
  Model/                       ServerModel (app state), activity log, login item
  UI/                          app scenes, window panes, menu bar, pairing window
```
