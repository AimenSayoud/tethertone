# Building from source

## Prerequisites

| For | You need |
|---|---|
| macOS app | macOS 14+, Xcode **or** just the Command Line Tools (`xcode-select --install`) |
| Android app | JDK 17, Android SDK with platform 36 (Android Studio installs both) |
| USB route | `adb` — `brew install android-platform-tools` |
| BlackHole capture | `brew install blackhole-2ch` (optional) |
| `tools/probe.py` | Python 3.9+; `pip install sounddevice` only for `--play` |

The version number for both apps lives in [`VERSION`](../VERSION).

## macOS app

```bash
cd macos
./make-signing-identity.sh     # once per machine — see below
./build.sh                     # → macos/Tethertone.app (this Mac's architecture)
./build.sh --install           # …and copy it into /Applications
./build.sh --universal         # arm64 + x86_64
./package.sh                   # → dist/Tethertone-<version>-macos.{dmg,pkg}
```

The app is a SwiftPM executable; `build.sh` assembles the `.app` bundle,
writes `Info.plist` and signs it. The first build takes several minutes while
the SwiftUI module cache is built; later builds take seconds.

### Why a signing identity

macOS stores Screen Recording and Microphone permission against the app's
code signature. An ad-hoc signature changes on every build, so without a stable
identity **each rebuild silently revokes both permissions**.
`make-signing-identity.sh` creates a self-signed code-signing certificate named
*Tethertone Local Signing* in your login keychain. `build.sh` uses it when it
exists and falls back to ad-hoc signing otherwise. Use another identity with
`SIGNING_IDENTITY="…" ./build.sh`.

The app is signed with the hardened runtime and the
`com.apple.security.device.audio-input` entitlement
([`Tethertone.entitlements`](../macos/Tethertone.entitlements)). Without that
entitlement, microphone access, and so BlackHole capture, is refused.

### Self-test and headless mode

```bash
macos/Tethertone.app/Contents/MacOS/Tethertone --selftest   # protocol conformance vectors
macos/Tethertone.app/Contents/MacOS/Tethertone --headless   # server without UI, prints the pairing URI
```

XCTest ships with Xcode rather than the Command Line Tools, so the Swift
protocol tests run inside the app binary. They assert the same byte vectors as
the Kotlin tests.

## Android app

```bash
cd android
./gradlew :androidApp:installDebug      # build and install on a connected phone
./gradlew :androidApp:assembleRelease   # → androidApp/build/outputs/apk/release/
./gradlew :shared:jvmTest               # unit tests: protocol, pairing, jitter buffer, routes
```

`local.properties` (git-ignored) must point at your SDK if `ANDROID_HOME` is
not set:

```properties
sdk.dir=/Users/you/Library/Android/sdk
```

### Release signing

Release builds are signed with a key supplied through Gradle properties, never
from the repository. Put these in `~/.gradle/gradle.properties`:

```properties
TETHERTONE_KEYSTORE=/absolute/path/to/tethertone-release.jks
TETHERTONE_KEYSTORE_PASSWORD=…
TETHERTONE_KEY_ALIAS=tethertone
TETHERTONE_KEY_PASSWORD=…
```

Without them, `assembleRelease` signs with the debug key so anyone can still
build one. Android refuses to update an app signed with a different key, so
switching between debug and release builds on one phone needs an uninstall.

## Windows and Linux app

```bash
cd desktop
cargo test                          # protocol vectors, jitter buffer, server end to end
cargo run --release                 # the window
cargo run --release -- --headless   # server only, QR in the terminal
```

Needs `rustup`. Linux also needs `libpulse-dev` and the windowing headers listed
in [`desktop/README.md`](../desktop/README.md). From a Mac you can type-check the
Windows build with `rustup target add x86_64-pc-windows-msvc` and
`cargo clippy --target x86_64-pc-windows-msvc`. Release binaries come from
`.github/workflows/release-desktop.yml` when a version tag is pushed.

## Testing without a phone

```bash
# terminal 1 — the server
macos/Tethertone.app/Contents/MacOS/Tethertone --headless

# terminal 2 — either client
python3 tools/probe.py                                     # independent Python client
cd android && ./gradlew :desktopSink:run --args="--seconds 10"   # the real Kotlin client on the JVM
```

`probe.py` reads the token from `~/.tethertone/token`, completes the
handshake, answers pings, and reports the delivered sample rate, sequence
gaps, throughput and peak level, ending in PASS or FAIL. It also accepts a pairing URI: `python3 tools/probe.py 'tethertone://p?…'`.
If `probe.py` passes and the Kotlin client does not, the bug is in the Kotlin
client, and vice versa.

Headless mode uses the capture method last chosen in the app. Screen Recording
capture needs the app bundle, because macOS grants that permission to a bundle.

## Releasing

```bash
./scripts/release.sh
```

This builds the universal macOS app, the `.dmg` and `.pkg`, and the signed
release APK into `dist/`, with a `SHA256SUMS` file. To publish:

1. Bump [`VERSION`](../VERSION) and add a section to [`CHANGELOG.md`](../CHANGELOG.md).
2. Run `./scripts/release.sh`.
3. Commit, tag `v<version>`, push, and attach `dist/*` to a GitHub release:
   ```bash
   gh release create "v$(cat VERSION)" dist/* --title "Tethertone $(cat VERSION)" --notes-file <(…)
   ```

Keep the Android keystore and the macOS signing identity backed up. Updates
must be signed with the same keys, or Android refuses the update and macOS
drops the user's permissions.
