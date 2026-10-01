# Tethertone for Windows and Linux

The computer side of Tethertone for Windows and Linux: streams what the
computer plays to the Tethertone Android app over USB or Wi-Fi, and plays the
phone's microphone or audio back. It speaks the same protocol as the macOS app,
so the phone can't tell them apart.

One Rust codebase, one small native binary per platform: no Electron, no JVM,
no runtime to install.

```bash
cargo run --release                         # the window
cargo run --release -- --headless           # server only: prints the pairing link and a QR code
cargo run --release -- --headless --test-tone   # synthetic tone, no audio devices (tests, CI)
cargo test                                  # protocol vectors, jitter buffer, server end to end
```

Needs a Rust toolchain (`rustup`). On Linux also the PulseAudio client headers
and the windowing libraries:

```bash
sudo apt install libpulse-dev libxkbcommon-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev
```

Release builds for both platforms are made by GitHub Actions
(`.github/workflows/release-desktop.yml`) when a version tag is pushed.

## What it does

| | Windows | Linux |
|---|---|---|
| Captures | everything the PC plays (WASAPI loopback of the default output) | the default sink's monitor (PulseAudio, or PipeWire through pipewire-pulse) |
| Plays the phone's audio | default output (WASAPI) | default output (PulseAudio/PipeWire) |
| USB route | `adb reverse`, automatic when a phone is plugged in | same |
| Tray | yes: Start/Stop, Open, Quit; closing the window keeps it running | window only for now |
| Launch at login | per-user Run key | XDG autostart entry |

The window shows the pairing QR, the connected phones with their direction
switch (*To phone · To computer · Both*, microphone or the phone's own audio),
the phone's audio with its buffer and drift correction, settings and an
activity log.

Settings live in `~/.tethertone/desktop.json`. The pairing token is shared with
the macOS app at `~/.tethertone/token`.

## Layout

```
core/                 no audio, no UI: tested on any OS
  src/protocol.rs     frames, handshake, CONTROL messages (docs/PROTOCOL.md)
  src/pairing.rs      token, tethertone:// link, address order
  src/jitter.rs       buffer + drift correction for the phone's audio
  src/server.rs       TCP server: auth, per-phone drop-oldest queues, duplex
  tests/              docs/test-vectors.json; the server end to end over localhost
app/
  src/engine.rs       capture → server → playback, USB watcher; shared by window and headless
  src/audio/          cpal (Windows), PulseAudio (Linux), test tone, null playback
  src/gui.rs          egui window and Windows tray
  src/adb.rs, net.rs, qr.rs, autostart.rs, icon.rs
```

## Known gaps

- **Phone as a microphone in other apps** needs a virtual input device. On
  Windows that means a signed driver (VB-CABLE, or the open-source
  Virtual-Audio-Driver), and the output device isn't selectable yet. On Linux
  a PipeWire virtual source is planned.
- No tray on Linux yet (it would pull in GTK).
- Per-app capture on Windows (process loopback) is planned.
