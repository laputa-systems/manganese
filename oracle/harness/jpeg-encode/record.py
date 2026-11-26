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
oracle = subprocess.Popen(['.work/tmp/jpeg-encode/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)


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
    source = blob(data)
    result = blob(ask(data))
    rows.append(f'jpeg-encode {lane} {lane}-{name} {source[0]} {source[1]} {result[0]} {result[1]}')


lanes = [('seen', 1337, 20260301), ('holdout', 8719, 20260402)]
for lane, seed, random_seed in lanes:
    for width, height in corpus.SIZES:
        for quality in corpus.QUALITIES:
            for pattern in corpus.PATTERNS:
                add(lane, f'{width}x{height}-q{quality}-{pattern}',
                    corpus.encode_request(width, height, quality, seed + width * 31 + height * 7 + quality, pattern))
    for quality in corpus.QUALITIES:
        add(lane, f'large-{quality}', corpus.encode_request(257, 193, quality, seed + quality, 'noise'))
    rng = random.Random(random_seed)
    for index in range(120):
        add(lane, f'random-{index}', corpus.random_request(rng))

oracle.stdin.close()
if oracle.wait():
    raise RuntimeError('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('jpeg-encode ')]
if len(existing) != len(lock.read_text().splitlines()):
    raise RuntimeError('jpeg-encode goldens already exist')
lock.write_text('\n'.join(existing + rows) + '\n')
print('recorded', len(rows), 'jpeg-encode goldens')
