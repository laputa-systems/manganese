#!/usr/bin/env -S python3 -B
import pathlib
import random
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import fuzzdriver
from cases import fuzz_script
from relationships import check_partial_order

seed, count = fuzzdriver.budget()
rng = random.Random(seed)
check_partial_order()
fuzzdriver.run('runtime', seed, count, lambda oracle: fuzz_script(rng))
