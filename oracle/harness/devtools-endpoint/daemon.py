"""Serve the devtools-endpoint box's oracle: the frozen browser's DevTools endpoint, one box request at a time.

Listens on 127.0.0.1:47221. Each TCP connection carries one box request (four-byte big-endian length and
bytes) and receives one reply in the same framing: a closed flag byte, then the raw bytes the browser
sent. A request is version 1, a u16 segment count, each segment a u32 length, its bytes and a u16 pause
in milliseconds, then a u16 idle time that ends the read. Every request sees the browser as launched:
the daemon relaunches it after any request that may change its state and after a crash. The browser's
random identifiers are rewritten to fixed ones of the same length, in the order the targets are listed
(an identifier the request itself contained is echoed, not created, and stays as sent),
and fixed identifiers in a request are rewritten to the browser's own before it is sent.
"""

from __future__ import annotations

import base64
import json
import pathlib
import re
import socket
import struct
import subprocess
import sys
import threading

root = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(root / "oracle/launch"))
import launch  # noqa: E402

PORT = 47221
HERE = pathlib.Path(__file__).resolve().parent
BROWSER_ID = b"00000000-0000-4000-8000-000000000001"
TARGET_HEX = re.compile(rb"\b[0-9A-F]{32}\b")
MUTATING = (b"/json/new", b"/json/close/", b"/json/activate/", b"Upgrade:", b"upgrade:")


def decode(request: bytes) -> tuple[list[list[object]], int]:
    if not request or request[0] != 1:
        raise ValueError("unknown request version")
    count = struct.unpack_from(">H", request, 1)[0]
    offset = 3
    segments = []
    for _ in range(count):
        length = struct.unpack_from(">I", request, offset)[0]
        data = request[offset + 4:offset + 4 + length]
        if len(data) != length:
            raise ValueError("truncated segment")
        pause = struct.unpack_from(">H", request, offset + 4 + length)[0]
        segments.append([base64.b64encode(data).decode(), pause])
        offset += 6 + length
    idle = struct.unpack_from(">H", request, offset)[0]
    if offset + 2 != len(request):
        raise ValueError("trailing request bytes")
    return segments, idle


class Oracle:
    def __init__(self) -> None:
        self.archive = launch.resolve_archive(launch.new_run_dir("devtools-endpoint", "archive"), "x86_64", None, None)
        self.session: launch.Session | None = None
        self.browser: subprocess.Popen[bytes] | None = None
        self.exchange: subprocess.Popen[str] | None = None
        self.identifiers: dict[bytes, bytes] = {}
        self.created = 0
        self.dirty = True

    def stop(self) -> None:
        if self.exchange is not None:
            self.exchange.stdin.close()
            self.exchange.wait(timeout=30)
            self.exchange = None
        if self.browser is not None and self.browser.poll() is None:
            self.browser.kill()
            self.browser.wait()
        if self.session is not None:
            self.session.close()
        self.session = self.browser = None

    def start(self) -> None:
        self.stop()
        self.session = launch.open_session(launch.new_run_dir("devtools-endpoint", "headless"), self.archive)
        _, self.browser = self.session.start_browser("headless")
        if not self.session.probe()["readiness"]["ok"]:
            raise RuntimeError("oracle browser did not become ready")
        name = f"{self.session.name}-exchange"
        self.session.sidecars.append(name)
        self.exchange = subprocess.Popen(
            ["docker", "run", "-i", "--rm", "--name", name, "--network", f"container:{self.session.name}",
             "-v", f"{HERE}:/exchange:ro", launch.PROBE_IMAGE, "python3", "-B", "/exchange/exchange.py"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        version = json.loads(self.http(b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"))
        targets = json.loads(self.http(b"GET /json/list HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"))
        browser_id = version["webSocketDebuggerUrl"].rsplit("/", 1)[1].encode()
        self.identifiers = {browser_id: BROWSER_ID}
        for index, target in enumerate(targets, 1):
            self.identifiers[target["id"].encode()] = b"%032X" % index
        self.created = 0
        self.dirty = False

    def replay(self, segments: list[list[object]], idle: int) -> tuple[bool, bytes]:
        self.exchange.stdin.write(json.dumps({"segments": segments, "idle_ms": idle}) + "\n")
        self.exchange.stdin.flush()
        reply = json.loads(self.exchange.stdout.readline())
        if "error" in reply:
            raise RuntimeError(reply["error"])
        return reply["closed"], base64.b64decode(reply["raw"])

    def http(self, request: bytes) -> bytes:
        _, raw = self.replay([[base64.b64encode(request).decode(), 0]], 1000)
        return raw.split(b"\r\n\r\n", 1)[1]

    def rewrite(self, raw: bytes, sent: bytes) -> bytes:
        for actual, fixed in self.identifiers.items():
            raw = raw.replace(actual, fixed)
        def fresh(match: re.Match[bytes]) -> bytes:
            token = match.group(0)
            if token in self.identifiers:
                return self.identifiers[token]
            if token in self.identifiers.values() or token in sent:
                return token
            self.created += 1
            self.identifiers[token] = b"F%031X" % self.created
            return self.identifiers[token]
        return TARGET_HEX.sub(fresh, raw)

    def answer(self, request: bytes) -> bytes:
        segments, idle = decode(request)
        if self.dirty or self.browser is None or self.browser.poll() is not None:
            self.start()
        actual = {fixed: real for real, fixed in self.identifiers.items()}
        for segment in segments:
            data = base64.b64decode(str(segment[0]))
            for fixed, real in actual.items():
                data = data.replace(fixed, real)
            segment[0] = base64.b64encode(data).decode()
        sent = b"".join(base64.b64decode(str(data)) for data, _ in segments)
        self.dirty = any(marker in sent for marker in MUTATING)
        try:
            closed, raw = self.replay(segments, idle)
        except RuntimeError:
            self.start()
            closed, raw = self.replay(segments, idle)
        if self.browser.poll() is not None:
            self.dirty = True
        return bytes([closed]) + self.rewrite(raw, sent)


def read_exact(connection: socket.socket, size: int) -> bytes:
    data = b""
    while len(data) < size:
        chunk = connection.recv(size - len(data))
        if not chunk:
            raise ConnectionError("box client closed early")
        data += chunk
    return data


def main() -> int:
    oracle = Oracle()
    lock = threading.Lock()
    listener = socket.create_server(("127.0.0.1", PORT))
    print(f"devtools-endpoint oracle listening on 127.0.0.1:{PORT}", file=sys.stderr, flush=True)
    try:
        while True:
            connection, _ = listener.accept()
            with connection, lock:
                try:
                    request = read_exact(connection, struct.unpack(">I", read_exact(connection, 4))[0])
                    reply = oracle.answer(request)
                except (OSError, ValueError, RuntimeError, launch.LaunchError) as error:
                    print(f"devtools-endpoint oracle: request failed: {error}", file=sys.stderr, flush=True)
                    oracle.dirty = True
                    continue
                connection.sendall(struct.pack(">I", len(reply)) + reply)
    except KeyboardInterrupt:
        return 130
    finally:
        oracle.stop()


if __name__ == "__main__":
    raise SystemExit(main())
