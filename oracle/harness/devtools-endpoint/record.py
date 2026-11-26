"""Record the devtools-endpoint box's goldens through its oracle adapter.

Needs the oracle daemon running (python3 -B oracle/harness/devtools-endpoint/daemon.py) and the adapter
built (make box BOX=devtools-endpoint builds it). Writes blobs to MANGANESE_STORE and replaces the box's
rows in goldens.lock.
"""

from __future__ import annotations

import hashlib
import json
import os
import pathlib
import struct
import subprocess
import zlib

root = pathlib.Path(__file__).resolve().parents[3]
ADAPTER = root / ".work/tmp/target/x86_64-unknown-linux-musl/debug/devtools-endpoint-oracle"
BROWSER = "00000000-0000-4000-8000-000000000001"
TARGETS = ["%032X" % n for n in (1, 2, 3)]
KEY = "dGhlIHNhbXBsZSBub25jZQ=="


def script(segments: list[tuple[bytes, int]], idle: int = 600) -> bytes:
    out = bytes([1]) + struct.pack(">H", len(segments))
    for data, pause in segments:
        out += struct.pack(">I", len(data)) + data + struct.pack(">H", pause)
    return out + struct.pack(">H", idle)


def get(path: str, host: str | None = "127.0.0.1:9221", method: str = "GET", version: str = "HTTP/1.1",
        extra: str = "") -> bytes:
    host_line = f"Host: {host}\r\n" if host is not None else ""
    return f"{method} {path} {version}\r\n{host_line}{extra}\r\n".encode()


def frame(opcode: int, payload: bytes, fin: bool = True, compressed: bool = False) -> bytes:
    mask = b"\x11\x22\x33\x44"
    head = bytes([(0x80 if fin else 0) | (0x40 if compressed else 0) | opcode])
    if len(payload) < 126:
        head += bytes([0x80 | len(payload)])
    elif len(payload) < 65536:
        head += bytes([0x80 | 126]) + struct.pack(">H", len(payload))
    else:
        head += bytes([0x80 | 127]) + struct.pack(">Q", len(payload))
    return head + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload))


def deflate_payload(payload: bytes, compressor) -> bytes:
    data = compressor.compress(payload) + compressor.flush(zlib.Z_SYNC_FLUSH)
    if not data.endswith(b"\x00\x00\xff\xff"):
        raise ValueError("sync flush did not end with the deflate tail")
    return data[:-4]


def compressed_frame(payload: bytes, compressor) -> bytes:
    return frame(1, deflate_payload(payload, compressor), compressed=True)


def upgrade(path: str, host: str = "127.0.0.1:9221", key: str = KEY, extra: str = "") -> bytes:
    return (f"GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n{extra}\r\n").encode()


def cdp(message: dict[str, object]) -> bytes:
    return frame(1, json.dumps(message, separators=(",", ":")).encode())


