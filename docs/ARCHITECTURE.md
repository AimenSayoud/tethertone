# Architecture

Tethertone has two halves that share one small wire protocol:

- **macOS server** (`macos/`, Swift) — captures audio, serves it over TCP,
  advertises itself with a QR code and optionally tunnels over USB with
  `adb reverse`.
- **Android client** (`android/`, Kotlin) — connects, buffers, corrects clock
  drift and plays through `AudioTrack`.

```
┌──────────────────────────── macOS ────────────────────────────┐        ┌──────────────────────── Android ────────────────────────┐
│                                                               │        │                                                          │
│  AudioSource ──► 10 ms PCM blocks ──► BridgeServer (NWListener)│  TCP   │  BridgeClient ──► JitterBuffer ──► AudioSink (AudioTrack)│
│  ├─ SystemAudioCapture (ScreenCaptureKit)      │ fan-out      │───────►│  (races all hosts,  (target depth,   (rate-trimmed for  │
│  └─ DeviceCapture (AVAudioEngine, BlackHole)   ▼              │        │   auto-reconnect)    drop-oldest)     drift correction)  │
│                                   ClientSession × N           │        │                                                          │
│                                   (handshake, queue, ping)    │        │  TethertoneService — foreground service, owns the above │
└───────────────────────────────────────────────────────────────┘        └──────────────────────────────────────────────────────────┘
```

## Capture (macOS)

Both sources implement one protocol, `AudioSource`, and both emit **fixed-size
blocks** of interleaved 16-bit PCM — 480 frames, 10 ms at 48 kHz — regardless of
what Core Audio delivers.
A client's jitter buffer copes far better with a steady cadence than with
whatever buffer size the hardware felt like that moment.

| Source | File | How |
|---|---|---|
| Screen Recording | `Audio/SystemAudioCapture.swift` | An `SCStream` with `capturesAudio = true`. ScreenCaptureKit cannot switch video off, so the stream asks for a 2×2 frame twice a second and ignores it. `excludesCurrentProcessAudio` prevents feedback if the app ever plays sound. |
| BlackHole | `Audio/DeviceCapture.swift` | `AVAudioEngine` with its input pinned to a chosen Core Audio device. |

`Audio/MonitorPlayback.swift` optionally plays a copy of the captured audio to
a local output device, so routing system output into BlackHole does not
silence the Mac.

## Transport

A raw TCP connection per client, not WebSocket or HTTP:

- an **8-byte header** carries a **sequence number**, so the client reports
  real packet loss rather than guessing;
- **PING/PONG** frames give a true round-trip time, measured on the Mac where
  both timestamps share a clock;
- there is no upgrade handshake, masking or per-message overhead on a stream of
  100 packets a second.

Each `ClientSession` has its own bounded send queue that **drops the oldest**
audio when a client falls behind. A slow phone never stalls the capture thread
or the other clients. See [PROTOCOL.md](PROTOCOL.md) for the exact format.

### Routes

The pairing QR lists every address the Mac can be reached at, in order:

1. `127.0.0.1` — when a phone is attached over USB and `adb reverse` succeeded;
2. physical LAN addresses (Wi-Fi, Ethernet);
3. VPN addresses (`utun*`, e.g. Tailscale);
4. an optional user-supplied hostname (a tunnel).

The client races connections to all of them — each one a few milliseconds
after the previous, so earlier routes get a head start — and keeps the first
that completes the handshake, so the cable wins whenever it is plugged in and
Wi-Fi takes over when it is not. The Mac polls for USB devices and sets up
`adb reverse` as they appear.

## Two-way audio

With the `duplex` capability the phone can also send audio up: its microphone
(`AudioRecord`, with `VOICE_COMMUNICATION` and the echo canceller when the Mac's
audio plays at the same time) or other apps' playback (`AudioPlaybackCapture`
through a MediaProjection consent, Android 10+).

The **phone owns the mode**, because it holds the permissions. The Mac's
direction switch sends a `setMode` request; the phone applies it or answers
`refused` with a reason the Mac shows. This matters on Android 14+, where
microphone and projection access from the background require the foreground
service to have claimed those types while the app was on screen.

On the Mac, `UpstreamPlayer` is the mirror of the phone's playback path: a
jitter buffer (60 ms target) and the same ±0.2% drift-correction law, applied
by linear-interpolation resampling inside the render callback. It can play to
any output, including BlackHole, which is how the phone becomes a microphone
for other apps. The server takes audio from one phone at a time.

Loops are broken where they can be seen: Tethertone's own playback on the phone
is marked `ALLOW_CAPTURE_BY_NONE`, ScreenCaptureKit excludes the Mac app's own
output, and the Mac refuses to play phone audio into the device it captures.

## Pairing and authentication

