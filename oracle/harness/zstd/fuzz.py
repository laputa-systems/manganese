#!/usr/bin/env python3
import os
import pathlib
import random
import struct
import sys
from compare import ask, peer, request

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
seed = int(os.environ.get('SEED') or random.SystemRandom().getrandbits(64))
cases = int(os.environ.get('CASES') or 10000)
rng = random.Random(seed)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
rows = [line.split() for line in pathlib.Path('goldens.lock').read_text().splitlines() if line.startswith('zstd seen ')]
if not rows:
    raise SystemExit('no seen inputs')
seeds = [(store / row[3]).read_bytes() for row in rows]
a = peer(os.environ['FUZZ_ORACLE'])
b = peer(os.environ['FUZZ_CANDIDATE'])
print(f'seed={seed} cases={cases}', flush=True)
for case in range(cases):
    raw = rng.choice(seeds)
    output_size, dict_size, count = struct.unpack_from('>III', raw)
    dictionary = raw[12:12 + dict_size]
    pos = 12 + dict_size
    data = bytearray()
    for _ in range(count):
        length = struct.unpack_from('>I', raw, pos)[0]
        pos += 4
        data.extend(raw[pos:pos + length])
        pos += length
    for _ in range(rng.randint(1, 4)):
        action = rng.randrange(4)
        at = rng.randrange(len(data) + 1)
        if action == 0 and at < len(data):
            data[at] ^= 1 << rng.randrange(8)
        elif action == 1:
            data[at:at] = bytes([rng.randrange(256)])
        elif action == 2 and at < len(data):
            del data[at]
        else:
            del data[at:]
    chunk_size = rng.choice((1, 3, 17, 256, 32768, max(1, len(data))))
    chunks = [data[i:i + chunk_size] for i in range(0, len(data), chunk_size)] or [b'']
    item = request(chunks, dictionary, rng.choice((128, 256, 32768)))
    try:
        x, y = ask(a, item), ask(b, item)
    except Exception as error:
        print(f'divergence case={case}: {error}', file=sys.stderr)
        raise SystemExit(1)
    if x != y:
        path = pathlib.Path(f'.work/tmp/fuzz-zstd-{seed}.bin')
        path.write_bytes(item)
        print(f'divergence case={case} input={path} oracle={x.hex()} candidate={y.hex()}', file=sys.stderr)
        raise SystemExit(1)
for process in (a, b):
    process.stdin.close()
    if process.wait():
        raise SystemExit('adapter failed')
print(f'{cases} cases, zero divergences')
