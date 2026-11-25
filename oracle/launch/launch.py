#!/usr/bin/env python3
"""Run a browser archive the way the appliance does and check clauses 1.7, 1.8 and 2 to 4.

The browser runs in a pristine Alpine container with no network interface besides loopback, as
user browser (uid 1000, gid 27), with Docker's seccomp profile disabled so the browser installs its
own sandbox. A sidecar container sharing its network and PID namespaces plays the supervisor's
readiness, sandbox and shutdown roles (probe.py). Writes a JSON report and exits 1 if any clause fails.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

import appliance

ROOT = Path(__file__).resolve().parents[2]
LAUNCH = ROOT / "oracle/launch"
LOCK = ROOT / "oracle/frozen/artifacts.lock"
VERIFIER = ROOT / "oracle/build/scripts/install-runtime.py"
BASE_IMAGE = "alpine@sha256:d56c381f961d307a21b3ca004cf1e3910f106644aefb1f43e654c8a56c4fd395"
PROBE_IMAGE = "manganese-launch-probe:1"
TINYLAND_REV = "4796621b96537bb78232e3d01ca0e96297216ffd"
TINYLAND_SOURCE = ROOT / ".work/tmp" / f"tinyland-{TINYLAND_REV}"
COMPOSITOR = ROOT / ".work/tmp/compositor" / TINYLAND_REV
XKB = Path("/usr/share/X11/xkb")
ARCHES = {"x86_64": "amd64", "aarch64": "arm64"}


class LaunchError(RuntimeError):
    pass


@dataclass(frozen=True)
class Archive:
    """An archive with its two sidecars beside it, under the name the appliance matches (1.1)."""

    path: Path
    identity: str

    @property
    def root_name(self) -> str:
        return self.path.name.removesuffix(".tar.xz")


def run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, text=True, capture_output=True, **kwargs)


def checked(command: list[str], **kwargs: object) -> str:
    result = run(command, **kwargs)
    if result.returncode != 0:
        raise LaunchError(f"{' '.join(command[:3])} failed ({result.returncode}): {result.stderr.strip()[-2000:]}")
    return result.stdout


def frozen_archive(tag: str, run_dir: Path) -> Archive:
    """Fetch the three files of one frozen OCI tag (artifacts.lock) and stage them by file name."""
    names = []
    for line in LOCK.read_text(encoding="utf-8").splitlines():
        fields = line.split()
        if line.startswith("#") or len(fields) != 4:
            continue
        if fields[3].split(":", 1)[1] == tag:
            names.append(fields[2])
    archives = [name for name in names if name.endswith(".tar.xz")]
    if len(archives) != 1:
        raise LaunchError(f"{tag} names {len(archives)} archives in {LOCK}, want 1")
    staged = run_dir / "archive"
    staged.mkdir(parents=True)
    for name in names:
        stored = Path(checked(["sh", str(ROOT / "scripts/artifact"), "fetch", name]).strip())
        try:
            os.link(stored, staged / name)
        except OSError:
            shutil.copyfile(stored, staged / name)
    return Archive(staged / archives[0], f"frozen:{tag}")


def given_archive(path: Path) -> Archive:
    for suffix in ("", ".sha256", ".acceptance.json"):
        if not Path(str(path) + suffix).is_file():
            raise LaunchError(f"missing {path}{suffix}")
    return Archive(path.resolve(), f"file:{path.resolve()}")


def verify(archive: Archive, run_dir: Path) -> tuple[Path, dict[str, object]]:
    """Clause 1.7: the frozen verifier accepts the archive and extracts it."""
    destination = run_dir / "browser"
    result = run(["python3", "-B", str(VERIFIER), "--archive", str(archive.path), "--verify-extract", str(destination)])
    clause = {"ok": result.returncode == 0, "returncode": result.returncode, "stderr": result.stderr.strip()}
    return destination, clause


def ensure_probe_image() -> None:
    if run(["docker", "image", "inspect", PROBE_IMAGE]).returncode == 0:
        return
    checked(["docker", "build", "-q", "--build-arg", f"BASE_IMAGE={BASE_IMAGE}", "-t", PROBE_IMAGE,
             "-f", str(LAUNCH / "probe.Dockerfile"), str(LAUNCH)])


def ensure_abi_image() -> None:
    if run(["docker", "image", "inspect", "manganese-abi:1"]).returncode == 0:
        return
    directory = ROOT / "oracle/probes/abi"
    checked(["docker", "build", "-q", "--build-arg", f"BASE_IMAGE={BASE_IMAGE}", "-t", "manganese-abi:1",
             "-f", str(directory / "Dockerfile"), str(directory)])


def ensure_compositor() -> Path:
    binary = COMPOSITOR / "tinyland"
    host = COMPOSITOR / "remote-host"
    host_inputs = [LAUNCH / "remote-host/Cargo.toml", LAUNCH / "remote-host/Cargo.lock",
                   LAUNCH / "remote-host/src/main.rs"]
    if binary.is_file() and host.is_file() and all(host.stat().st_mtime_ns >= p.stat().st_mtime_ns for p in host_inputs):
        return host
    COMPOSITOR.mkdir(parents=True, exist_ok=True)
    if not (TINYLAND_SOURCE / "Cargo.toml").is_file():
        TINYLAND_SOURCE.mkdir(parents=True, exist_ok=True)
        archive = subprocess.run(["git", "-C", str(Path.home() / "repos/laputa-systems/tinyland"), "archive", TINYLAND_REV],
                                 capture_output=True, check=True).stdout
        subprocess.run(["tar", "-x", "-C", str(TINYLAND_SOURCE)], input=archive, check=True)
    environment = {**os.environ, "RUSTC_WRAPPER": "kache",
                   "RUSTFLAGS": "-C target-feature=+crt-static -C link-arg=-fuse-ld=mold",
                   "CARGO_TARGET_DIR": str(ROOT / ".work/tmp/tinyland-target")}
    cargo = str(Path.home() / ".cargo/bin/cargo")
    checked([cargo, "+nightly-2026-11-09", "build", "--locked", "--target", "x86_64-unknown-linux-musl", "-p", "tinyland"],
            cwd=TINYLAND_SOURCE, env=environment)
    shutil.copyfile(ROOT / ".work/tmp/tinyland-target/x86_64-unknown-linux-musl/debug/tinyland", binary)
    binary.chmod(0o755)
    checked([cargo, "+nightly-2026-11-09", "build", "--locked", "--target", "x86_64-unknown-linux-musl", "--manifest-path",
             str(LAUNCH / "remote-host/Cargo.toml")], env=environment)
    shutil.copyfile(ROOT / ".work/tmp/tinyland-target/x86_64-unknown-linux-musl/debug/manganese-remote-host", host)
    host.chmod(0o755)
    return host


class Session:
    """One browser container: the image root, /opt/browser, /profile, Downloads and a tmpfs /tmp."""

    def __init__(self, run_dir: Path, browser_root: Path, name: str) -> None:
        self.name = name
        self.run_dir = run_dir
        self.sidecars: list[str] = []
        self.tracers: list[str] = []
        self.trace_dirs: list[Path] = []
        self.profile = run_dir / "profile"
        self.downloads = run_dir / "downloads"
        for directory in (self.profile, self.downloads):
            directory.mkdir(exist_ok=True)
        if os.getuid() != appliance.UID:
            raise LaunchError(f"run as uid {appliance.UID} so /profile is owned by the browser user")
        COMPOSITOR.mkdir(parents=True, exist_ok=True)
        checked([
            "docker", "run", "-d", "--rm", "--init", "--name", name, "--network", "none",
            "--security-opt", "seccomp=unconfined", "--tmpfs", "/tmp:exec,mode=1777",
            "-v", f"{browser_root}:{appliance.INSTALL_ROOT}:ro",
            "-v", f"{COMPOSITOR}:/opt/compositor:ro",
            *(["-v", f"{XKB}:/opt/xkb:ro"] if XKB.is_dir() else []),
            "-v", f"{self.profile}:{appliance.PROFILE}",
            "-v", f"{self.downloads}:{appliance.DOWNLOADS}",
            BASE_IMAGE, "sleep", "infinity",
        ])
        self.root(
            f"adduser -D -H -u {appliance.UID} -G video -h {appliance.HOME} {appliance.USER}"
            f" && chown {appliance.UID}:{appliance.GID} {appliance.HOME}"
            f" && mkdir -p {Path(appliance.LAUNCHER_LINK).parent} /run/user/{appliance.UID}"
            f" && chown {appliance.UID}:{appliance.GID} /run/user/{appliance.UID}"
            f" && chmod 700 /run/user/{appliance.UID}"
            f" && ln -s {appliance.INSTALL_ROOT}/chrome {appliance.LAUNCHER_LINK}"
        )

    def root(self, script: str) -> str:
        return checked(["docker", "exec", self.name, "sh", "-c", script])

    def as_browser(self, command: list[str], environment: dict[str, str]) -> list[str]:
        """A docker exec command line that runs COMMAND as the browser user with exactly ENVIRONMENT."""
        return ["docker", "exec", "-i", "-u", f"{appliance.UID}:{appliance.GID}", "-w", appliance.HOME,
                self.name, "env", "-i", *(f"{k}={v}" for k, v in environment.items()), *command]

    def resolve_launcher(self) -> str:
        return self.root(f"readlink -f {appliance.LAUNCHER_LINK}").strip()

    def start_browser(self, mode: str, extra: tuple[str, ...] = (), start_url: str = appliance.START_URL,
                      ) -> tuple[list[str], subprocess.Popen[bytes]]:
        """Start the browser as the supervisor does; stdout and stderr append to the run's browser.log (2.5)."""
        command = appliance.argv(self.resolve_launcher(), mode, start_url, extra)
        trace = os.environ.get("MANGANESE_ABI_TRACE")
        if trace:
            ensure_abi_image()
            (self.downloads / ".abi-pid").unlink(missing_ok=True)
        actual = (["sh", "-c", 'echo $$ > /home/browser/Downloads/.abi-pid; kill -STOP $$; unset SHLVL PWD; exec "$@"', "sh", *command]
                  if trace else command)
        with (self.run_dir / "browser.log").open("ab") as log:
            if mode == "headed":
                if not XKB.is_dir():
                    raise LaunchError(f"XKB data missing: {XKB}")
                host = ensure_compositor()
                environment = {**appliance.ENVIRONMENT, "XDG_RUNTIME_DIR": "/run/user/1000",
                               "XKB_CONFIG_ROOT": "/opt/xkb", "TINYLAND_XKB_ROOT": "/opt/xkb"}
                compositor = ["/opt/compositor/tinyland", "--remote", "--width", "1024", "--height", "768",
                              "--socket", "wayland-1", "--", *actual]
                browser = subprocess.Popen([str(host), str(self.run_dir), *self.as_browser(compositor, environment)],
                                           stdin=subprocess.DEVNULL, stdout=log, stderr=log)
            else:
                browser = subprocess.Popen(self.as_browser(actual, appliance.ENVIRONMENT), stdin=subprocess.DEVNULL,
                                           stdout=log, stderr=log)
        if trace:
            directory = Path(trace).resolve() / f"{self.name}-{time.time_ns()}"
            directory.mkdir(parents=True)
            self.sidecar(["sh", "-c", 'while [ ! -s /downloads/.abi-pid ]; do sleep .1; done; '
                          'p=$(cat /downloads/.abi-pid); strace -f -qq -y --decode-pids=pidns -s 256 -ff -o /trace/trace -p "$p" & '
                          't=$!; echo "$t" > /trace/tracer-pid; while [ "$(awk \'/TracerPid/ {print $2}\' /proc/$p/status)" = 0 ]; do '
                          'kill -0 "$t" || exit 1; sleep .1; done; '
                          'kill -CONT "$p"; echo ready > /trace/ready; wait "$t"; '
                          'status=$?; echo "$status" > /trace/status; exit "$status"'],
                         {self.downloads: "/downloads", directory: "/trace"}, image="manganese-abi:1", detach=True)
            self.tracers.append(self.sidecars[-1])
            self.trace_dirs.append(directory)
            deadline = time.monotonic() + 30
            while not (directory / "ready").is_file():
                if time.monotonic() >= deadline or browser.poll() is not None:
                    raise LaunchError("ABI tracer did not attach to the browser")
                time.sleep(0.05)
        return command, browser

    def sidecar(self, command: list[str], mounts: dict[Path, str] | None = None, image: str = PROBE_IMAGE,
                detach: bool = False, timeout: float | None = None) -> str:
        """Run COMMAND in a container sharing the browser's network and PID namespaces.

        oracle/launch is mounted at /launch. Detached sidecars are removed with the session.
        """
        volumes = [item for host, target in {LAUNCH: "/launch:ro", **(mounts or {})}.items()
                   for item in ("-v", f"{host}:{target}")]
        name = f"{self.name}-sidecar-{len(self.sidecars)}"
        self.sidecars.append(name)
        return checked([
            "docker", "run", *(["-d"] if detach else []), "--rm", "--name", name, "--cap-add", "SYS_PTRACE",
            "--network", f"container:{self.name}", "--pid", f"container:{self.name}", *volumes, image, *command,
        ], timeout=timeout)

    def probe(self, *args: str) -> dict[str, object]:
        result = json.loads(self.sidecar(["python3", "-B", "/launch/probe.py", *args]))
        if os.environ.get("MANGANESE_ABI_TRACE") and "--close" in args:
            (self.run_dir / "abi-shutdown.json").write_text(json.dumps(result))
        return result

    def close(self) -> None:
        if self.tracers:
            for tracer, directory in zip(self.tracers, self.trace_dirs):
                pidfile = directory / "tracer-pid"
                if pidfile.is_file() and not (directory / "status").is_file():
                    pid = int(pidfile.read_text())
                    self.root('for p in /proc/[0-9]*; do '
                              f'[ "$(awk \'/TracerPid/ {{print $2}}\' "$p/status" 2>/dev/null)" = "{pid}" ] '
                              '&& kill -KILL "${p##*/}" 2>/dev/null || true; done')
                try:
                    run(["docker", "wait", tracer], timeout=15)
                except subprocess.TimeoutExpired:
                    raise LaunchError(f"ABI tracer did not finish: {tracer}")
        run(["docker", "rm", "-f", *self.sidecars, self.name])


