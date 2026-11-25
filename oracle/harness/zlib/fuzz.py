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
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
seeds = []
invalid = []
for row in pathlib.Path('goldens.lock').read_text().splitlines():
    fields = row.split()
    if len(fields) == 7 and fields[0:2] == ['zlib', 'seen'] and fields[2].startswith('seen-invalid-'):
        record = (store / fields[3]).read_bytes()
        invalid.append((struct.unpack('b', record[2:3])[0], record[12:]))
    if len(fields) == 7 and fields[0:2] == ['zlib', 'seen'] and fields[2].startswith('seen-deflate-'):
        data = (store / fields[3]).read_bytes()
        if int.from_bytes(data[5:7], 'big') == 1:
            seeds.append(data[12:])
seed, cases = fuzzdriver.budget()
rng = random.Random(seed)
ops = iter(range(cases))


def generate(oracle):
    data = bytearray(rng.choice(seeds))
    if len(data) > 32768: data = data[:32768]
    for _ in range(rng.randrange(1, 5)):
        if data and rng.randrange(2): data[rng.randrange(len(data))] ^= 1 << rng.randrange(8)
        else: data.insert(rng.randrange(len(data) + 1), rng.randrange(256))
    level = rng.randrange(-1, 10)
    window = rng.choice(list(range(8, 16)) + list(range(-15, -7)) + list(range(24, 32)))
    mem = rng.randrange(1, 10)
    strategy = rng.randrange(5)
    op = next(ops) % 4
    if op == 1:
        if rng.randrange(2):
            window, source = rng.choice(invalid)
            encoded = bytearray(source)
        else:
            window = rng.choice([15, -15, 31])
            encoded = bytearray(oracle.ask(request(0, 6, window, 8, 0, [(data, 4)]))[8:])
        if encoded and rng.randrange(2): encoded[rng.randrange(len(encoded))] ^= 1 << rng.randrange(8)
        data = encoded
    cut = len(data) // 2
    flush = rng.choice([0, 2, 3]) if op == 0 else 0
    rec = request(op, level, window, mem, strategy, [(data[:cut], flush), (data[cut:], 4)])
    return rec


fuzzdriver.run('zlib', seed, cases, generate)
