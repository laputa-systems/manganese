#!/usr/bin/env -S python3 -B
import os
import pathlib
import random
import sys

from corpus import carries_parsed_codec, mutate

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import fuzzdriver

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
seeds = []
for row in pathlib.Path('goldens.lock').read_text().splitlines():
    fields = row.split()
    if len(fields) == 7 and fields[0:2] == ['media-demux', 'seen'] and int(fields[4]) <= 1 << 20:
        seeds.append((store / fields[3]).read_bytes())
if not seeds:
    raise SystemExit('no media-demux seen inputs in goldens.lock')
seed, cases = fuzzdriver.budget()
rng = random.Random(seed)


def generate(_oracle):
    while True:
        data = mutate(rng.choice(seeds), rng)
        if not carries_parsed_codec(data):
            return data


fuzzdriver.run('media-demux', seed, cases, generate)