def version_probe(session: Session) -> dict[str, object]:
    """Clause 1.8: chrome --version with only HOME and LD_LIBRARY_PATH, directly and through musl's loader."""
    environment = {"HOME": appliance.HOME, "LD_LIBRARY_PATH": f"{appliance.INSTALL_ROOT}/lib"}
    loader = f"/lib/ld-musl-{platform.machine()}.so.1"
    chrome = f"{appliance.INSTALL_ROOT}/chrome"
    forms = {}
    for label, command in (("direct", [chrome, "--version"]), ("loader", [loader, chrome, "--version"])):
        result = run(session.as_browser(command, environment), stdin=subprocess.DEVNULL, timeout=60)
        forms[label] = {"returncode": result.returncode, "stdout": result.stdout.strip()}
    ok = all(form["returncode"] == 0 and form["stdout"] == "Chromium 156.0.8078.79" for form in forms.values())
    return {"ok": ok, **forms}


def launch(session: Session, mode: str) -> tuple[dict[str, object], dict[str, object]]:
    """Start the browser as the supervisor does, then run the supervisor's probes and shutdown."""
    executable = session.resolve_launcher()
    install = {"ok": Path(executable).name == "chrome", "launcher": appliance.LAUNCHER_LINK, "resolved": executable}
    started = time.monotonic()
    command, browser = session.start_browser(mode)
    try:
        if mode == "headed":
            observed = session.probe()
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                frame = session.run_dir / "frame.rgba"
                if frame.is_file():
                    pixels = frame.read_bytes()
                    if len(pixels) >= 8 and any(pixels[i:i + 4] != pixels[:4] for i in range(4, len(pixels), 4)):
                        (session.run_dir / "visible-frame.rgba").write_bytes(pixels)
                        break
                if browser.poll() is not None:
                    break
                time.sleep(0.1)
            if observed["readiness"]["ok"]:
                observed["shutdown"] = session.probe("--close").get("shutdown", {"ok": False})
        else:
            observed = session.probe("--close")
        try:
            returncode = browser.wait(timeout=appliance.CLOSE_DEADLINE_SECONDS + 7)
        except subprocess.TimeoutExpired:
            browser.kill()
            returncode = None
    finally:
        if browser.poll() is None:
            browser.kill()
    observed["wall_seconds"] = round(time.monotonic() - started, 3)
    observed["returncode"] = returncode
    observed["command"] = command
    observed["mode"] = mode
    if mode == "headed":
        frame = session.run_dir / "frame.rgba"
        visible = session.run_dir / "visible-frame.rgba"
        pixels = visible.read_bytes() if visible.is_file() else b""
        size = (session.run_dir / "frame-size").read_text().strip() if pixels else ""
        observed["frame"] = {"ok": len(pixels) >= 8 and any(pixels[i:i + 4] != pixels[:4]
                             for i in range(4, len(pixels), 4)), "bytes": len(pixels), "size": size,
                             "path": str(visible), "last_frame": str(frame)}
        log = (session.run_dir / "browser.log").read_text(errors="replace")
        observed["gpu_log"] = [line for line in log.splitlines() if "gpu" in line.lower() and "TINYLAND_SPAWN_COMMAND" not in line]
        observed["compositor_ready"] = "TINYLAND_COMPOSITOR_READY" in log
    return install, observed


