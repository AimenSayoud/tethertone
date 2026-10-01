# Plan: two-way audio, Windows and Linux, website, and what comes next

Status (1 October 2026): phases **A–B** (two-way audio) and **D1–D4** (Windows
and Linux app) are built and tested; awaiting device testing before the 0.3.0
release. Written 24 September 2026. Sizes are relative (S ≈ a day or two, M ≈ a week, L ≈ several
weeks) and assume one developer.

---

## 0. Summary

| Track | Recommendation | Size |
|---|---|---|
| **Two-way audio** | Protocol v2 with a control channel. The phone can send its **microphone** or its **playing audio** to the Mac; the Mac plays it or exposes it as a microphone. A direction switch in both apps: *To phone · To computer · Both*. | L |
| **Mac as a microphone target** | Phase 1: play into a user-chosen output such as BlackHole (no driver work). Phase 2: our own "Tethertone Mic" virtual device (a Core Audio Audio Server Plug-in built on libASPL, MIT licensed). | M, then L |
| **Windows + Linux** | One **Rust** app for both: `cpal` for audio (WASAPI loopback on Windows, PipeWire on Linux), `egui` + `tray-icon` for a tray app, built in **GitHub Actions**. Locally you only need `rustup`; no Visual Studio, no Qt, no Electron, no JVM. | L |
| **Website** | Only advertise what has shipped. For each feature: one how-to page aimed at a real search, updates to the comparison, download cards and structured data. | S per feature |
| **Next** | Remove the Screen Recording prompt (Core Audio taps), encrypted transport, optional Opus, a browser receiver (iPhone with no install), multi-room sync. | varies |

Recommended order: **A → B → C → D → E**, with the website following each release (§4).

---

## 1. Two-way audio

### 1.1 What "phone → computer" can mean

