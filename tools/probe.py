#!/usr/bin/env python3
"""Independent Tethertone client: checks a running server without a phone.

Standard library only; --play additionally needs `pip install sounddevice`.

    python3 probe.py                      # read ~/.tethertone/token, connect to 127.0.0.1
    python3 probe.py 'tethertone://p?..' # exactly what the QR encodes
    python3 probe.py --seconds 30         # listen longer (default 8)
    python3 probe.py --play               # also play the stream locally
    python3 probe.py --up-tone            # duplex: also send a test tone up to the server
    python3 probe.py --selftest           # check this file's codec against docs/test-vectors.json

Exits 0 when the delivered sample rate matches the handshake and no audio
frames were lost, 1 otherwise.
"""
from __future__ import annotations

import argparse
import json
import math
import socket
import struct
import sys
import threading
import time
import urllib.parse
from pathlib import Path

HEADER = struct.Struct("!BBHI")
T_AUDIO, T_PING, T_PONG, T_STATS, T_BYE, T_CONTROL = 0, 1, 2, 3, 4, 5


def selftest() -> int:
    """Checks the header codec against the vectors every implementation shares."""
    path = Path(__file__).resolve().parent.parent / "docs" / "test-vectors.json"
    vectors = json.loads(path.read_text())
    failures = 0
    for v in vectors["headers"]:
        got = HEADER.pack(v["type"], v["flags"], v["length"], v["seq"]).hex()
        ok = got == v["hex"] and HEADER.unpack(bytes.fromhex(v["hex"]))[3] == v["seq"]
        failures += not ok
        print(f"  {'ok  ' if ok else 'FAIL'} header: {v['name']}")
    for v in vectors["control"]:
        msg = json.loads(v["json"])
        expect = v["expect"]
        up = msg.get("up") if msg.get("up") in ("off", "mic", "playback") else "off"
        ok = msg["t"] == expect["t"] and all(
            (up if k == "up" else msg.get(k)) == val for k, val in expect.items() if k != "t")
        failures += not ok
        print(f"  {'ok  ' if ok else 'FAIL'} control: {v['name']}")
    print("PASS" if not failures else f"FAIL ({failures})")
    return 1 if failures else 0


def parse_uri(uri: str):
    u = urllib.parse.urlparse(uri)
    q = urllib.parse.parse_qs(u.query)
    return {"hosts": q.get("h", ["127.0.0.1"]), "port": int(q.get("p", ["45678"])[0]),
            "token": q.get("t", [""])[0], "name": q.get("n", ["?"])[0]}


