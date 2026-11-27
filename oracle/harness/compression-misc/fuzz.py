#!/usr/bin/env -S python3 -B
import os
import pathlib
import random
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import compare
import corpus
import fuzzdriver

os.chdir(pathlib.Path(__file__).resolve().parents[3])
seed, cases = fuzzdriver.budget()
rng = random.Random(seed)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
seeds = []
for row in pathlib.Path('goldens.lock').read_text().splitlines():
    fields = row.split()
    if (len(fields) == 7 and fields[:2] == ['compression-misc', 'seen']
            and not fields[2].startswith(('seen-invalid-', 'seen-mutated-'))):
        record = (store / fields[3]).read_bytes()
        if record[0] == 0:
            seeds.append(record[5:])

if not seeds:
    raise SystemExit('no seen compression seeds')


def generate(oracle):
    data = bytearray(rng.choice(seeds))
    if len(data) > 32768:
        data = data[:32768]
    for _ in range(rng.randrange(1, 5)):
        if data and rng.randrange(3):
            data[rng.randrange(len(data))] ^= 1 << rng.randrange(8)
        else:
            data.insert(rng.randrange(len(data) + 1), rng.randrange(256))
    if rng.randrange(2):
        return corpus.compress_request(bytes(data))
    status, stream = compare.unwrap(oracle.ask(corpus.compress_request(bytes(data))))
    if status != 0 or not stream:
        stream = bytes([rng.randrange(256)])
    stream = bytearray(stream)
    for _ in range(rng.randrange(1, 4)):
        if stream and rng.randrange(3):
            stream[rng.randrange(len(stream))] ^= 1 << rng.randrange(8)
        else:
            stream.insert(rng.randrange(len(stream) + 1), rng.randrange(256))
    limit = rng.choice((0, 1, len(data) // 2, len(data), len(data) + 1, 1 << 16))
    return corpus.decode_request(bytes(stream), limit)


fuzzdriver.run('compression-misc', seed, cases, generate)
