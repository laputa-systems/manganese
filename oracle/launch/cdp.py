"""A small Chrome DevTools Protocol client over RFC 6455 WebSockets. Standard library only."""

from __future__ import annotations

import base64
import json
import os
import socket
import struct
import time
import urllib.request
from collections import deque


class WebSocket:
    """A minimal RFC 6455 client: one text frame out, text frames in."""

    def __init__(self, url: str) -> None:
        if not url.startswith("ws://"):
            raise ValueError(f"not a ws:// URL: {url}")
        authority, _, path = url.removeprefix("ws://").partition("/")
        host, _, port = authority.rpartition(":")
        self.connection = socket.create_connection((host, int(port)), timeout=5)
        key = base64.b64encode(os.urandom(16)).decode()
        self.connection.sendall(
            (
                f"GET /{path} HTTP/1.1\r\nHost: {authority}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
            ).encode()
        )
        head = b""
        while b"\r\n\r\n" not in head:
            chunk = self.connection.recv(1)
            if not chunk:
                raise OSError("connection closed during the WebSocket handshake")
            head += chunk
        if not head.startswith(b"HTTP/1.1 101"):
            raise OSError(f"WebSocket upgrade refused: {head.split(b'\r\n', 1)[0]!r}")

    def send_text(self, text: str) -> None:
        payload = text.encode()
        mask = os.urandom(4)
        length = len(payload)
        if length < 126:
            prefix = struct.pack("!BB", 0x81, 0x80 | length)
        elif length < 1 << 16:
            prefix = struct.pack("!BBH", 0x81, 0x80 | 126, length)
        else:
            prefix = struct.pack("!BBQ", 0x81, 0x80 | 127, length)
        masked = bytes(byte ^ mask[index % 4] for index, byte in enumerate(payload))
        self.connection.sendall(prefix + mask + masked)

    def receive_text(self) -> str | None:
        """The next text message, or None when the peer closes."""
        try:
            first, second = self.exactly(2)
        except OSError:
            return None
        length = second & 0x7F
        if length == 126:
            (length,) = struct.unpack("!H", self.exactly(2))
        elif length == 127:
            (length,) = struct.unpack("!Q", self.exactly(8))
        payload = self.exactly(length)
        opcode = first & 0x0F
        if opcode == 0x8:
            return None
        return payload.decode() if opcode == 0x1 else ""

    def exactly(self, count: int) -> bytes:
        data = b""
        while len(data) < count:
            chunk = self.connection.recv(count - len(data))
            if not chunk:
                raise OSError("connection closed")
            data += chunk
        return data

    def close(self) -> None:
        self.connection.close()


class CdpError(RuntimeError):
    def __init__(self, method: str, error: dict[str, object]) -> None:
        super().__init__(f"{method}: {error.get('message')} ({error.get('code')})")
        self.error = error


class Cdp:
    """One browser-level CDP connection with flattened target sessions."""

    def __init__(self, websocket_url: str) -> None:
        self.socket = WebSocket(websocket_url)
        self.next_id = 0
        self.events: deque[dict[str, object]] = deque()

    @classmethod
    def connect(cls, endpoint: str = "http://127.0.0.1:9221") -> Cdp:
        with urllib.request.urlopen(f"{endpoint}/json/version", timeout=5) as response:
            return cls(json.load(response)["webSocketDebuggerUrl"])

    def send(self, method: str, params: dict[str, object] | None = None, session: str | None = None,
             timeout: float = 30.0) -> dict[str, object]:
        self.next_id += 1
        message: dict[str, object] = {"id": self.next_id, "method": method, "params": params or {}}
        if session is not None:
            message["sessionId"] = session
        self.socket.send_text(json.dumps(message))
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            reply = self._receive(deadline)
            if reply.get("id") == self.next_id:
                if "error" in reply:
                    raise CdpError(method, reply["error"])
                return reply.get("result", {})
            self.events.append(reply)
        raise TimeoutError(f"{method}: no reply in {timeout} s")

    def wait_event(self, method: str, session: str | None = None, timeout: float = 30.0) -> dict[str, object]:
        deadline = time.monotonic() + timeout
        while True:
            for event in list(self.events):
                if event.get("method") == method and (session is None or event.get("sessionId") == session):
                    self.events.remove(event)
                    return event.get("params", {})
            if time.monotonic() >= deadline:
                raise TimeoutError(f"no {method} in {timeout} s")
            self.events.append(self._receive(deadline))

    def _receive(self, deadline: float) -> dict[str, object]:
        self.socket.connection.settimeout(max(0.01, deadline - time.monotonic()))
        text = self.socket.receive_text()
        if text is None:
            raise ConnectionError("CDP connection closed")
        return json.loads(text) if text else {}

    def page(self, url: str = "about:blank") -> str:
        """Create a page target and return its flattened session ID."""
        target = self.send("Target.createTarget", {"url": url})["targetId"]
        return self.send("Target.attachToTarget", {"targetId": target, "flatten": True})["sessionId"]

    def evaluate(self, session: str, expression: str, timeout: float = 30.0) -> object:
        """Runtime.evaluate in the main world with awaitPromise, returning the value by value."""
        result = self.send("Runtime.evaluate", {"expression": expression, "awaitPromise": True,
                                                "returnByValue": True}, session=session, timeout=timeout)
        if "exceptionDetails" in result:
            raise CdpError("Runtime.evaluate", {"message": json.dumps(result["exceptionDetails"])[:2000]})
        return result["result"].get("value")

    def close(self) -> None:
        self.socket.close()
