<div align="center">

# Tethertone

**Your Mac's audio on your Android phone, and your phone's microphone on your Mac — over USB or Wi-Fi, paired with one QR scan.**

[Website](https://aimensayoud.github.io/tethertone-website/) ·
[Download](https://aimensayoud.github.io/tethertone-website/download/) ·
[Setup guide](https://aimensayoud.github.io/tethertone-website/guide/) ·
[How it works](https://aimensayoud.github.io/tethertone-website/how-it-works/) ·
[Compare](https://aimensayoud.github.io/tethertone-website/compare/)

[![CI](https://github.com/AimenSayoud/tethertone/actions/workflows/ci.yml/badge.svg)](https://github.com/AimenSayoud/tethertone/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/AimenSayoud/tethertone)](https://github.com/AimenSayoud/tethertone/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![macOS 14+](https://img.shields.io/badge/macOS-14%2B-black)
![Android 8+](https://img.shields.io/badge/Android-8.0%2B-3DDC84)

</div>

Tethertone streams whatever your Mac is playing to an Android phone as
uncompressed 16-bit stereo PCM, typically at 48 kHz. The Mac runs a small native menu-bar app; the
phone runs a native Kotlin app that plays the stream through `AudioTrack`, with
a jitter buffer and continuous clock-drift correction so latency stays flat for
hours instead of creeping up.

```
Mac apps ──► system audio mix ──► Tethertone.app ──┬── USB  (adb reverse) ──┐
             (ScreenCaptureKit or BlackHole)         └── Wi-Fi (TCP)        ──┴─► Tethertone for Android ──► speaker / headphones
```

## Features

- **Both directions** — the phone can send its microphone, or the audio its
  other apps play, back to the Mac. Switch *Mac → phone*, *Phone → Mac* or
  *Both ways* live from either device. Route it into BlackHole to use the phone
  as a microphone in Zoom, Discord or OBS.
- **Two capture methods** — read the system mix directly with ScreenCaptureKit
  (nothing to install), or read a [BlackHole](https://github.com/ExistentialAudio/BlackHole)
  loopback device with AVAudioEngine (Microphone permission only).
- **USB and Wi-Fi at once** — the QR code lists every route; the phone races
  them and keeps whichever answers first, so the cable wins when it is plugged in.
- **Low, stable latency** — uncompressed PCM, 10 ms packets, a 90 ms default
  buffer (20–250 ms adjustable), and drift correction that never glitches.
- **One-scan pairing** — a token in the QR authenticates the phone; pairing is
  remembered and the phone reconnects on its own.
- **Background playback** — a foreground service keeps audio going with the
  screen off.
- **Live diagnostics** — round-trip time, buffer depth, underruns, lost packets
  and drift correction in ppm, on both the Mac and the phone.
- **Hear it on the Mac too** — optional local monitor, so routing output into
  BlackHole does not silence your speakers.
- **Small and native** — a ~2 MB universal SwiftUI app on the Mac, no Electron and no JVM.

## Install

Download the latest files from **[Releases](https://github.com/AimenSayoud/tethertone/releases/latest)**:

| Platform | File | Requirements |
|---|---|---|
| macOS | `Tethertone-<version>-macos.dmg` (or `.pkg`) | macOS 14 Sonoma or later, Apple Silicon or Intel |
| Android | `Tethertone-<version>-android.apk` | Android 8.0 or later |

### macOS

1. Open the `.dmg` and drag **Tethertone** into **Applications**.
2. The app is not notarized by Apple, so the first launch is blocked. Open
   **System Settings › Privacy & Security**, scroll down and click **Open Anyway**
   — or run once in Terminal:
   ```bash
   xattr -dr com.apple.quarantine /Applications/Tethertone.app
   ```
3. Launch Tethertone and press **Start**. macOS asks for one permission, depending
   on the capture method (see [Capture methods](#capture-methods)).

### Android

1. Download the `.apk` on your phone and open it. Allow installs from your
   browser or file manager when Android asks.
2. Open Tethertone and tap **Scan pairing QR**.

For the USB route you also need
[USB debugging](https://developer.android.com/studio/debug/dev-options#enable)
enabled on the phone and `adb` on the Mac (`brew install android-platform-tools`).
Wi-Fi needs nothing extra.

## Usage

1. On the Mac, open Tethertone and press **Start** (menu bar icon or window).
2. On the phone, tap **Scan pairing QR** and point it at the code on the Mac's
   **Pairing** tab.
3. Play anything on the Mac.

That is the whole setup. Next time, press Start on the Mac and the phone
reconnects by itself.

The **menu bar icon** holds only Start/Stop, status and a button to open the
window. The **window** has everything else: the QR code, capture method,
connected devices, network options and settings.

## Capture methods

Choose in the window under **Overview › Capture method**.

| | Screen Recording | BlackHole |
|---|---|---|
| API | ScreenCaptureKit | AVAudioEngine |
| Install anything? | No | [BlackHole 2ch](https://github.com/ExistentialAudio/BlackHole) (`brew install blackhole-2ch`) |
| Permission | Screen & System Audio Recording | Microphone |
| Menu bar indicator | purple screen-recording dot while running | orange microphone dot while running |
| Mac speakers | keep working | silenced unless you enable **Hear it on this Mac too** or use a Multi-Output Device |

The Screen Recording method never reads a pixel — ScreenCaptureKit simply has
no audio-only mode, so macOS files it under screen recording.

To use BlackHole: install it, set **BlackHole 2ch** as the Mac's sound output
(or as part of a Multi-Output Device), choose **BlackHole** in Tethertone and
select it as the input device.

## Documentation

| | |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | How it works: capture, transport, jitter buffer, drift correction |
| [docs/PROTOCOL.md](docs/PROTOCOL.md) | The wire protocol, normative for both sides |
| [docs/BUILDING.md](docs/BUILDING.md) | Building both apps from source, tests, releasing |
| [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) | Permissions, connection problems, remote access |
| [docs/FDROID.md](docs/FDROID.md) | Publishing on F-Droid: store metadata and the submission draft |
| [macos/README.md](macos/README.md) | The macOS app |
| [android/README.md](android/README.md) | The Android app and the shared Kotlin module |

## Build from source

```bash
git clone https://github.com/AimenSayoud/tethertone.git && cd tethertone

# macOS app — needs only the Xcode Command Line Tools
cd macos && ./make-signing-identity.sh && ./build.sh --install

# Android app — needs JDK 17 and the Android SDK
cd ../android && ./gradlew :androidApp:installDebug
```

See [docs/BUILDING.md](docs/BUILDING.md) for details.

## Security

The pairing token keeps other people on your network from listening in, but
**the audio itself is not encrypted**. On a network you do not trust, use the
USB route. See [SECURITY.md](SECURITY.md).

## Repository layout

```
macos/        SwiftUI menu-bar app: capture, TCP server, QR pairing, adb setup
android/
  shared/     Kotlin Multiplatform: protocol, pairing, jitter buffer, client state machine
  androidApp/ Compose UI, QR scanner, foreground playback service
  desktopSink/ headless JVM client for testing without a phone
docs/         architecture, protocol, building, troubleshooting
tools/        probe.py — independent protocol conformance client
```

## Contributing

Bug reports, fixes and ideas are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE) © Tethertone contributors
