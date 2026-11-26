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
    if len(fields) == 7 and fields[0:2] == ['jpeg-decode', 'seen'] and int(fields[4]) <= 1 << 18:
        seeds.append(corpus.request_data((store / fields[3]).read_bytes()))
if not seeds:
    raise SystemExit('no jpeg-decode seen inputs in goldens.lock')
seed, cases = fuzzdriver.budget()
rng = random.Random(seed)


def generate(_oracle):
    while True:
        if rng.random() < 0.25:
            data = corpus.random_jpeg(rng)
            if rng.random() < 0.5:
                data = data[:rng.randrange(len(data) + 1)]
        else:
            data = rng.choice(seeds)
        data = corpus.mutate(data, rng)
        if not corpus.oversized(data):
            return corpus.request(data, corpus.random_cuts(len(data), rng, 4))


fuzzdriver.run('jpeg-decode', seed, cases, generate)
