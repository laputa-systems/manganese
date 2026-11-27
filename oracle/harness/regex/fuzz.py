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
    if len(fields) == 7 and fields[0:2] == ['regex', 'seen']:
        data = (store / fields[3]).read_bytes()
        try:
            parsed = corpus.decode_request(data)
        except (ValueError, UnicodeDecodeError):
            continue
        if parsed[0] not in corpus.INVALID:
            seeds.append(parsed)
if not seeds:
    raise SystemExit('no regex seen inputs in goldens.lock')

seed, cases = fuzzdriver.budget()
rng = random.Random(seed)


def flags_text(current):
    return ''.join(flag for flag in corpus.FLAG_SET if flag in current or rng.random() < 0.2)


def subject_text(current):
    text = ''.join(rng.choice(corpus.SUBJECT_ATOMS) for _ in range(rng.randrange(0, 14)))
    if rng.random() < 0.2 and current:
        text = current + text
    return text.rstrip('\n')


def generate(_oracle):
    if rng.random() < 0.55:
        return corpus.encode_request(*corpus.random_case(rng))
    pattern, flag, text, _, _ = rng.choice(seeds)
    return corpus.encode_request(pattern, flags_text(flag), subject_text(text), rng.randrange(1, 5), 0)


fuzzdriver.run('regex', seed, cases, generate)
