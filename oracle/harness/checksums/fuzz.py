import os
import pathlib
import random
import time
from compare import ask, peer, request

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
seed = int(os.environ.get('SEED') or 7211)
cases = int(os.environ.get('CASES') or 10000)
rng = random.Random(seed)
a = peer(os.environ['FUZZ_ORACLE'])
b = peer(os.environ['FUZZ_CANDIDATE'])
start = time.monotonic()
print(f'fuzz box=checksums seed={seed} cases={cases}', flush=True)
for i in range(cases):
    size = rng.choice([0, 1, 3, 4, 7, 8, 15, 16, 31, 32, 63, 64, 127, 128, 255, 256, 8191, 8192, 8193, rng.randrange(16385)])
    data = bytearray(rng.randbytes(size))
    for _ in range(rng.randrange(1, 5)):
        if data and rng.randrange(2): data[rng.randrange(len(data))] ^= 1 << rng.randrange(8)
        else: data.insert(rng.randrange(len(data) + 1), rng.randrange(256))
    points = sorted([0, len(data)] + [rng.randrange(len(data) + 1) for _ in range(rng.randrange(5))])
    chunks = [bytes(data[x:y]) for x, y in zip(points, points[1:])]
    op = rng.randrange(4)
    initial = rng.choice([0, 0xbc9f1d34, rng.getrandbits(32)])
    rec = request(op, rng.randrange(64), initial, chunks)
    expected, actual = ask(a, rec), ask(b, rec)
    if expected != actual:
        print('divergence seed', seed, 'case', i, 'request', rec.hex(), 'oracle', expected.hex(), 'candidate', actual.hex(), flush=True)
        raise SystemExit(1)
for process in (a, b):
    process.stdin.close()
    if process.wait(): raise SystemExit('peer failed')
print(f'fuzz passed {cases} cases seed={seed} elapsed={time.monotonic()-start:.2f}s')
