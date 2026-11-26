import os
import pathlib
import shutil
import signal
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
HARNESS = pathlib.Path(__file__).resolve().parent
WORK = ROOT / '.work/tmp/fuzzdriver-selftest' / str(os.getpid())
PEER = WORK / 'peer.py'
DRIVER = WORK / 'driver.py'

PEER_SOURCE = '''\
import os
import struct
import sys
import time
mode, pidfile = sys.argv[1], sys.argv[2]
open(pidfile, 'w').write(str(os.getpid()))
out = sys.stdout.buffer
inp = sys.stdin.buffer
out.write(b'MGB1')
out.flush()
inp.read(4)
if mode == 'deaf':
    while True:
        time.sleep(3600)
if mode == 'extra':
    header = inp.read(4)
    length = struct.unpack('>I', header)[0]
    data = inp.read(length)
    out.write(struct.pack('>I', length) + data)
    out.flush()
    while inp.read(1):
        pass
    out.write(b'\\000\\000\\000\\001X')
    out.flush()
else:
    while True:
        header = inp.read(4)
        if len(header) < 4:
            break
        length = struct.unpack('>I', header)[0]
        data = inp.read(length)
        if len(data) != length:
            break
        out.write(struct.pack('>I', length) + data)
        out.flush()
'''

DRIVER_SOURCE = '''\
import os
import sys
sys.path.insert(0, os.environ['FUZZDRIVER_PATH'])
import fuzzdriver
mode = os.environ['DRIVER_MODE']
def generate(oracle):
    return b'A' * (1 << 20) if mode == 'deaf' else b'hello'
fuzzdriver.run('control', int(os.environ['SEED']), int(os.environ['CASES']), generate)
'''


def alive(pid):
    result = subprocess.run(['ps', '-o', 'stat=', '-p', str(pid)], capture_output=True, text=True)
    if result.returncode != 0:
        return False
    state = result.stdout.strip().split()
    return bool(state) and not state[0].startswith('Z')


def cleanup(pids):
    for pid in pids:
        if alive(pid):
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


def pids_from(paths):
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline and any(not path.exists() for path in paths):
        time.sleep(0.02)
    return [int(path.read_text().strip()) for path in paths if path.exists()]


def wrapper(name, mode, pidfile):
    path = WORK / name
    path.write_text(f"#!/bin/sh\nexec python3 -B {PEER} {mode} {pidfile}\n")
    path.chmod(0o755)
    return path


def start(oracle, candidate, seed, env):
    environment = dict(os.environ)
    environment.update({
        'FUZZ_ORACLE': str(oracle),
        'FUZZ_CANDIDATE': str(candidate),
        'FUZZ_RESULT': str(WORK / f'result-{seed}'),
        'FUZZDRIVER_PATH': str(HARNESS),
        'SEED': seed,
        'CASES': '1',
        'PEER_MAX_OUTPUT': str(8 << 20),
    })
    environment.update(env)
    return subprocess.Popen([sys.executable, '-B', str(DRIVER)], cwd=ROOT, env=environment,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)


def report(name, problems):
    if problems:
        print(f'FAIL {name}: ' + '; '.join(problems))
        return False
    print(f'ok   {name}')
    return True


def stop_group(process):
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    if process.poll() is None:
        process.wait(timeout=5)


def bounded_write():
    pidfiles = [WORK / 'write-oracle.pid', WORK / 'write-candidate.pid']
    for path in pidfiles:
        path.unlink(missing_ok=True)
    saved = ROOT / '.work/tmp/fuzz-control-101-case0.bin'
    saved.unlink(missing_ok=True)
    oracle = wrapper('write-oracle', 'deaf', pidfiles[0])
    candidate = wrapper('write-candidate', 'echo', pidfiles[1])
    process = start(oracle, candidate, '101', {'PEER_TIMEOUT_SECS': '1', 'DRIVER_MODE': 'deaf'})
    pids = []
    problems = []
    stderr = b''
    try:
        started = time.monotonic()
        try:
            _, stderr = process.communicate(timeout=8)
        except subprocess.TimeoutExpired:
            problems.append('the driver blocked past its wrapper timeout')
        elapsed = time.monotonic() - started
        pids.extend(pids_from(pidfiles))
        if process.returncode == 0:
            problems.append('the driver accepted the stalled case')
        if process.returncode is None:
            problems.append('the driver did not exit')
        if elapsed >= 6:
            problems.append(f'took {elapsed:.2f}s')
        if 'did not accept the request within 1s' not in stderr.decode(errors='replace'):
            problems.append(f'missing bounded-write failure: {stderr.decode(errors="replace")[:200]}')
        if not saved.exists() or saved.stat().st_size != 1 << 20:
            problems.append('the in-flight input was not saved')
        if any(alive(pid) for pid in pids):
            problems.append('a peer survived the failure')
    finally:
        pids.extend(pids_from(pidfiles))
        cleanup(pids)
        stop_group(process)
    return report('a deaf peer fails within the write deadline', problems)


