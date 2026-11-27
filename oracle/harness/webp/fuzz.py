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
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
seeds = []
for row in pathlib.Path('goldens.lock').read_text().splitlines():
    fields = row.split()
    if len(fields) == 7 and fields[0:2] == ['webp', 'seen'] and int(fields[4]) <= 1 << 18:
        seeds.append(corpus.request_data((store / fields[3]).read_bytes()))
if not seeds:
    raise SystemExit('no webp seen inputs in goldens.lock')
seed, cases = fuzzdriver.budget()
rng = random.Random(seed)


def generate(_oracle):
    if rng.random() < 0.7:
        data = corpus.random_webp(rng)
    else:
        data = corpus.mutate(rng.choice(seeds), rng)
    return corpus.request(data)


fuzzdriver.run('webp', seed, cases, generate)
