#!/usr/bin/env -S python3 -B
import os
import pathlib
import random
import sys
from compare import request_decode, request_encode, request_label

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import fuzzdriver

os.chdir(pathlib.Path(__file__).resolve().parents[3])
seed, cases = fuzzdriver.budget()
rng = random.Random(seed)
labels = []


def generate(oracle):
    if not labels:
        labels.extend(oracle.ask(b'\x03').splitlines())
    label = rng.choice(labels)
    op = rng.randrange(6)
    if op == 0:
        return request_label(label, web=bool(rng.randrange(2)))
    if op == 1:
        units = [rng.choice([rng.randrange(0x80), 0x20ac, 0x3042, 0x4e2d, 0xd800, 0xdbff, 0xdc00, 0xdfff]) for _ in range(rng.randrange(16))]
        return request_encode(label, b''.join(unit.to_bytes(2, 'big') for unit in units))
    data = rng.randbytes(rng.randrange(0, 32))
    if rng.randrange(4) == 0:
        data = rng.choice([b'\xef\xbb\xbf', b'\xff\xfe', b'\xfe\xff']) + data
    cuts = sorted(rng.randrange(len(data) + 1) for _ in range(rng.randrange(0, 5)))
    points = [0, *cuts, len(data)]
    chunks = [data[a:b] for a, b in zip(points, points[1:])]
    return request_decode(label, rng.randrange(3), rng.randrange(2), chunks)


fuzzdriver.run('encoding', seed, cases, generate)
