# Tethertone wire protocol v1

One TCP connection per sink. The server (the computer) pushes audio to the
client (the phone). With the **duplex** capability (§6) the client can also
send audio back, and either side can switch directions live. All multi-byte
integers are **big-endian** (network order).

Test vectors shared by every implementation live in
[`test-vectors.json`](test-vectors.json).

## 1. Handshake

Two UTF-8 JSON documents, each terminated by `\n`. Max 8 KiB each.

**Client → server (HELLO)**

```json
{"v":1,"role":"sink","token":"<base64url>","name":"Pixel 9","caps":["pcm_s16le","duplex"]}
```

**Server → client (WELCOME)**

```json
{"v":1,"ok":true,"rate":48000,"ch":2,"fmt":"pcm_s16le","block":480,"name":"Studio Mac","caps":["duplex"]}
```

`caps` lists optional capabilities. Each side enables a capability only if
both list it, and ignores capabilities it does not know. This is how new
features stay compatible with older apps without a version bump.

On failure the server replies `{"v":1,"ok":false,"err":"invalid token"}` and
closes. The token is compared with a constant-time comparison. A client that
sends no HELLO within 5 s is dropped.

`block` is advisory: the frame header carries the real length.

## 2. Frames

After WELCOME, the stream is a sequence of frames:

```
 0        1        2                4                              8
 +--------+--------+----------------+------------------------------+
 |  type  | flags  |     length     |            seq               |
 +--------+--------+----------------+------------------------------+
 |                        payload (length bytes)                   |
 +-----------------------------------------------------------------+
```

| Field    | Size | Meaning                                        |
|----------|------|------------------------------------------------|
| `type`   | u8   | frame type (below)                             |
| `flags`  | u8   | reserved, must be 0                            |
| `length` | u16  | payload length, 0..65535                       |
| `seq`    | u32  | wraps at 2^32; increments per frame, per type  |

| type | name  | direction | payload                                       |
|------|-------|-----------|-----------------------------------------------|
| 0    | AUDIO   | S → C (and C → S with duplex) | interleaved PCM s16le |
| 1    | PING    | S → C     | u64 server monotonic microseconds             |
| 2    | PONG    | C → S     | the PING payload, echoed verbatim             |
| 3    | STATS   | C → S     | UTF-8 JSON, client telemetry (best effort)    |
| 4    | BYE     | both      | optional UTF-8 reason                         |
| 5    | CONTROL | both      | UTF-8 JSON object; duplex only (§6)           |

Unknown frame types must be ignored, not treated as errors.

Note that **AUDIO payload is little-endian** PCM (it is raw sample data
straight from CoreAudio, not a protocol integer); only the header is
big-endian. A gap in `seq` on AUDIO frames means frames were dropped in the
server's send queue — the client reports this as packet loss.

Server sends PING once per second. RTT = now − echoed timestamp, measured on
the server; the client also reports its own view in STATS.

## 3. Pairing payload (QR code)

```
tethertone://p?h=<ipv4>&h=<ipv4>&p=<port>&t=<token>&r=<rate>&c=<channels>&n=<name>
```

`h` repeats once per reachable interface address; the client races connections
to all of them in parallel and keeps the first that completes the handshake.
`n` is percent-encoded. `r`, `c` and `n` are hints for the UI before the
handshake lands — the WELCOME values always win.

A bare `host:port` string is also accepted (no token, for manual/USB entry).

## 4. Transports

| Mode | Server bind | Client target            | Setup                              |
|------|-------------|--------------------------|------------------------------------|
| USB  | 127.0.0.1   | 127.0.0.1:port           | `adb reverse tcp:port tcp:port`    |
| LAN  | 0.0.0.0     | one of the QR's `h` hosts| same Wi-Fi                         |

USB is lower latency and immune to Wi-Fi congestion; LAN is cable-free. The
client treats them identically once connected.

## 5. Security

The token is 16 random bytes, base64url, persisted at `~/.tethertone/token`
so pairing survives restarts. It authenticates the client to the server only —
the audio itself is **not encrypted**. On an untrusted network, use USB mode.
Rotate it in the macOS app under **Pairing › Generate a new token**; every
paired client then has to scan again.

## 6. Duplex: audio in both directions

Enabled when both HELLO and WELCOME list `"duplex"` in `caps`.

### Modes

The **client owns the mode**, because it owns the microphone and the
capture consent. A mode has two parts:

| Field  | Values | Meaning |
|--------|--------|---------|
| `down` | `true` / `false` | the server sends AUDIO to this client |
| `up`   | `"off"`, `"mic"`, `"playback"` | the client sends AUDIO to the server: its microphone, or the audio its other apps are playing |

### CONTROL messages

Each CONTROL payload is one JSON object with a `t` field. Unknown `t` values
and unknown keys are ignored.

| `t` | Direction | Fields | Meaning |
|-----|-----------|--------|---------|
| `mode` | C → S | `down`, `up`, `upRate`, `upCh` | The client's current mode. Sent right after the handshake and on every change. `upRate`/`upCh` describe the client's AUDIO frames and are present when `up` is not `off`. |
| `setMode` | S → C | `down`, `up` | The server's user asked for a different mode. The client answers with `mode` (applied) or `refused`. |
| `refused` | C → S | `reason` | The client could not apply `setMode`, e.g. because the phone's screen must be used to grant a permission. `reason` is shown to the server's user. |
| `upDenied` | S → C | `reason` | The server will not take this client's audio, e.g. because another client is already sending. The client switches `up` to `off` and shows `reason`. |

Examples:

```json
{"t":"mode","down":true,"up":"mic","upRate":48000,"upCh":1}
{"t":"setMode","down":false,"up":"playback"}
{"t":"refused","reason":"Open Tethertone on the phone to allow capturing its audio."}
{"t":"upDenied","reason":"Another phone is already sending audio to this computer."}
```

### Audio from the client

- AUDIO frames from the client use the format of its last `mode` message
  (`upRate`, `upCh`, always `pcm_s16le`). A 10 ms block is the recommended size.
- `seq` counts per direction, from 0 for each new up stream.
- While `down` is `false` the server sends no AUDIO to that client, but keeps
  sending PING.
- A server takes audio from **one client at a time** and answers any other
  with `upDenied`.

### Feedback loops

With both directions on, audio can loop: phone mic → computer speakers →
phone mic, or computer capture → phone → computer. Implementations must break
the loops they can see:

- The server must not play client audio into the device it is capturing, and
  must exclude its own playback from system capture.
- A client capturing its microphone while also playing should use the
  platform's echo cancellation (on Android, `VOICE_COMMUNICATION`).
- A client capturing its own playback must exclude the stream it receives from
  the server.