def clauses(observed: dict[str, object], install: dict[str, object]) -> dict[str, dict[str, object]]:
    readiness = observed["readiness"]
    result: dict[str, dict[str, object]] = {"2.1": install}
    if not readiness["ok"]:
        result["4.1"] = {"ok": False, **readiness}
        return result
    browser = observed["browser"]
    environment = browser["environment"]
    result["2.2"] = {
        "ok": browser["uid"][:1] == [str(appliance.UID)] and browser["gid"][:1] == [str(appliance.GID)]
        and ((observed["mode"] == "headless" and environment == appliance.ENVIRONMENT)
             or (observed["mode"] == "headed"
                 and all(environment.get(k) == v for k, v in appliance.ENVIRONMENT.items())
                 and environment.get("XDG_RUNTIME_DIR") == "/run/user/1000")),
        "uid": browser["uid"], "gid": browser["gid"], "environment": environment,
    }
    result["3.1-3.3"] = {
        "ok": browser["argv"] == observed["command"] and browser["argv"][-1] == appliance.START_URL,
        "argv": browser["argv"],
    }
    version = readiness["version"]
    result["4.1"] = {
        "ok": readiness["status_line"].startswith("HTTP/1.1 200") and readiness["content_length"] is not None,
        "seconds": readiness["seconds"], "attempts": readiness["attempts"],
        "status_line": readiness["status_line"], "content_length": readiness["content_length"],
        "version": version,
    }
    if observed["mode"] == "headed":
        result["3.1-3.3"]["ok"] = result["3.1-3.3"]["ok"] and observed["compositor_ready"]
    result["4.2"] = observed["sandbox"]
    shutdown = observed.get("shutdown", {"ok": False, "error": "not attempted"})
    result["4.3"] = {**shutdown, "ok": shutdown["ok"] and observed["returncode"] == 0,
                     "returncode": observed["returncode"]}
    return result


