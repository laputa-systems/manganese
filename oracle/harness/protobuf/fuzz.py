#!/usr/bin/env -S python3 -B
import pathlib
import random
import sys
from cases import generate

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import fuzzdriver

seed, cases = fuzzdriver.budget()
rng = random.Random(seed)
fuzzdriver.run('protobuf', seed, cases, lambda oracle: generate(rng))