def corpus() -> list[tuple[str, str, bytes]]:
    items: list[tuple[str, str, bytes]] = []
    seen_inputs: set[bytes] = set()
    def add(lane: str, name: str, segments: list[tuple[bytes, int]], idle: int = 600) -> None:
        data = script(segments, idle)
        if data not in seen_inputs:
            seen_inputs.add(data)
            items.append((lane, name, data))

    hosts = {"ip-port": "127.0.0.1:9221", "localhost": "localhost", "localhost-port": "localhost:7000",
             "upper": "LOCALHOST", "ipv6": "[::1]:9221", "private": "10.1.2.3", "public-ip": "8.8.8.8:80",
             "name": "relay.example:4444", "dotted": "localhost.", "short-ip": "127.1", "empty": ""}
    for index, (name, host) in enumerate(hosts.items()):
        lane = "holdout" if index % 3 == 2 else "seen"
        add(lane, f"version-host-{name}", [(get("/json/version", host), 0)])
        add(lane, f"list-host-{name}", [(get("/json/list", host), 0)])
    add("seen", "version-no-host", [(get("/json/version", None), 0)])
    add("holdout", "list-no-host", [(get("/json", None), 0)])

    paths = ["/json/version", "/json", "/json/list", "/json/list?x=1", "/json/version/", "/json/version?x",
             "/JSON/VERSION", "//json/version", "/json/", "/", "/nothing", "/devtools/inspector.html",
             "/devtools/browser/" + BROWSER, "/devtools/page/" + TARGETS[0], "/json/%76ersion", "/json/new",
             "/json/activate", "/json/close"]
    for index, path in enumerate(paths):
        add("holdout" if index % 4 == 3 else "seen", f"path-{index}", [(get(path), 0)])

    methods = ["GET", "HEAD", "POST", "PUT", "DELETE", "OPTIONS", "PATCH", "get"]
    for index, method in enumerate(methods):
        lane = "holdout" if index % 3 == 1 else "seen"
        label = method.lower() if method.isupper() else f"lowercase-{method}"
        add(lane, f"method-version-{label}", [(get("/json/version", method=method), 0)])
        add(lane, f"method-list-{label}", [(get("/json/list", method=method), 0)])
        add(lane, f"method-new-{label}", [(get("/json/new?about:blank", method=method), 0)], 1500)

    for index, url in enumerate(["about:blank", "data:text/html,hi", "", "chrome://version/", "http://example.test/",
                                 "about:blank#x", "about%3Ablank"]):
        add("holdout" if index % 3 == 2 else "seen", f"new-{index}", [(get(f"/json/new?{url}", method="PUT"), 0)], 1500)
    for index, target in enumerate(TARGETS + ["nonexistent", "", "0" * 31]):
        lane = "holdout" if index % 3 == 1 else "seen"
        add(lane, f"activate-{index}", [(get(f"/json/activate/{target}"), 0)], 1000)
        add(lane, f"close-{index}", [(get(f"/json/close/{target}"), 0)], 1500)
        add(lane, f"close-put-{index}", [(get(f"/json/close/{target}", method="PUT"), 0)], 1500)

    for index, version in enumerate(["HTTP/1.0", "HTTP/1.2", "HTTP/2.0", "http/1.1", "HTTP/1"]):
        add("holdout" if index % 3 == 2 else "seen", f"version-{index}", [(get("/json/version", version=version), 0)])
    malformed = [b"garbage\r\n\r\n", b"GET /json/version\r\n\r\n", b"GET\r\n\r\n", b"\r\n\r\n",
                 b"GET /json/version HTTP/1.1\r\nHost 127.0.0.1\r\n\r\n", b"GET /json/version HTTP/1.1\n\n",
                 b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1\r\n", b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1",
                 b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1\r\nX: " + b"a" * 20000 + b"\r\n\r\n",
                 b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 5\r\n\r\nhello",
                 b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
                 b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1\r\nHost: localhost\r\n\r\n",
                 b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
                 b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: keep-alive\r\n\r\n",
                 b"\x16\x03\x01\x00\xa5\x01\x00\x00\xa1\x03\x03"]
    for index, data in enumerate(malformed):
        add("holdout" if index % 4 == 1 else "seen", f"malformed-{index}", [(data, 0)])

    request = get("/json/version")
    add("seen", "split-two", [(request[:7], 50), (request[7:], 0)])
    add("seen", "split-bytes", [(request[i:i + 1], 2) for i in range(len(request))])
    add("holdout", "split-three", [(request[:3], 30), (request[3:20], 30), (request[20:], 0)])
    add("seen", "pipelined-two", [(get("/json/version") + get("/json/list"), 0)])
    add("seen", "pipelined-three", [(get("/json/version") + get("/json/version", "localhost") + get("/nothing"), 0)])
    add("holdout", "pipelined-bad-host", [(get("/json/version") + get("/json/version", "example.test") + get("/json"), 0)])
    add("seen", "sequential", [(get("/json/version"), 200), (get("/json/list"), 0)])
    add("holdout", "sequential-close", [(get("/json/version", extra="Connection: close\r\n"), 200), (get("/json/list"), 0)])

    browser = f"/devtools/browser/{BROWSER}"
    page = f"/devtools/page/{TARGETS[0]}"
    add("seen", "ws-browser-handshake", [(upgrade(browser), 0)], 1000)
    add("holdout", "ws-page-handshake", [(upgrade(page), 0)], 1000)
    add("seen", "ws-unknown-browser", [(upgrade("/devtools/browser/" + "1" * 36), 0)], 1000)
    add("seen", "ws-unknown-page", [(upgrade("/devtools/page/" + "1" * 32), 0)], 1000)
    add("holdout", "ws-bad-host", [(upgrade(browser, host="relay.example"), 0)], 1000)
    add("holdout", "ws-other-key", [(upgrade(browser, key="AQIDBAUGBwgJCgsMDQ4PEA=="), 0)], 1000)
    add("seen", "ws-no-key", [(get(browser, extra="Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\n"), 0)], 1000)
    add("holdout", "ws-version-8", [(get(browser, extra=f"Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {KEY}\r\nSec-WebSocket-Version: 8\r\n"), 0)], 1000)
    add("holdout", "ws-protocol-header", [(upgrade(browser, extra="Sec-WebSocket-Protocol: cdp\r\n"), 0)], 1000)
    add("seen", "ws-extensions-header", [(upgrade(browser, extra="Sec-WebSocket-Extensions: permessage-deflate\r\n"), 0)], 1000)
    deflate_upgrade = upgrade(browser, extra="Sec-WebSocket-Extensions: permessage-deflate\r\n")
    reset_upgrade = upgrade(browser, extra="Sec-WebSocket-Extensions: permessage-deflate; server_no_context_takeover; client_no_context_takeover\r\n")
    get_version = json.dumps({"id": 1, "method": "Browser.getVersion"}, separators=(",", ":")).encode()
    get_version_again = json.dumps({"id": 2, "method": "Browser.getVersion"}, separators=(",", ":")).encode()
    add("seen", "ws-browser-deflate-get-version", [(deflate_upgrade, 300), (compressed_frame(get_version, zlib.compressobj(wbits=-15)), 100)], 1500)
    retained = zlib.compressobj(wbits=-15)
    add("holdout", "ws-browser-deflate-two-messages", [(deflate_upgrade, 300), (compressed_frame(get_version, retained), 100), (compressed_frame(get_version_again, retained), 100)], 1500)
    pieces = deflate_payload(get_version, zlib.compressobj(wbits=-15))
    cut = len(pieces) // 2
    add("seen", "ws-browser-deflate-fragmented", [(deflate_upgrade, 300), (frame(1, pieces[:cut], fin=False, compressed=True), 100), (frame(0, pieces[cut:]), 100)], 1500)
    add("holdout", "ws-browser-deflate-no-context-takeover", [(reset_upgrade, 300), (compressed_frame(get_version, zlib.compressobj(wbits=-15)), 100), (compressed_frame(get_version_again, zlib.compressobj(wbits=-15)), 100)], 1500)
    add("holdout", "ws-browser-compressed-without-offer", [(upgrade(browser), 300), (compressed_frame(get_version, zlib.compressobj(wbits=-15)), 100)], 1500)
    messages = {
        "get-version": [cdp({"id": 1, "method": "Browser.getVersion"})],
        "unknown-method": [cdp({"id": 2, "method": "Nope.nothing"})],
        "no-id": [cdp({"method": "Browser.getVersion"})],
        "string-id": [cdp({"id": "a", "method": "Browser.getVersion"})],
        "bad-json": [frame(1, b"{not json")],
        "empty-text": [frame(1, b"")],
        "binary": [frame(2, b'{"id":3,"method":"Browser.getVersion"}')],
        "ping": [frame(9, b"ping")],
        "close": [frame(8, struct.pack(">H", 1000))],
        "fragmented": [frame(1, b'{"id":4,"meth', fin=False), frame(0, b'od":"Browser.getVersion"}')],
        "two-messages": [cdp({"id": 5, "method": "Browser.getVersion"}), cdp({"id": 6, "method": "Target.getTargets"})],
        "params-error": [cdp({"id": 7, "method": "Target.activateTarget", "params": {}})],
        "large": [cdp({"id": 8, "method": "Browser.getVersion", "params": {"pad": "x" * 200000}})],
    }
    for index, (name, frames) in enumerate(messages.items()):
        lane = "holdout" if index % 4 == 2 else "seen"
        add(lane, f"ws-browser-{name}", [(upgrade(browser), 300)] + [(f, 100) for f in frames], 1500)
    for index, name in enumerate(["get-version", "unknown-method", "ping", "close"]):
        add("holdout" if index == 1 else "seen", f"ws-page-{name}", [(upgrade(page), 300)] + [(f, 100) for f in messages[name]], 1500)
    return items


