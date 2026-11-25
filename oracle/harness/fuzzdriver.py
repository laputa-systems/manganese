"""The loop every structured fuzz driver runs under `box-fuzz`.

The runner validates SEED and CASES and passes them with FUZZ_ORACLE, FUZZ_CANDIDATE and FUZZ_RESULT.
Each peer response is bounded by PEER_TIMEOUT_SECS and PEER_MAX_OUTPUT. Whatever stops the run, the
request in flight is saved under .work/tmp with the peers' identities, and FUZZ_RESULT records the
seed, the requested cases and the cases completed; the runner rejects a run without it.
"""

import hashlib
import os
import pathlib
import select
import struct
import subprocess
import sys
import time

DEFAULT_TIMEOUT_SECS = 300
DEFAULT_MAX_OUTPUT = 1 << 30


class PeerFailure(Exception):
    pass


def positive(name, default=None):
    value = os.environ.get(name, '')
    if not value and default is not None:
        return default
    if not value.isdigit() or int(value) <= 0:
        raise SystemExit(f'{name} must be a positive integer, got {value!r}')
    return int(value)


def budget():
    seed = os.environ.get('SEED', '')
    if not seed.isdigit():
        raise SystemExit(f'SEED must be an unsigned integer, got {seed!r}')
    return int(seed), positive('CASES')


class Peer:
    def __init__(self, role, path, timeout, max_output):
        self.role, self.timeout, self.max_output = role, timeout, max_output
        self.process = subprocess.Popen([path], stdin=subprocess.PIPE, stdout=subprocess.PIPE, start_new_session=True)

    def handshake(self):
        self.send(b'MGB1')
        if self.read(4) != b'MGB1':
            raise PeerFailure(f'{self.role} wrote an invalid protocol header')

    def send(self, data):
        try:
            self.process.stdin.write(data)
            self.process.stdin.flush()
        except BrokenPipeError:
            raise PeerFailure(f'{self.role} stopped reading its input') from None

    def read(self, length):
        deadline = time.monotonic() + self.timeout
        data = bytearray()
        descriptor = self.process.stdout.fileno()
        while len(data) < length:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([descriptor], [], [], remaining)[0]:
                raise PeerFailure(f'{self.role} did not respond within {self.timeout}s')
            chunk = os.read(descriptor, length - len(data))
            if not chunk:
                raise PeerFailure(f'{self.role} exited before responding' if not data else f'{self.role} wrote a truncated record')
            data += chunk
        return bytes(data)

    def ask(self, record):
        self.send(struct.pack('>I', len(record)) + record)
        length = struct.unpack('>I', self.read(4))[0]
        if length > self.max_output:
            raise PeerFailure(f'{self.role} announced a {length}-byte record, over the {self.max_output}-byte output budget')
        return self.read(length)

    def finish(self):
        self.process.stdin.close()
        try:
            status = self.process.wait(self.timeout)
        except subprocess.TimeoutExpired:
            raise PeerFailure(f'{self.role} did not exit after its input closed') from None
        if status:
            raise PeerFailure(f'{self.role} exited {status}')

    def stop(self):
        try:
            os.killpg(self.process.pid, 9)
        except ProcessLookupError:
            pass
        self.process.wait()


def identity(role, path):
    try:
        return f'{role} {path} sha256:{hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()}'
    except OSError as error:
        return f'{role} {path} unreadable: {error}'


def run(box, seed, cases, generate):
    """Ask both peers each request generate(oracle) returns, and stop at the first difference or failure."""
    timeout = positive('PEER_TIMEOUT_SECS', DEFAULT_TIMEOUT_SECS)
    max_output = positive('PEER_MAX_OUTPUT', DEFAULT_MAX_OUTPUT)
    paths = {'oracle': os.environ['FUZZ_ORACLE'], 'candidate': os.environ['FUZZ_CANDIDATE']}
    identities = '\n'.join(identity(role, path) for role, path in paths.items())
    print(f'fuzz box={box} seed={seed} cases={cases}', flush=True)
    started = time.monotonic()
    peers, completed, failure, record = [], 0, None, b''
    try:
        for role, path in paths.items():
            peers.append(Peer(role, path, timeout, max_output))
            peers[-1].handshake()
        oracle, candidate = peers
        for index in range(cases):
            record = b''
            record = generate(oracle)
            expected, actual = oracle.ask(record), candidate.ask(record)
            if expected != actual:
                failure = f'divergence\noracle hex={expected.hex()}\ncandidate hex={actual.hex()}'
                break
            completed += 1
        else:
            for peer in peers:
                peer.finish()
    except PeerFailure as error:
        failure = str(error)
    finally:
        for peer in peers:
            peer.stop()
    pathlib.Path(os.environ['FUZZ_RESULT']).write_text(f'seed={seed}\nrequested={cases}\ncompleted={completed}\n')
    if failure is not None:
        saved = pathlib.Path(f'.work/tmp/fuzz-{box}-{seed}-case{completed}.bin')
        saved.parent.mkdir(parents=True, exist_ok=True)
        saved.write_bytes(record)
        print(f'{failure}\nseed={seed} case={completed} request hex={record.hex()}\n{identities}\nsaved {saved}', file=sys.stderr, flush=True)
        raise SystemExit(1)
    print(f'fuzz passed {completed} cases seed={seed} elapsed={time.monotonic() - started:.2f}s', flush=True)
