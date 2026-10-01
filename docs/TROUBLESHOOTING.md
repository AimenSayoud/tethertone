# Troubleshooting

## macOS

### “Tethertone can't be opened” / “Apple could not verify…”

Release builds are not notarized. Open **System Settings › Privacy & Security**,
scroll to the message about Tethertone and click **Open Anyway**. Or:

```bash
xattr -dr com.apple.quarantine /Applications/Tethertone.app
```

### Permission errors although the toggle is on

macOS ties each permission to the app's code signature. After an update signed
differently, System Settings can still show Tethertone as allowed while macOS
denies it. Switch Tethertone **off and on again** in the relevant list. If that
does not help, reset the permission and press Start again to get a fresh prompt:

```bash
tccutil reset ScreenCapture dev.tethertone.mac   # Screen Recording method
tccutil reset Microphone dev.tethertone.mac      # BlackHole method
```

If you build from source, run `macos/make-signing-identity.sh` once so rebuilds
keep the same signature (see [BUILDING.md](BUILDING.md)).

### Why does it ask for Screen Recording? It only sends audio.

The **Screen Recording** capture method uses ScreenCaptureKit, the only macOS
API that hands over the full system mix without a virtual driver. It has no
audio-only mode, so macOS classes it as screen recording and shows the
recording indicator. Tethertone requests a 2×2 video frame twice a second and
discards it; no image is ever read.

To avoid the permission, install BlackHole and switch **Capture method** to
**BlackHole**, which needs only the Microphone permission.

### The level meter shows silence

- **BlackHole method:** the Mac's output must go *into* BlackHole. Set **BlackHole
  2ch** as the sound output, or build a Multi-Output Device in Audio MIDI Setup
  that includes it. Check that Tethertone's input device is BlackHole too.
- **Screen Recording method:** make sure something is playing. Audio sent
  straight to a hardware device by pro audio apps that bypass the system mix may
  not be captured.

### My Mac's speakers went silent

That happens when output is routed into BlackHole. Either enable **Audio › Hear
it on this Mac too**, use a Multi-Output Device, or use the Screen Recording
method, which leaves the speakers alone.

### Pitch or speed is wrong

BlackHole's sample rate changed after capture started. Set it to 48 kHz in
**Audio MIDI Setup** and press Stop, then Start.

## Two-way audio

### The Mac shows “The phone declined”

The phone decides what it sends, because only it can grant the permissions.
Some things need the phone's screen:

- **Microphone:** allow microphone access in Tethertone on the phone once. After
  that the Mac can switch the microphone on while the phone is in your pocket.
- **Phone's audio:** Android asks for a capture consent every session. Open
  Tethertone on the phone and tap **Allow capturing this phone's audio**.

### “Another phone is already sending audio”

The Mac plays audio from one phone at a time. Switch the other phone back to
*Mac → phone* first.

### Echo or howling

With *Both ways* and the phone's microphone, the phone's speaker is heard by
its own microphone. Use headphones on the phone. Tethertone uses Android's
echo cancellation in this mode, but no canceller removes a loud speaker right
next to the microphone.

### “Would send it straight back to the phone”

You are capturing BlackHole and also playing the phone's audio into BlackHole,
so it would loop back to the phone. Under **Direction › Phone audio plays on**,
pick another output, or use the Screen Recording capture method.

### Using the phone as a microphone in other apps

Install BlackHole, set **Phone audio plays on** to **BlackHole 2ch**, and pick
BlackHole as the microphone in Zoom, Discord, OBS or any other app. If BlackHole
is also your capture source, install **BlackHole 16ch** as a second device for
the microphone, or capture with Screen Recording instead.

## Connecting

### The phone scans the QR but never connects

- Check the Mac's **Overview › Activity** card. If nothing arrives there, the
  phone's packets are not reaching the Mac: the devices are on different
  networks, or the network blocks device-to-device traffic (common on guest and
  corporate Wi-Fi). Use the USB cable.
- The macOS firewall may be blocking incoming connections. Allow Tethertone
  in **System Settings › Network › Firewall › Options**.
- If you changed the port or rotated the token, scan the new QR.

### USB is listed as “not available”

- Install `adb`: `brew install android-platform-tools`.
- Enable **USB debugging** on the phone and accept the “Allow USB debugging?”
  prompt.
- Check `adb devices` lists the phone as `device`, not `unauthorized`.
- Make sure **Network › Offer the USB path** is on. Tethertone sets up
  `adb reverse` by itself when the phone appears.

### Audio stutters

- Prefer USB; Wi-Fi congestion is the usual cause.
- Raise the **Jitter buffer** slider on the phone's home screen (disconnect
  first — it is locked while streaming). Higher values mean more latency and
  fewer underruns.
- On the Mac, choose a larger packet size under **Network › Packet size**.
- The phone's **Clock drift** card should show a small ppm correction. If it
  says rate control is unavailable, turn off **Prefer low-latency audio path**.

### Playback stops when the screen turns off

Some manufacturers kill background apps aggressively. Exempt Tethertone from
battery optimisation in the phone's settings. See
[dontkillmyapp.com](https://dontkillmyapp.com).

## Using it away from home

A phone on mobile data cannot reach the Mac directly. The Mac sits behind a
router and the phone behind carrier-grade NAT, so neither has an address the
other can dial. What works, best first:

| Route | Setup | Notes |
|---|---|---|
| USB cable | none | lowest latency, works anywhere |
| Same Wi-Fi | none | the default |
| Mesh VPN (Tailscale, ZeroTier) | install on both devices | works from anywhere; its address appears in the QR by itself |
| Public tunnel (ngrok, cloudflared) | enter the hostname in **Network › Reaching this Mac from anywhere** | exposes the port to the internet: the token gates it, but audio is unencrypted |

The stream is uncompressed 16-bit stereo PCM at 48 kHz: about **1.5 Mbit/s, or
roughly 690 MB per hour**. That is nothing over USB or Wi-Fi, but expensive on
a metered connection.

## Still stuck?

Open an [issue](https://github.com/AimenSayoud/tethertone/issues/new/choose)
with your macOS and Android versions, the phone model, the capture method, and
the contents of the Mac's **Activity** card.
