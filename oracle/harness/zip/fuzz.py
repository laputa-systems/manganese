#!/usr/bin/env -S python3 -B
import os
import pathlib
import random
import sys

import corpus

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import fuzzdriver

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
seed, cases = fuzzdriver.budget()
rng = random.Random(seed)


def generate(_oracle):
    return corpus.encode_request(corpus.random_case(rng))


fuzzdriver.run('zip', seed, cases, generate)