def trailing_record():
    pidfiles = [WORK / 'trailing-oracle.pid', WORK / 'trailing-candidate.pid']
    for path in pidfiles:
        path.unlink(missing_ok=True)
    oracle = wrapper('trailing-oracle', 'extra', pidfiles[0])
    candidate = wrapper('trailing-candidate', 'echo', pidfiles[1])
    process = start(oracle, candidate, '102', {'PEER_TIMEOUT_SECS': '2', 'DRIVER_MODE': 'echo'})
    pids = []
    problems = []
    stderr = b''
    try:
        try:
            _, stderr = process.communicate(timeout=8)
        except subprocess.TimeoutExpired:
            problems.append('the driver blocked past its wrapper timeout')
        pids.extend(pids_from(pidfiles))
        if process.returncode == 0:
            problems.append('the driver accepted an unrequested record')
        if process.returncode is None:
            problems.append('the driver did not exit')
        if 'unrequested record' not in stderr.decode(errors='replace'):
            problems.append(f'missing trailing-record failure: {stderr.decode(errors="replace")[:200]}')
        if any(alive(pid) for pid in pids):
            problems.append('a peer survived the failure')
    finally:
        pids.extend(pids_from(pidfiles))
        cleanup(pids)
        stop_group(process)
    return report('an extra trailing record fails the run', problems)


def interrupted():
    pidfiles = [WORK / 'interrupt-oracle.pid', WORK / 'interrupt-candidate.pid']
    for path in pidfiles:
        path.unlink(missing_ok=True)
    saved = ROOT / '.work/tmp/fuzz-control-103-case0.bin'
    saved.unlink(missing_ok=True)
    oracle = wrapper('interrupt-oracle', 'deaf', pidfiles[0])
    candidate = wrapper('interrupt-candidate', 'echo', pidfiles[1])
    process = start(oracle, candidate, '103', {'PEER_TIMEOUT_SECS': '30', 'DRIVER_MODE': 'deaf'})
    pids = []
    problems = []
    try:
        deadline = time.monotonic() + 6
        while not saved.exists() and time.monotonic() < deadline and process.poll() is None:
            time.sleep(0.05)
        appeared = saved.exists()
        stop_group(process)
        pids.extend(pids_from(pidfiles))
        if not appeared:
            problems.append('the in-flight input was not saved before the interruption')
        if saved.exists() and saved.stat().st_size != 1 << 20:
            problems.append('the saved input is incomplete')
        if any(alive(pid) for pid in pids):
            problems.append('a peer survived the driver group kill')
    finally:
        pids.extend(pids_from(pidfiles))
        cleanup(pids)
        stop_group(process)
    return report('an interrupted run keeps its input and stops its peers', problems)


def main():
    if not (ROOT / '.work').exists():
        (ROOT / '.work').mkdir(parents=True, exist_ok=True)
    (ROOT / '.work/tmp').mkdir(parents=True, exist_ok=True)
    if WORK.exists():
        shutil.rmtree(WORK)
    WORK.mkdir(parents=True)
    PEER.write_text(PEER_SOURCE)
    DRIVER.write_text(DRIVER_SOURCE)
    results = [bounded_write(), trailing_record(), interrupted()]
    shutil.rmtree(WORK, ignore_errors=True)
    return 0 if all(results) else 1


if __name__ == '__main__':
    raise SystemExit(main())
