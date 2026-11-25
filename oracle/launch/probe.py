"""The supervisor's view of a running browser (docs/appliance-contract.md, clause 4).

Runs in a sidecar container that shares the browser container's network and PID namespaces, so
127.0.0.1:9221 and /proc are the browser's. Standard library only. Prints one JSON object.
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import sys
import time

import appliance
from cdp import WebSocket


def readiness() -> dict[str, object]:
    """Clause 4.1: poll every 2 ms for up to 60 s until GET /json/version starts with HTTP/1.1 200."""
    started = time.monotonic()
    attempts = 0
    last_error = ""
    request = (
        f"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:{appliance.DEVTOOLS_PORT}\r\nConnection: close\r\n\r\n"
    ).encode()
    while time.monotonic() - started < appliance.READINESS_DEADLINE_SECONDS:
        attempts += 1
        try:
            with socket.create_connection(
                ("127.0.0.1", appliance.DEVTOOLS_PORT), timeout=appliance.READINESS_CONNECT_TIMEOUT_SECONDS
            ) as connection:
                connection.settimeout(appliance.READINESS_READ_TIMEOUT_SECONDS)
                connection.sendall(request)
                head, body = read_response(connection)
            if head[:12] == b"HTTP/1.1 200":
                return {
                    "ok": True,
                    "seconds": round(time.monotonic() - started, 3),
                    "attempts": attempts,
                    "status_line": head.split(b"\r\n", 1)[0].decode(),
                    "content_length": header(head, "content-length"),
                    "version": json.loads(body),
                }
            last_error = f"status {head[:12]!r}"
        except (OSError, ValueError) as error:
            last_error = str(error)
        time.sleep(appliance.READINESS_POLL_SECONDS)
    return {"ok": False, "seconds": round(time.monotonic() - started, 3), "attempts": attempts, "error": last_error}


def read_response(connection: socket.socket) -> tuple[bytes, bytes]:
    """The response head and its Content-Length body; the server need not close the connection."""
    data = b""
    while b"\r\n\r\n" not in data:
        chunk = connection.recv(65536)
        if not chunk:
            break
        data += chunk
    head, _, body = data.partition(b"\r\n\r\n")
    length = header(head, "content-length")
    while length is not None and len(body) < int(length):
        chunk = connection.recv(65536)
        if not chunk:
            break
        body += chunk
    return head, body


def header(head: bytes, name: str) -> str | None:
    for line in head.split(b"\r\n")[1:]:
        key, _, value = line.partition(b":")
        if key.strip().lower() == name.encode():
            return value.strip().decode()
    return None


def processes() -> list[dict[str, object]]:
    found = []
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            with open(f"/proc/{entry}/cmdline", "rb") as stream:
                cmdline = stream.read()
            with open(f"/proc/{entry}/status", encoding="utf-8") as stream:
                status = dict(line.split(":\t", 1) for line in stream.read().splitlines() if ":\t" in line)
        except OSError:
            continue
        found.append(
            {
                "pid": int(entry),
                "argv": [part.decode(errors="replace") for part in cmdline.split(b"\0") if part],
                "uid": status.get("Uid", "").split(),
                "gid": status.get("Gid", "").split(),
                "state": status.get("State", ""),
                "no_new_privs": status.get("NoNewPrivs", "").strip(),
                "seccomp": status.get("Seccomp", "").strip(),
                "nspid": status.get("NSpid", "").split(),
            }
        )
    return sorted(found, key=lambda item: item["pid"])


def browser_processes() -> list[dict[str, object]]:
    return [item for item in processes() if item["uid"][:1] == [str(appliance.UID)]]


def sandbox_witness(deadline: float) -> dict[str, object]:
    """Clause 4.2: the first process whose space-split cmdline has the word --type=renderer.

    Readiness needs it too, so the probe keeps scanning until the readiness deadline.
    """
    while True:
        for item in processes():
            words = " ".join(item["argv"]).split(" ")
            if "--type=renderer" in words:
                ok = item["no_new_privs"] == "1" and item["seccomp"] == "2" and len(item["nspid"]) > 1
                return {"ok": ok, **{key: item[key] for key in ("pid", "no_new_privs", "seccomp", "nspid")}}
        if time.monotonic() >= deadline:
            return {"ok": False, "error": "no renderer process"}
        time.sleep(appliance.READINESS_POLL_SECONDS)


def environment_of(pid: int) -> dict[str, str]:
    with open(f"/proc/{pid}/environ", "rb") as stream:
        pairs = [part.decode(errors="replace") for part in stream.read().split(b"\0") if part]
    return dict(pair.split("=", 1) for pair in pairs if "=" in pair)


def browser_root() -> dict[str, object]:
    """The browser process the supervisor started, whether directly or through the compositor."""
    for item in browser_processes():
        if item["argv"] and item["argv"][0] == f"{appliance.INSTALL_ROOT}/chrome":
            if any(arg.startswith("--type=") for arg in item["argv"]):
                continue
            return {"pid": item["pid"], "argv": item["argv"], "uid": item["uid"], "gid": item["gid"],
                    "environment": environment_of(item["pid"]),
                    "executable": os.readlink(f"/proc/{item['pid']}/exe")}
    raise RuntimeError("no browser process")


def shutdown(websocket_url: str) -> dict[str, object]:
    """Clause 4.3: Browser.close over the browser WebSocket; the whole tree must exit within 8 s."""
    started = time.monotonic()
    socket_ = WebSocket(websocket_url)
    socket_.send_text('{"id":1,"method":"Browser.close"}')
    reply = None
    try:
        reply = socket_.receive_text()
    except OSError:
        pass
    socket_.close()
    survivors: list[dict[str, object]] = []
    while time.monotonic() - started < appliance.CLOSE_DEADLINE_SECONDS:
        survivors = browser_processes()
        if not survivors:
            return {"ok": True, "seconds": round(time.monotonic() - started, 3), "reply": reply, "survivors": []}
        time.sleep(0.01)
    return {"ok": False, "seconds": round(time.monotonic() - started, 3), "reply": reply,
            "survivors": [{"pid": item["pid"], "argv": item["argv"][:1]} for item in survivors]}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--close", action="store_true", help="send Browser.close after the witnesses")
    args = parser.parse_args()
    deadline = time.monotonic() + appliance.READINESS_DEADLINE_SECONDS
    report: dict[str, object] = {"readiness": readiness()}
    if report["readiness"]["ok"]:
        report["sandbox"] = sandbox_witness(deadline)
        report["browser"] = browser_root()
        report["processes"] = [{"pid": item["pid"], "argv": item["argv"]} for item in browser_processes()]
        if args.close:
            report["shutdown"] = shutdown(report["readiness"]["version"]["webSocketDebuggerUrl"])
    json.dump(report, sys.stdout)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
