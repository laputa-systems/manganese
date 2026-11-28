#!/usr/bin/env -S python3 -B
import pathlib
import random
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import cases
import fuzzdriver

seed, count = fuzzdriver.budget()
rng = random.Random(seed)


def generate(_oracle):
    return cases.fuzz_script(rng).encode()


fuzzdriver.run('sqlite', seed, count, generate)