def recv_exact(sock: socket.socket, n: int) -> bytes:
    buf = bytearray()
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise ConnectionError("server closed")
        buf += chunk
    return bytes(buf)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("uri", nargs="?")
    ap.add_argument("--host"); ap.add_argument("--port", type=int, default=45678)
    ap.add_argument("--token"); ap.add_argument("--seconds", type=float, default=8)
    ap.add_argument("--play", action="store_true", help="play the stream on this Mac")
    ap.add_argument("--up-tone", action="store_true", help="duplex: send a 440 Hz tone up to the server")
    ap.add_argument("--selftest", action="store_true", help="check the codec against docs/test-vectors.json")
    a = ap.parse_args()
    if a.selftest:
        return selftest()

    if a.uri:
        cfg = parse_uri(a.uri)
        host, port, token = cfg["hosts"][0], cfg["port"], cfg["token"]
    else:
        host = a.host or "127.0.0.1"
        port = a.port
        token = a.token or (Path.home() / ".tethertone/token").read_text().strip()

    print(f"[probe] connecting to {host}:{port}")
    s = socket.create_connection((host, port), timeout=10)
    s.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    t_connect = time.monotonic()

    caps = ["pcm_s16le", "duplex"] if a.up_tone else ["pcm_s16le"]
    hello = {"v": 1, "role": "sink", "token": token, "name": "probe.py", "caps": caps}
    s.sendall((json.dumps(hello) + "\n").encode())

    line = b""
    while not line.endswith(b"\n"):
        line += s.recv(1)
    welcome = json.loads(line)
    print(f"[probe] handshake {(time.monotonic() - t_connect) * 1000:.0f} ms -> {welcome}")
    if not welcome.get("ok"):
        return 1

    rate, ch = welcome["rate"], welcome["ch"]

    # Duplex: announce a mono microphone-style stream and send it from a thread,
    # paced to real time. Writes from both threads go through one lock so
    # frames never interleave on the socket.
    write_lock = threading.Lock()
    def send(frame_type, payload, seq=0):
        with write_lock:
            s.sendall(HEADER.pack(frame_type, 0, len(payload), seq) + payload)

    up = {"sent": 0, "denied": None, "stop": False}
    if a.up_tone:
        if "duplex" not in welcome.get("caps", []):
            print("[probe] server does not support duplex")
            return 1
        up_rate = 48000
        send(T_CONTROL, json.dumps({"t": "mode", "down": True, "up": "mic", "upRate": up_rate, "upCh": 1}).encode())

        def uplink():
            block, phase, seq = up_rate // 100, 0.0, 0
            step = 2 * math.pi * 440 / up_rate
            next_at = time.monotonic()
            while not up["stop"]:
                samples = []
                for _ in range(block):
                    samples.append(int(math.sin(phase) * 3000))
                    phase += step
                try:
                    send(T_AUDIO, struct.pack(f"<{block}h", *samples), seq)
                except OSError:
                    return
                seq = (seq + 1) & 0xFFFFFFFF
                up["sent"] += 1
                next_at += 0.01
                time.sleep(max(0.0, next_at - time.monotonic()))

        threading.Thread(target=uplink, daemon=True).start()

    sink = None
    if a.play:
        import sounddevice as sd
        sink = sd.RawOutputStream(channels=ch, samplerate=rate, dtype="int16", latency="low")
        sink.start()

    s.settimeout(5)
    audio_bytes = frames = pings = gaps = peak = 0
    expect_seq = None
    t0 = time.monotonic()
    try:
        while time.monotonic() - t0 < a.seconds:
            head = recv_exact(s, HEADER.size)
            ftype, _flags, length, seq = HEADER.unpack(head)
            payload = recv_exact(s, length) if length else b""
            if ftype == T_AUDIO:
                if expect_seq is not None and seq != expect_seq:
                    gaps += (seq - expect_seq) & 0xFFFFFFFF
                expect_seq = (seq + 1) & 0xFFFFFFFF
                audio_bytes += len(payload); frames += 1
                sm = struct.unpack_from("<%dh" % (len(payload) // 2), payload)
                peak = max(peak, max(abs(x) for x in sm))
                if sink:
                    sink.write(payload)
            elif ftype == T_PING:
                send(T_PONG, payload, seq)
                pings += 1
                stats = json.dumps({"bufMs": 0, "under": 0, "lost": gaps, "speed": 1.0}).encode()
                send(T_STATS, stats)
            elif ftype == T_CONTROL:
                msg = json.loads(payload)
                if msg.get("t") == "upDenied":
                    up["denied"] = msg.get("reason", "denied")
    except (ConnectionError, socket.timeout) as e:
        print(f"[probe] stream ended: {e}")
    finally:
        up["stop"] = True
        if sink:
            sink.stop(); sink.close()
        s.close()

    dur = time.monotonic() - t0
    fps = audio_bytes / (2 * ch) / dur
    print(f"[probe] {frames} audio frames, {audio_bytes} B in {dur:.1f}s")
    print(f"[probe] {fps:.0f} samples/s (expect {rate}), {audio_bytes * 8 / dur / 1000:.0f} kbit/s")
    print(f"[probe] pings answered: {pings}, seq gaps: {gaps}")
    print(f"[probe] peak amplitude: {peak} ({'AUDIO PRESENT' if peak > 100 else 'SILENCE'})")
    ok = abs(fps - rate) / rate < 0.05 and gaps == 0
    if a.up_tone:
        expected = int(dur * 100)
        print(f"[probe] up: sent {up['sent']} blocks (expect ~{expected})"
              + (f", DENIED: {up['denied']}" if up["denied"] else ""))
        ok = ok and up["denied"] is None and up["sent"] >= expected * 0.9
    print(f"[probe] {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
