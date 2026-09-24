# Tethertone for Android

The client half of Tethertone: a Jetpack Compose app that receives the Mac's
audio and plays it with low, stable latency. It also contains the shared Kotlin
Multiplatform module and a desktop test client.

```bash
./gradlew :androidApp:installDebug   # build and install on a connected phone
./gradlew :shared:jvmTest            # unit tests, no device needed
```

Requires JDK 17 and the Android SDK (platform 36). The app runs on Android 8.0
(API 26) and later. See [../docs/BUILDING.md](../docs/BUILDING.md) for release
builds and signing.

## The app

- **Scan pairing QR** — CameraX + ZXing scanner. Opening a `tethertone://` link
  pairs without the camera, and **Enter address manually** takes host, port and
  token.
- **Home** — connection state, level meter, volume, jitter buffer slider, live
  stats (RTT, buffer, underruns, lost packets) and the **Clock drift** card.
- **Direction** — *Mac → phone*, *Phone → Mac* or *Both ways*, sending the
  microphone or (Android 10+) what other apps play. The Mac can switch it too;
  the phone asks for its screen only when a permission is needed.
- **Settings** — automatic reconnect, and **Prefer low-latency audio path**.
- **Foreground service** — playback continues with the screen off; the
  notification has a Stop action.

Permissions: camera (scanning only), notifications (the playback
notification), network access, and — only when you choose to send audio — the
microphone and a per-session capture consent.

## Modules

```
shared/        Kotlin Multiplatform (android + jvm)
  commonMain     Protocol, Pairing, JitterBuffer, TransportRoute, BridgeClient
  jvmCommonMain  blocking socket transport shared by both targets
  androidMain    AudioTrack sink with drift correction
  jvmMain        javax.sound sink
  commonTest     protocol vectors, pairing, jitter buffer, level, routes
androidApp/    Compose UI, QR scanner, TethertoneService, preferences
desktopSink/   headless JVM client built on the same shared code
```

`BridgeClient` is the heart of the client: it races connections to every host
in the pairing code, performs the handshake, feeds the jitter buffer, answers
pings, reports stats and reconnects with exponential backoff. It has no Android
dependencies, which is why it runs unchanged in `desktopSink`:

```bash
./gradlew :desktopSink:run --args="--seconds 10"                # uses ~/.tethertone/token
./gradlew :desktopSink:run --args="'tethertone://p?h=…&t=…'"   # a specific pairing URI
```

See [../docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md) for how the jitter
buffer and drift correction work.
