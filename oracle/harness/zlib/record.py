import hashlib
import os
import pathlib
import random
from compare import ask, peer, request

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
rng = random.Random(7211)
seen = [b'', b'a', b'hello world\n' * 100, bytes(range(256)) * 20, b'a' * 5000, b'abracadabra' * 1000, rng.randbytes(8192), pathlib.Path('docs/oracle.md').read_bytes()]
holdout = [b'b', b'other text\n' * 300, rng.randbytes(8193), b'xyz' * 15000, bytes(range(255, -1, -1)) * 128]
params = [(n, 15, 8, 0) for n in range(-1, 10)] + [(6, w, 8, 0) for w in list(range(8, 16)) + list(range(-15, -7)) + list(range(24, 32))] + [(6, 15, m, 0) for m in range(1, 10)] + [(6, 15, 8, s) for s in range(5)]
rows = []
p = peer('.work/tmp/zlib/oracle')
def blob(data):
    digest = hashlib.sha256(data).hexdigest()
    (store / digest).write_bytes(data)
    return digest, len(data)
def add(lane, name, value):
    result = ask(p, value)
    a, b = blob(value), blob(result)
    rows.append(f'zlib {lane} {lane}-{name} {a[0]} {a[1]} {b[0]} {b[1]}')
for lane, inputs in [('seen', seen), ('holdout', holdout)]:
    for i, data in enumerate(inputs):
        for j, (level, window, mem, strategy) in enumerate(params):
            if lane == "holdout" and window in (-8, 24): continue
            add(lane, f'deflate-{i}-{j}', request(0, level, window, mem, strategy, [(data, 4)]))
            if window not in (-8, 24):
                encoded = ask(p, request(0, level, window, mem, strategy, [(data, 4)]))[8:]
                decode_window = -15 if window < 0 else 31 if window > 15 else 15
                add(lane, f'matrix-inflate-{i}-{j}', request(1, level, decode_window, mem, strategy, [(encoded, 4)]))
        for w in (15, -15, 31):
            encoded = ask(p, request(0, 6, w, 8, 0, [(data, 4)]))[8:]
            add(lane, f'inflate-{i}-{w + 15}', request(1, 6, w, 8, 0, [(encoded, 4)]))
            for mode in (0, 2):
                cut = len(encoded) // 2
                add(lane, f'inflate-stream-{i}-{w + 15}-{mode}', request(1, 6, w, 8, 0, [(encoded[:cut], mode), (encoded[cut:], 4)]))
            for flush in (0, 2, 3):
                half = len(data) // 2
                add(lane, f'stream-{i}-{w + 15}-{flush}', request(0, 6, w, 8, 0, [(data[:half], flush), (data[half:], 4)]))
        for op in (2, 3, 4, 5):
            add(lane, f'checksum-{op}-{i}', request(op, 6, 15, 8, 0, [(data[:len(data)//2], 0), (data[len(data)//2:], 4)]))
for lane, pattern in [('seen', b'large seen input '), ('holdout', b'large holdout input ')]:
    compressed = ask(p, request(0, 6, 15, 8, 0, [(pattern * 1000, 4)]))[8:]
    for i, data in enumerate([pattern * (1048600 // len(pattern) + 1), (pattern * 3000)[:32767], (pattern * 3000)[:32768], (pattern * 3000)[:32769], compressed]):
        for window in (15, -15, 31):
            add(lane, f'boundary-{i}-{window + 15}', request(0, 6, window, 8, 0, [(data, 4)]))
            encoded = ask(p, request(0, 6, window, 8, 0, [(data, 4)]))[8:]
            add(lane, f'boundary-inflate-{i}-{window + 15}', request(1, 6, window, 8, 0, [(encoded, 4)]))
for i, data in enumerate([b'xx', bytes.fromhex('789c07'), bytes.fromhex('789ccb48cdc9c90700'), bytes.fromhex('789ccb48cdc9c9070000000000'), bytes.fromhex('53974b975135b167a6dc1ec4fcaaf9b0c55587ab92977c53'), bytes.fromhex('05009204')]):
    add('seen', f'invalid-{i}', request(1, 6, -15 if i >= 4 else 15, 8, 0, [(data, 4)]))
p.stdin.close()
if p.wait(): raise SystemExit('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('zlib ')]
lock.write_text('\n'.join(existing + rows) + '\n')
print('recorded', len(rows))