def new_run_dir(kind: str, label: str) -> Path:
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    run_dir = ROOT / ".work/tmp" / kind / f"{stamp}-{label}-{os.getpid()}"
    run_dir.mkdir(parents=True)
    return run_dir


def resolve_archive(run_dir: Path, arch: str, archive: Path | None, frozen: str | None) -> Archive:
    """An explicit archive path, or a frozen OCI tag (default hydrogen-r2-<arch>)."""
    if archive is not None:
        return given_archive(archive)
    return frozen_archive(frozen or f"hydrogen-r2-{ARCHES[arch]}", run_dir)


def open_session(run_dir: Path, archive: Archive) -> Session:
    """Verify and extract ARCHIVE (1.7) and start its container; raises LaunchError if verification fails."""
    browser_root, verified = verify(archive, run_dir)
    if not verified["ok"]:
        raise LaunchError(f"the frozen verifier rejected {archive.path.name}: {verified['stderr']}")
    ensure_probe_image()
    return Session(run_dir, browser_root, f"manganese-{run_dir.parent.name}-{os.getpid()}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument("--arch", choices=sorted(ARCHES), default=platform.machine())
    parser.add_argument("--mode", choices=("headless", "headed"), default="headless")
    source = parser.add_mutually_exclusive_group()
    source.add_argument("--archive", type=Path, help="an archive with its .sha256 and .acceptance.json beside it")
    source.add_argument("--frozen", help="an artifacts.lock OCI tag (default hydrogen-r2-<arch>)")
    parser.add_argument("--report", type=Path, help="report path (default: the run directory's report.json)")
    args = parser.parse_args()
    if args.arch != platform.machine():
        print(f"oracle-launch: {args.arch} needs a native {args.arch} host; emulation is not evidence", file=sys.stderr)
        return 2
    run_dir = new_run_dir("launch", f"{args.arch}-{args.mode}")
    report_path = args.report or run_dir / "report.json"
    report: dict[str, object] = {"schema": 1, "arch": args.arch, "mode": args.mode, "run_dir": str(run_dir)}
    session = None
    try:
        archive = resolve_archive(run_dir, args.arch, args.archive, args.frozen)
        report["archive"] = {"identity": archive.identity, "name": archive.path.name}
        browser_root, verified = verify(archive, run_dir)
        report["clauses"] = {"1.7": verified}
        if verified["ok"]:
            ensure_probe_image()
            session = Session(run_dir, browser_root, f"manganese-launch-{os.getpid()}")
            report["clauses"]["1.8"] = version_probe(session)
            install, observed = launch(session, args.mode)
            report["clauses"].update(clauses(observed, install))
            report["processes"] = observed.get("processes", [])
            report["wall_seconds"] = observed["wall_seconds"]
            if args.mode == "headed":
                report["frame"] = observed["frame"]
                report["gpu_log"] = observed["gpu_log"]
                report["compositor_ready"] = observed["compositor_ready"]
    except (LaunchError, OSError, subprocess.SubprocessError, json.JSONDecodeError) as error:
        report["error"] = str(error)
    finally:
        if session is not None:
            session.close()
    report["ok"] = "error" not in report and (args.mode != "headed" or report.get("frame", {}).get("ok", False)) and all(item["ok"] for item in report.get("clauses", {}).values())
    report_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    for clause, item in report.get("clauses", {}).items():
        print(f"{'ok  ' if item['ok'] else 'FAIL'} {clause}")
    if "error" in report:
        print(f"oracle-launch: {report['error']}", file=sys.stderr)
    print(f"oracle-launch: report {report_path}")
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
