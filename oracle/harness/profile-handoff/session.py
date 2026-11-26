"""Launch the frozen browser on an appliance-shaped profile and drive it through CDP.

The profile is copied in from, or out to, a directory the caller owns. The browser itself runs
with the appliance's launch switches (oracle/launch/appliance.py) in a container with no interface
besides loopback, and shuts down through Browser.close.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'oracle/launch'))
import launch

MODES = {'write', 'read'}


def copy_profile(source: Path, destination: Path) -> None:
    if destination.exists():
        shutil.rmtree(destination)
    shutil.copytree(source, destination)


def main() -> int:
    if len(sys.argv) not in (3, 5) or sys.argv[1] not in MODES:
        print(f'usage: session.py {{{"|".join(sorted(MODES))}}} PROFILE [KEY VALUE]', file=sys.stderr)
        return 2
    mode, profile = sys.argv[1], Path(sys.argv[2]).resolve()
    expectation = sys.argv[3:] if len(sys.argv) == 5 else []
    if mode != 'write' and not profile.is_dir():
        raise RuntimeError(f'profile missing: {profile}')

    run_dir = launch.new_run_dir('profile-handoff', mode)
    archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
    session = None
    browser: subprocess.Popen[bytes] | None = None
    cleanup_failure: Exception | None = None
    try:
        if mode != 'write':
            copy_profile(profile, run_dir / 'profile')
        session = launch.open_session(run_dir, archive)
        _, browser = session.start_browser('headless')
        witness = str(Path(__file__).resolve().parent / 'sidecar.py')
        output = session.sidecar(['python3', '-B', '/witness/sidecar.py', mode, *expectation],
                                 {Path(witness).parent: '/witness:ro'}, timeout=180)
        shutdown = session.probe('--close').get('shutdown')
        if not isinstance(shutdown, dict) or shutdown.get('ok') is not True or browser.wait(timeout=15) != 0:
            raise RuntimeError('browser shutdown failed')
        result = json.loads(output)
    finally:
        primary = sys.exc_info()[0]
        if browser is not None:
            try:
                if browser.poll() is None:
                    browser.kill()
                    browser.wait()
            except Exception as error:
                if primary is None:
                    cleanup_failure = error
                else:
                    print(f'browser cleanup: {type(error).__name__}: {error}', file=sys.stderr)
        if session is not None:
            try:
                session.close()
            except Exception as error:
                if primary is None and cleanup_failure is None:
                    cleanup_failure = error
                else:
                    print(f'session cleanup: {type(error).__name__}: {error}', file=sys.stderr)
    if cleanup_failure is not None:
        raise cleanup_failure
    if mode == 'write' and session is not None:
        copy_profile(session.profile, profile)
    print(json.dumps(result))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
