#!/usr/bin/env -S python3 -B
import os
import pathlib
import random
import struct
import sys
from compare import request

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import fuzzdriver

STREAM, EXACT, BUFFER, PATCH, SHARED, SHARED_DICT = range(6)
MAX_SEED_BYTES = 200000
root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'


def parse(record):
    mode, count, output_size, dict_size = struct.unpack('>BHII', record[:11])
    pos = 11
    dictionary = record[pos:pos + dict_size]
    pos += dict_size
    chunks = []
    for _ in range(count):
        length = struct.unpack('>I', record[pos:pos + 4])[0]
        pos += 4
        chunks.append(record[pos:pos + length])
        pos += length
    if pos != len(record):
        raise RuntimeError('invalid seen record')
    return mode, output_size, dictionary, b''.join(chunks)


seeds = []
for row in pathlib.Path('goldens.lock').read_text().splitlines():
    fields = row.split()
    if len(fields) == 7 and fields[:2] == ['brotli', 'seen'] and int(fields[4]) <= MAX_SEED_BYTES:
        seeds.append(parse((store / fields[3]).read_bytes()))
if not seeds:
    raise RuntimeError('no seen inputs')
dictionaries = sorted({dictionary for _, _, dictionary, _ in seeds if dictionary})

seed, cases = fuzzdriver.budget()
rng = random.Random(seed)


def mutate(data):
    data = bytearray(data)
    for _ in range(rng.randrange(0, 5)):
        op = rng.randrange(7)
        if op == 0 and data:
            data[rng.randrange(len(data))] ^= 1 << rng.randrange(8)
        elif op == 1 and data:
            del data[rng.randrange(len(data))]
        elif op == 2:
            data.insert(rng.randrange(len(data) + 1), rng.randrange(256))
        elif op == 3 and data:
            del data[rng.randrange(len(data)):]
        elif op == 4:
            other = rng.choice(seeds)[3]
            if other:
                start = rng.randrange(len(other))
                data[rng.randrange(len(data) + 1):0] = other[start:start + rng.randrange(1, 64)]
        elif op == 5 and data:
            start = rng.randrange(len(data))
            data[start:start + rng.randrange(1, 8)] = rng.randbytes(rng.randrange(1, 8))
        elif op == 6:
            data += rng.randbytes(rng.randrange(1, 40))
    return bytes(data)


def generate(oracle):
    mode, output_size, dictionary, encoded = rng.choice(seeds)
    kind = rng.randrange(20)
    if kind == 0:
        data = rng.randbytes(rng.randrange(0, 64))
    elif kind == 1:
        data = encoded[:rng.randrange(len(encoded) + 1)] + rng.randbytes(rng.randrange(0, 32))
    else:
        data = mutate(encoded)
    if rng.randrange(5) == 0:
        mode = rng.randrange(SHARED_DICT + 1)
    if mode in (SHARED_DICT, STREAM) and not dictionary and dictionaries and rng.randrange(3) == 0:
        dictionary = rng.choice(dictionaries)
    if mode == STREAM:
        width = min(32768, max(-(-len(data) // 65535), rng.choice([1, 2, 7, 17, 255, 1024, 4093, 32768])))
        chunks = [data[i:i + width] for i in range(0, len(data), width)]
        output_size = rng.choice([1, 3, 64, 257, 1024, 4096, 65536, max(1, output_size)])
        if dictionary and rng.randrange(8) == 0:
            dictionary = mutate(dictionary)[:rng.randrange(1, len(dictionary) + 2)] or b'x'
    else:
        chunks = [data]
        if mode == PATCH:
            output_size = 32768
        else:
            output_size = max(0, rng.choice([output_size, output_size + 1, max(0, output_size - 1), rng.randrange(0, 70000), output_size + rng.randrange(0, 500)]))
        if mode != SHARED_DICT:
            dictionary = b''
        elif rng.randrange(12) == 0:
            dictionary = b''
    return request(mode, output_size, dictionary, chunks)


fuzzdriver.run('brotli', seed, cases, generate)