| Source on the phone | Android API | Limits |
|---|---|---|
| **Microphone** | `AudioRecord` with `MIC` / `VOICE_COMMUNICATION` | Needs `RECORD_AUDIO`. On Android 14+ a `microphone` foreground service can only start while the app is in the foreground, which is fine because the user toggles it in the UI. [Android: FGS background-start restrictions](https://developer.android.com/develop/background-work/services/fgs/restrictions-bg-start) |
| **Audio other apps play** (Spotify, YouTube…) | `AudioPlaybackCapture` + MediaProjection (Android 10+) | The user approves a capture prompt **every session**; the token can't be cached. Apps can opt out of capture, and calls can't be captured. [Android: capture audio playback](https://developer.android.com/media/platform/av-capture), [FGS changes 2026](https://dev.to/joe_wang_6a4a3e51566e8b52/android-foreground-services-in-2026-what-changed-and-how-to-adapt-2o3d) |

| Destination on the computer | How |
|---|---|
| **Play it** on the computer's speakers | A playback path with its own jitter buffer and drift correction (the mirror of the phone's). |
| **Use it as a microphone** in Zoom, Discord, OBS… | Needs a virtual input device; see §1.4. |

So the user-facing switch is:

```
Direction:   [ To phone ]  [ To computer ]  [ Both ]
From phone:  ( Microphone | What the phone is playing )     ← when "To computer" or "Both"
On computer: ( Play on speakers | Use as microphone )       ← when "To computer" or "Both"
```

Both apps show the same switch, and changing it on one side updates the other
over the control channel. No reconnect or re-pairing is needed.

### 1.2 Protocol v2 (backward compatible)

- **HELLO/WELCOME gain `v: 2` and `streams`.** A v1 phone keeps working against a
  v2 Mac (down only), and a v2 phone falls back to v1 when the Mac says `v: 1`.
- **AUDIO (type 0) becomes valid in both directions** once negotiated, with
  `seq` counted per direction. The up stream declares its own format (e.g. 48 kHz
  mono for the mic, stereo for captured playback).
- **New frame type 5, CONTROL (JSON, both directions):**
  `{"cmd":"set_streams","down":true,"up":{"source":"mic","ch":1}}`, acknowledged
  with `{"cmd":"streams","…"}`. This is what makes the switch live.
- **PING/PONG in both directions**, so each receiver measures its own path.
- **Shared test vectors move to `docs/test-vectors.json`** and are read by the
  Swift self-test, the Kotlin tests, `probe.py` and the new Rust tests. Four
  implementations need one source of truth, not four copies.

### 1.3 Echo and feedback: the design risk

With **Both** on and the phone on its speaker, the phone's mic hears the
computer's audio and sends it back. And if the Mac captures the system mix
while playing the phone's audio, the phone's audio goes round a loop.

- **Mac capture excludes our own playback.** Screen Recording capture already sets
  `excludesCurrentProcessAudio`. For BlackHole capture, refuse to play the up
  stream into the same device being captured, and say why in the UI.
- **Phone mic with Both on:** use `VOICE_COMMUNICATION`, which applies the phone's
  echo canceller ([CDD 5.4](https://android.googlesource.com/platform/compatibility/cdd/+/refs/tags/platform-tools-31.0.0/5_multimedia/5_4_audio-recording.md)),
  and attach `AcousticEchoCanceler` when available. With only *To computer*, use
  the cleaner `MIC` / `UNPROCESSED` source.
- **The UI recommends headphones** on the phone when Both + Microphone is selected.

### 1.4 The Mac as a microphone target

| Option | Effort | Notes |
|---|---|---|
| **Play into a chosen output device** (e.g. BlackHole 2ch); other apps pick BlackHole as their mic | S | No driver of our own. Needs a *second* BlackHole device (16ch) if BlackHole is also the capture source, or Screen Recording capture. |
| **"Tethertone Mic": our own virtual input device** | L | A Core Audio Audio Server Plug-in (user-space; no kernel extension) built with [libASPL](https://github.com/gavv/libASPL) (C++17, MIT), the same approach as [roc-vad](https://github.com/roc-streaming/roc-vad). The pkg installs it into `/Library/Audio/Plug-Ins/HAL` and restarts `coreaudiod`. |

Risks to prove with a **spike before committing to the plug-in**:
1. Getting audio from the app into the plug-in. Plug-ins run inside the
   sandboxed `coreaudiod` host, so the path has to be one the sandbox allows
   (Mach services declared in the plug-in's Info.plist, or shared memory).
2. Whether a self-signed (not notarized) plug-in loads on current macOS on Apple
   Silicon.
3. Building a `.driver` bundle with the Command Line Tools only (clang++ compiling
   libASPL directly, no CMake) to keep the "no Xcode" promise.

**Recommendation:** ship option 1 first; run the spike in parallel.

### 1.5 Work breakdown

| # | Work | Where | Size |
|---|---|---|---|
| A1 | Protocol v2 spec, CONTROL frame, shared `test-vectors.json` | `docs/`, all implementations | M |
| A2 | Kotlin: `BridgeClient` sends AUDIO up and handles CONTROL; tests | `android/shared` | M |
| A3 | Swift: `ClientSession` receives AUDIO; jitter buffer + drift correction ported from `JitterBuffer.kt`; `UpstreamPlayer` (AVAudioEngine + varispeed) | `macos/` | M |
| B1 | Android: `MicSource` (`AudioRecord`, AEC when Both); FGS types `mediaPlayback\|microphone`; permission flow | `androidApp` | M |
| B2 | Android: `PlaybackCaptureSource` (MediaProjection consent each session, FGS type `mediaProjection`) | `androidApp` | M |
| B3 | UI in both apps: direction switch, source and destination pickers, per-direction stats | both | M |
| C1 | Mac: "Use as microphone" via a chosen output device, with loop protection | `macos/` | S |
| C2 | Spike, then the Tethertone Mic plug-in (libASPL), pkg install/uninstall | `macos/driver/` | L |

---

## 2. Windows and Linux

### 2.1 Options compared

| Approach | Capture system audio | Footprint | Tooling to install | Verdict |
|---|---|---|---|---|
| **Rust + cpal + egui/tray-icon** | WASAPI loopback ([cpal PR #339](https://github.com/RustAudio/cpal/pull/339)); PipeWire backend on Linux ([cpal](https://github.com/RustAudio/cpal)) | one binary, a few MB | `rustup` only; CI builds the rest | **Recommended** |
| Kotlin + Compose Desktop | JVM has no loopback capture; would need native code anyway | 50–80 MB with bundled JVM | JDK, Gradle | Reuses the client code only; rejected |
| Tauri (Rust + web UI) | same as Rust | small on Windows (WebView2 built in) | Node + Rust; WebKitGTK on Linux | UI is nice, but WebKitGTK is a heavy Linux dependency |
| C++ + miniaudio + Dear ImGui | yes | smallest | a C++ toolchain per OS | More code, no memory safety |
| Go + malgo | yes (cgo) | small | Go + C toolchain | Weaker GUI and tray story |

Why **egui** over Slint or Tauri: MIT/Apache licensed like Tethertone (Slint's
free licence has conditions), no web runtime, and a tray app plus one settings
window is exactly what egui does well.

### 2.2 Shape of the app (`desktop/` in this repo)

```
desktop/
  Cargo.toml             workspace
  core/                  protocol v2, pairing URI, jitter buffer, drift correction (no UI, no OS code)
  audio/                 capture + playback per OS behind one trait
    windows.rs           WASAPI loopback; later per-app capture via process loopback
    linux.rs             PipeWire (fallback: PulseAudio monitor source)
  app/                   tray icon, settings window (egui), QR code, adb reverse
```

- **Same features as the Mac app:** server + QR pairing, USB via `adb reverse`,
  capture method, level meter, clients list, then two-way audio (§1).
- **Per-app capture on Windows:** process loopback via `ActivateAudioInterfaceAsync`
  ([Microsoft sample](https://learn.microsoft.com/en-us/samples/microsoft/windows-classic-samples/applicationloopbackaudio-sample/)),
  Windows 10 build 20348+.
- **Phone as microphone:**
  - **Linux** creates a PipeWire virtual source, with no admin rights and no driver.
  - **Windows** can't do it without a signed kernel driver. v1 plays into a
    user-installed virtual cable (VB-CABLE, or the open-source
    [Virtual-Audio-Driver](https://github.com/VirtualDrivers/Virtual-Audio-Driver),
    which has production-signed releases). We don't ship a driver of our own.

### 2.3 Building without heavy tools

- **CI does the real builds:** `windows-latest` and `ubuntu-latest` runners produce
  the release artifacts, so nobody needs Windows or Linux machines to release.
- **Locally on the Mac:** `rustup` for quick checks. Optionally
  [cargo-xwin](https://github.com/rust-cross/cargo-xwin) cross-compiles the Windows
  binary without Visual Studio.
- **Artifacts:**
  - Windows: a portable `.zip`, then an installer and a `winget` manifest.
  - Linux: `.tar.gz` and `.deb` (`cargo-deb`), then Flathub.
- **Windows signing:** unsigned binaries trigger SmartScreen. Free code signing
  for open source (e.g. SignPath's OSS programme) is worth applying for; check the
  current terms first.

### 2.4 Work breakdown

| # | Work | Size |
|---|---|---|
| D1 | `core`: protocol v2 + tests against `test-vectors.json` and `probe.py` | M |
| D2 | `audio`: Windows loopback + Linux PipeWire capture, playback | M |
| D3 | `app`: tray, settings window, QR, adb, autostart | M |
| D4 | CI release jobs for Windows and Linux, SHA256SUMS joined with the Mac/Android release | S |
| D5 | Two-way audio on desktop (after §1), PipeWire virtual mic, virtual-cable output on Windows | M |

---

## 3. Phasing

| Phase | Contents | Release |
|---|---|---|
| **A** | Protocol v2 + CONTROL + shared vectors (A1–A3) | internal |
| **B** | Phone mic → Mac speakers, then phone playback → Mac; switches in both UIs (B1–B3, C1) | **0.3.0** "Two-way" |
| **C** | Tethertone Mic device on macOS (spike, then C2) | 0.4.0 |
| **D** | Windows + Linux sender (D1–D4) | **0.5.0** "Windows & Linux" |
| **E** | Two-way on Windows/Linux (D5) | 0.6.0 |

Testing stays hands-on on real devices; unit and protocol tests run in CI.

---

## 4. Website plan

Rule: **the site describes what's in the latest release.** Unreleased work
appears only in the roadmap, labelled as planned.

| When | Change |
|---|---|
| Now | Update the open-source roadmap: two-way audio and Windows/Linux as **Next** |
| 0.3.0 | New how-to pages aimed at real searches: **"Use your Android phone as a Mac microphone"** and **"Play your Android phone's audio on your Mac"**. Home hero and features gain the direction switch. FAQ: "Can I use my phone as a mic?" Compare: the microphone row becomes *Yes* for Tethertone. `SoftwareApplication.featureList` updated. |
| 0.4.0 | The mic page explains "Tethertone Mic"; Troubleshooting covers the driver |
| 0.5.0 | **/windows/** and **/linux/** landing pages; download page detects Windows/Linux; `operatingSystem` lists all four; Compare: "Other computers" row updated; install guides (SmartScreen, PipeWire) |
| Each release | Version bump in `site.config.mjs`; IndexNow sends the changed pages; `llms.txt` regenerates |

Illustrations: the hero gains a second, reversed packet stream on the link line
when showing *Both*, using the same transform-only animation.

---

## 5. Other directions worth taking

Ranked by value for effort.

| Idea | Why | Size |
|---|---|---|
| **Core Audio taps on macOS 14.2+** | Captures the system mix with only the "System Audio Recording" permission, with no Screen Recording prompt and no BlackHole; also enables per-app capture. [Apple docs](https://developer.apple.com/documentation/CoreAudio/capturing-system-audio-with-core-audio-taps), [AudioCap sample](https://github.com/insidegui/AudioCap) | M |
| **Encrypted transport** | The QR can carry the Mac's public key, so pairing stays one scan and every stream is encrypted with a pinned key. Makes VPN/tunnel use safe. | M |
| **Browser receiver** | Any device with a browser becomes a speaker, including **iPhones with no App Store app**. The original WebSocket prototype proves it works; add AudioWorklet and the v2 protocol over WebSocket. | M |
| **Optional Opus** | ~128 kbit/s instead of 1.5 Mbit/s for mobile data and VPNs. macOS has `kAudioFormatOpus` in AVAudioConverter ([Apple](https://developer.apple.com/documentation/coreaudiotypes/kaudioformatopus)); Android decodes Opus natively (encoding to verify per API level, or bundle libopus). | M |
| **Android TV and tablets** | Mostly a leanback launcher entry and D-pad focus. Turns a TV into a Mac speaker. | S |
| **Multi-room sync** | Several phones playing sample-aligned. Build on PING for clock sync, or interoperate with [Snapcast](https://github.com/snapcast/snapcast) (sub-millisecond sync, already on F-Droid). | L |
| **Headless Linux server** | Falls out of D1–D2: a Raspberry Pi as a network audio bridge. | S after D |
| **Distribution** | Google Play, F-Droid (metadata ready), Homebrew cask, winget, Flathub; notarized macOS builds with an Apple Developer ID. | S each |
| **iOS app** | Native iPhone receiver; needs an Apple Developer account. The browser receiver covers most of the value first. | L |

---

## 6. Decisions needed before starting

1. **Order:** A → B → C → D as proposed, or Windows/Linux (D) before two-way (B)?
2. **Mac microphone:** ship "play into BlackHole" first and spike the plug-in, or go straight to the plug-in?
3. **Desktop stack:** Rust + egui as recommended?
4. **Phone playback capture** (MediaProjection prompt every session): include it in 0.3.0, or ship the microphone first and add it after?
