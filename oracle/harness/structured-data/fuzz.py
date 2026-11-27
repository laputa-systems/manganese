#!/usr/bin/env -S python3 -B
import os
import pathlib
import random
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import fuzzdriver
import corpus

os.chdir(pathlib.Path(__file__).resolve().parents[3])
seed, cases = fuzzdriver.budget()
rng = random.Random(seed)


def generate(oracle):
    kind = rng.randrange(3)
    if kind == 0:
        return corpus.random_json(rng)
    if kind == 1:
        return corpus.write_request(rng.randrange(4), corpus.random_value(rng))
    return corpus.random_xml(rng)


fuzzdriver.run('structured-data', seed, cases, generate)
