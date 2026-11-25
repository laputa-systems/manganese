#!/usr/bin/env python3
import os
import random
import struct
import subprocess
import sys

from corpus import decode_request, encode_request, make_png

seed = int(os.environ.get('SEED') or random.SystemRandom().randrange(1 << 64))
cases = int(os.environ.get('CASES') or '10000')
rng = random.Random(seed)
peers = []
for name in ('FUZZ_ORACLE', 'FUZZ_CANDIDATE'):
    process = subprocess.Popen([os.environ[name]], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    process.stdin.write(b'MGB1')
    process.stdin.flush()
    if process.stdout.read(4) != b'MGB1':
        raise RuntimeError(f'{name} has an invalid protocol header')
    peers.append(process)


def ask(process, value):
    process.stdin.write(struct.pack('>I', len(value)) + value)
    process.stdin.flush()
    raw = process.stdout.read(4)
    if len(raw) != 4:
        raise RuntimeError('peer exited during fuzzing')
    length = struct.unpack('>I', raw)[0]
    data = process.stdout.read(length)
    if len(data) != length:
        raise RuntimeError('short response during fuzzing')
    return data


layouts = [(0, 1), (0, 2), (0, 4), (0, 8), (0, 16), (2, 8), (2, 16),
           (3, 1), (3, 2), (3, 4), (3, 8), (4, 8), (4, 16), (6, 8), (6, 16)]
print(f'png fuzz seed={seed} cases={cases}', flush=True)
for index in range(cases):
    width, height = rng.randrange(1, 20), rng.randrange(1, 20)
    if rng.randrange(3) == 0:
        value = encode_request(width, height, rng.randrange(3), rng.randrange(1 << 32), bool(rng.randrange(2)))
    else:
        color, depth = rng.choice(layouts)
        data = bytearray(make_png(width, height, depth, color, bool(rng.randrange(2)), rng.randrange(5), rng.randrange(1 << 16),
                                  color_chunk=rng.choice([None, None, 'gamma', 'srgb', 'chromaticity', 'icc']),
                                  animated=bool(rng.randrange(3) == 0)))
        mutation = rng.randrange(5)
        if mutation == 1:
            del data[rng.randrange(len(data) + 1):]
        elif mutation == 2:
            data[rng.randrange(len(data))] ^= 1 << rng.randrange(8)
        elif mutation == 3:
            position = rng.randrange(len(data))
            del data[position:position + rng.randrange(1, 9)]
        elif mutation == 4:
            position = rng.randrange(len(data) + 1)
            data[position:position] = rng.randbytes(rng.randrange(1, 9))
        cuts = sorted(rng.randrange(len(data) + 1) for _ in range(rng.randrange(0, 5)))
        value = decode_request(data, cuts)
    left, right = (ask(process, value) for process in peers)
    if left != right:
        print(f'divergence seed={seed} case={index} input={value.hex()} oracle={left.hex()} candidate={right.hex()}', file=sys.stderr)
        sys.exit(1)
for process in peers:
    process.stdin.close()
    if process.wait() != 0:
        raise RuntimeError('peer exited unsuccessfully')
print(f'png fuzz passed {cases} cases')
