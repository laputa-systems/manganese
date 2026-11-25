"""Replay connection scripts against the browser's DevTools endpoint; runs in a sidecar sharing its network.

Reads one JSON object per line: segments (base64 bytes and the pause after them in milliseconds) and the
idle time that ends the read. Writes one JSON object per line: the raw bytes received and whether the
server closed the connection. A server that closes while segments remain unsent is not an error: the replay
stops sending and reads what the server left.
"""

import base64
import json
import socket
import sys
import time

PORT = 9221


def replay(segments: list[list[object]], idle_ms: int) -> dict[str, object]:
    connection = socket.create_connection(("127.0.0.1", PORT), timeout=5)
    received = b""
    closed = False
    try:
        for data, pause in segments:
            try:
                connection.sendall(base64.b64decode(str(data)))
            except (BrokenPipeError, ConnectionResetError):
                break
            if int(pause):
                time.sleep(int(pause) / 1000)
        connection.settimeout(idle_ms / 1000)
        while True:
            try:
                chunk = connection.recv(1 << 20)
            except TimeoutError:
                break
            except ConnectionResetError:
                closed = True
                break
            if not chunk:
                closed = True
                break
            received += chunk
    finally:
        connection.close()
    return {"raw": base64.b64encode(received).decode(), "closed": closed}


def main() -> int:
    for line in sys.stdin:
        request = json.loads(line)
        try:
            reply = replay(request["segments"], int(request["idle_ms"]))
        except OSError as error:
            reply = {"error": str(error)}
        sys.stdout.write(json.dumps(reply) + "\n")
        sys.stdout.flush()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