def main() -> int:
    store = pathlib.Path(os.environ.get("MANGANESE_STORE", pathlib.Path.home() / ".local/share/manganese/store")) / "sha256"
    store.mkdir(parents=True, exist_ok=True)
    oracle = subprocess.Popen([str(ADAPTER)], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    oracle.stdin.write(b"MGB1")
    oracle.stdin.flush()
    if oracle.stdout.read(4) != b"MGB1":
        raise SystemExit("oracle adapter did not start")
    rows = []
    for lane, name, data in corpus():
        oracle.stdin.write(struct.pack(">I", len(data)) + data)
        oracle.stdin.flush()
        length = struct.unpack(">I", oracle.stdout.read(4))[0]
        result = oracle.stdout.read(length)
        if not result:
            raise SystemExit(f"oracle failed on {name}")
        for blob in (data, result):
            (store / hashlib.sha256(blob).hexdigest()).write_bytes(blob)
        rows.append(f"devtools-endpoint {lane} {name} {hashlib.sha256(data).hexdigest()} {len(data)} "
                    f"{hashlib.sha256(result).hexdigest()} {len(result)}")
        print(lane, name, len(result), flush=True)
    oracle.stdin.close()
    if oracle.wait():
        raise SystemExit("oracle adapter failed")
    lock = root / "goldens.lock"
    kept = [line for line in lock.read_text().splitlines() if not line.startswith("devtools-endpoint ")]
    lock.write_text("\n".join(kept + rows) + "\n")
    print("recorded", len(rows))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
