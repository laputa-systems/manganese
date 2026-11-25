#!/usr/bin/env -S python3 -B
import os
import pathlib
import random
import sys
from compare import request

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import fuzzdriver

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
seed, cases = fuzzdriver.budget()
rng = random.Random(seed)


def generate(oracle):
    size = rng.choice([0, 1, 3, 4, 7, 8, 15, 16, 31, 32, 63, 64, 127, 128, 255, 256, 8191, 8192, 8193, rng.randrange(16385)])
    data = bytearray(rng.randbytes(size))
    for _ in range(rng.randrange(1, 5)):
        if data and rng.randrange(2): data[rng.randrange(len(data))] ^= 1 << rng.randrange(8)
        else: data.insert(rng.randrange(len(data) + 1), rng.randrange(256))
    points = sorted([0, len(data)] + [rng.randrange(len(data) + 1) for _ in range(rng.randrange(5))])
    chunks = [bytes(data[x:y]) for x, y in zip(points, points[1:])]
    op = rng.randrange(4)
    initial = rng.choice([0, 0xbc9f1d34, rng.getrandbits(32)])
    return request(op, rng.randrange(64), initial, chunks)


fuzzdriver.run('checksums', seed, cases, generate)