The QR encodes a `tethertone://` URI with the hosts, port, a 16-byte random
token and display hints. The token lives in `~/.tethertone/token` (mode 0600)
so pairing survives restarts. The server compares it in constant time and sends
no audio before a valid HELLO. Rotating the token invalidates every paired
phone. Tapping a `tethertone://` link on the phone pairs without the camera.

The token authenticates; it does **not** encrypt. See [SECURITY.md](../SECURITY.md).

## Playback (Android)

### Jitter buffer

`shared/…/JitterBuffer.kt` holds decoded PCM between the network and the audio
device. It has a target depth (90 ms by default, 20–250 ms with the slider on the home screen). Network
bursts fill it and gaps drain it. If it runs dry the sink plays silence and
counts an **underrun**. If it overflows, the oldest audio is discarded.

### Clock drift — the actual hard problem

Two devices never agree on what a second is. The Mac's audio clock and the
phone's differ by tens of parts per million. A buffer that is only corrected
when it starves drifts one way until it either runs dry or grows without bound
— over an hour, that is seconds of added latency.

Tethertone holds the buffer at its target by playing a fraction of a percent
off nominal rate. At ±0.2 % that is about three cents of pitch — inaudible —
and there is never a discontinuity. The phone shows it live on the **Clock
drift** card (“playing 222 ppm fast to drain the buffer”), and the Mac shows the
same figure per client.

Two findings from real hardware shaped `AudioSink.android.kt`:

- `AudioTrack.setPlaybackParams` (the modern, pitch-preserving API) is rejected
  by a track built with `PERFORMANCE_MODE_LOW_LATENCY`. `setPlaybackRate`, the
  older API, works.
- On some devices a low-latency track refuses **both**. Smooth correction is
  worth more than the few milliseconds low-latency mode saves, so the sink
  probes for rate control and, if it is refused, rebuilds the track without
  low-latency mode. The preference is in Settings; the fallback is automatic.

When no rate control exists at all, the buffer is corrected by discarding
audio instead — a click every few minutes rather than unbounded latency. The UI
says which mode is active.

### Lifecycle

`TethertoneService` is a foreground service of type `mediaPlayback`. It owns
the client and the sink, so playback continues with the screen off, and its
notification can stop it from outside the app. The client reconnects
automatically with backoff when the Mac stops or the network changes.

## The shared Kotlin module

`android/shared` targets **android and jvm**:

| Source set | Contents |
|---|---|
| `commonMain` | protocol codec, pairing URI, jitter buffer, route ordering, client state machine — no platform types |
| `jvmCommonMain` | the blocking socket, shared by both targets |
| `androidMain` | `AudioTrack` sink |
| `jvmMain` | `javax.sound` sink |

The JVM target is not decoration. It keeps Android types out of the logic,
makes the unit tests run in seconds without a device, and powers
`desktopSink` — the same client running on a desktop JVM. When the phone
misbehaves, running the desktop sink tells you whether the protocol or Android
is at fault.

## Cross-implementation testing

The protocol is implemented three times, independently:

| Implementation | Language | Tested by |
|---|---|---|
| macOS server | Swift | `Tethertone --selftest` — fixed byte vectors |
| Android/JVM client | Kotlin | `./gradlew :shared:jvmTest` — the **same** vectors |
| `tools/probe.py` | Python | run against a live server |

The Swift self-test and the Kotlin tests assert identical byte sequences on
purpose. Two implementations of one wire format drift apart silently otherwise.

## Why the Mac side is native Swift

The Mac is the server and shares almost nothing with the Kotlin client — only
the frame codec and pairing URI, about 100 lines. Native Swift buys what a JVM
cannot reach: ScreenCaptureKit, dependable Core Audio device selection, proper
TCC permission handling and a ~2 MB app.

## Lessons recorded so they are not relearned

- **macOS permissions are keyed to the code signature.** An ad-hoc signature
  changes on every build, so each update silently revoked Screen Recording and
  Microphone. `make-signing-identity.sh` creates a stable self-signed identity
  so a grant survives rebuilds.
- **The hardened runtime needs `com.apple.security.device.audio-input`** for
  microphone access. Without it, access is denied even when System Settings
  shows the toggle on.
- **A session held only by its own callback is already dead.** `ClientSession`
  must be retained on accept, not when the handshake completes.
- **`NWListener` reports its bind asynchronously.** Success is only known in
  the `.ready` state; logging after `start()` announced ports that then failed
  with `EADDRINUSE`.
- **A failure reported from another task can race the start it is failing.**
  ScreenCaptureKit reports denial from its own `Task`, often while `start()` is
  still running; the failure handler hops to the main queue.
- **GUI apps do not inherit the shell's `PATH`**, so `adb` is looked up at the
  usual Homebrew and Android SDK locations.
