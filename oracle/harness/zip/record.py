import atexit
import hashlib
import os
import pathlib
import random
import struct
import subprocess

import corpus

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
oracle = subprocess.Popen(['.work/tmp/zip/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)


def stop_oracle():
    if oracle.poll() is None:
        oracle.kill()
    oracle.wait()


atexit.register(stop_oracle)
oracle.stdin.write(b'MGB1')
oracle.stdin.flush()
if oracle.stdout.read(4) != b'MGB1':
    raise RuntimeError('oracle protocol header')


def ask(data: bytes) -> bytes:
    oracle.stdin.write(struct.pack('>I', len(data)) + data)
    oracle.stdin.flush()
    size = oracle.stdout.read(4)
    if len(size) != 4:
        raise RuntimeError('oracle terminated')
    count = struct.unpack('>I', size)[0]
    result = oracle.stdout.read(count)
    if len(result) != count:
        raise RuntimeError('short oracle response')
    return result


def blob(data: bytes) -> tuple[str, int]:
    digest = hashlib.sha256(data).hexdigest()
    path = store / digest
    if path.exists() and path.read_bytes() != data:
        raise RuntimeError('artifact digest collision')
    if not path.exists():
        path.write_bytes(data)
    return digest, len(data)


rows = []


def add(lane: str, name: str, data: bytes) -> None:
    record = corpus.encode_request(data)
    source = blob(record)
    result = blob(ask(record))
    rows.append(f'zip {lane} {lane}-{name} {source[0]} {source[1]} {result[0]} {result[1]}')


base = corpus.cases()
for lane in ('seen', 'holdout'):
    if lane == 'seen':
        for name, data in base:
            add(lane, name, data)
    for seed in range(21 if lane == 'seen' else 91, (21 if lane == 'seen' else 91) + 4):
        rng = random.Random(seed)
        for index in range(100):
            add(lane, f'r{seed}-{index}', corpus.random_case(rng))

oracle.stdin.close()
if oracle.wait():
    raise RuntimeError('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('zip ')]
if len(existing) != len(lock.read_text().splitlines()):
    raise RuntimeError('zip goldens already exist')
lock.write_text('\n'.join(existing + rows) + '\n')
print('recorded', len(rows), 'zip goldens:', sum(1 for row in rows if ' seen ' in row),
      'seen', sum(1 for row in rows if ' holdout ' in row), 'holdout')
