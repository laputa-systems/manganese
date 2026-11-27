#!/usr/bin/env -S python3 -B
import os
import pathlib
import random
import sys

from corpus import decode_request, mutate, random_gif

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import fuzzdriver

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
seeds = []
for row in pathlib.Path('goldens.lock').read_text().splitlines():
    fields = row.split()
    if len(fields) == 7 and fields[0:2] == ['gif', 'seen'] and int(fields[4]) <= 1 << 18:
        seeds.append((store / fields[3]).read_bytes())
if not seeds:
    raise SystemExit('no gif seen inputs in goldens.lock')
seed, cases = fuzzdriver.budget()
rng = random.Random(seed)


def generate(_oracle):
    if rng.randrange(4) == 0:
        value = mutate(rng.choice(seeds), rng)
    else:
        data = random_gif(rng)
        if rng.randrange(4) == 0:
            data = mutate(data, rng)
        value = decode_request(data, [])
    return value


fuzzdriver.run('gif', seed, cases, generate)
