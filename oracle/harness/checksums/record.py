import hashlib
import os
import pathlib
import random
from compare import ask, peer, request

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
peer_process = peer('.work/tmp/checksums/oracle')
rows = []

def blob(data):
    digest = hashlib.sha256(data).hexdigest()
    (store / digest).write_bytes(data)
    return digest, len(data)

def add(lane, name, op, offset, seed, chunks):
    if lane == 'holdout': chunks = [b''] + chunks
    value = request(op, offset, seed, chunks)
    expected = ask(peer_process, value)
    a, b = blob(value), blob(expected)
    rows.append(f'checksums {lane} {lane}-{name} {a[0]} {a[1]} {b[0]} {b[1]}')

for lane, lane_seed in [('seen', 8111), ('holdout', 8112)]:
    rng = random.Random(lane_seed)
    for offset in range(64):
        for pattern in range(4):
            length = [0, 1, 7, 8, 15, 16, 31, 32, 63, 64, 127, 128, 255, 256, 257, 8191, 8192, 8193][(offset + pattern * 3) % 18]
            data = [bytes(length), bytes([255]) * length, rng.randbytes(length), (pathlib.Path('docs/oracle.md').read_bytes() * (length // 100 + 1))[:length]][pattern]
            cut = len(data) // 2
            for op in range(4):
                seed = 0xbc9f1d34 if op == 3 else 0
                add(lane, f'op-{op}-offset-{offset}-{pattern}', op, offset, seed, [data[:cut], data[cut:]])
    for size in [0, 1, 3, 4, 7, 8, 15, 16, 31, 32, 63, 64, 127, 128, 255, 256, 257, 8191, 8192, 8193, 32767, 32768, 32769, 1048575, 1048576, 1048577]:
        data = rng.randbytes(size)
        for cut in sorted({0, min(size, 1), min(size, 7), min(size, 8), min(size, 63), min(size, 64), size // 2, size}):
            for op in range(4):
                seed = 0xbc9f1d34 if op == 3 else 0 if op != 0 else rng.getrandbits(32)
                add(lane, f'op-{op}-boundary-{size}-{cut}', op, (cut + lane_seed) % 64, seed, [data[:cut], data[cut:]])
    for fill in (0, 255):
        data = bytes([fill]) * 1048593
        for op in range(4):
            seed = 0xbc9f1d34 if op == 3 else 0
            add(lane, f'op-{op}-large-fill-{fill}', op, fill % 64, seed, [data[:8192], data[8192:]])
peer_process.stdin.close()
if peer_process.wait(): raise SystemExit('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('checksums ')]
lock.write_text('\n'.join(existing + rows) + '\n')
print('recorded', len(rows))
