#!/usr/bin/env -S python3 -B
import os
import pathlib
import random
import struct
import sys
from compare import request

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import fuzzdriver

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
seed, cases = fuzzdriver.budget()
rng = random.Random(seed)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
rows = [line.split() for line in pathlib.Path('goldens.lock').read_text().splitlines() if line.startswith('zstd seen ')]
if not rows:
    raise SystemExit('no seen inputs')
seeds = [(store / row[3]).read_bytes() for row in rows]

def generate(oracle):
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
    return item


fuzzdriver.run('zstd', seed, cases, generate)
