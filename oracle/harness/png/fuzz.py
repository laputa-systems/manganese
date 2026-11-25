#!/usr/bin/env -S python3 -B
import os
import random
import struct
import subprocess
import sys

from corpus import decode_request, encode_request, mutate, random_png

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


def ask(process: subprocess.Popen, value: bytes) -> bytes:
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


print(f'png fuzz seed={seed} cases={cases}', flush=True)
try:
    for index in range(cases):
        if rng.randrange(4) == 0:
            width, height = rng.randrange(1, 20), rng.randrange(1, 20)
            value = encode_request(width, height, rng.randrange(5), rng.randrange(1 << 32), rng.randrange(3))
        else:
            data = mutate(random_png(rng), rng)
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
finally:
    for process in peers:
        if process.poll() is None:
            process.kill()
            process.wait()
print(f'png fuzz passed {cases} cases')
