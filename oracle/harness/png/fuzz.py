#!/usr/bin/env -S python3 -B
import pathlib
import random
import sys

from corpus import decode_request, encode_request, mutate, random_png

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import fuzzdriver

seed, cases = fuzzdriver.budget()
rng = random.Random(seed)


def generate(oracle):
    if rng.randrange(4) == 0:
        width, height = rng.randrange(1, 20), rng.randrange(1, 20)
        value = encode_request(width, height, rng.randrange(5), rng.randrange(1 << 32), rng.randrange(3))
    else:
        data = mutate(random_png(rng), rng)
        cuts = sorted(rng.randrange(len(data) + 1) for _ in range(rng.randrange(0, 5)))
        value = decode_request(data, cuts)
    return value


fuzzdriver.run('png', seed, cases, generate)
