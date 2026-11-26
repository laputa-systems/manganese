#!/usr/bin/env -S python3 -B
import os
import pathlib
import random
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import fuzzdriver
from cases import random_case, random_response, segment
from wire import MAX_SEGMENT, encode

os.chdir(pathlib.Path(__file__).resolve().parents[3])
seed, cases = fuzzdriver.budget()
rng = random.Random(seed)
SPECIAL = [b'\r', b'\n', b'\r\n', b'\x00', b':', b',', b' ', b'\t', b';', b'"', b'0', b'F', b'HTTP/1.1 ', b'Content-Length: ',
           b'Transfer-Encoding: chunked', b'\r\n\r\n', b'Connection: close', b'Location: /x', b'1', b'-', b'+']


def mutate(data):
    data = bytearray(data)
    for _ in range(rng.randrange(1, 6)):
        choice = rng.randrange(6)
        if not data or choice == 0:
            data[rng.randrange(len(data) + 1):rng.randrange(len(data) + 1)] = rng.choice(SPECIAL)
        elif choice == 1:
            data[rng.randrange(len(data))] ^= 1 << rng.randrange(8)
        elif choice == 2:
            start = rng.randrange(len(data))
            del data[start:start + rng.randrange(1, 16)]
        elif choice == 3:
            start = rng.randrange(len(data))
            data[start:start] = data[start:start + rng.randrange(1, 64)]
        elif choice == 4:
            data = data[:rng.randrange(len(data) + 1)]
        else:
            data[rng.randrange(len(data))] = rng.randrange(256)
    return bytes(data)


def generate(oracle):
    case = random_case(rng)
    if rng.randrange(3):
        response = b''.join(case['segments']) if rng.randrange(2) else random_response(rng)
        case['segments'] = segment(mutate(response)[:20 * MAX_SEGMENT], rng)
    return encode(case)


fuzzdriver.run('http1', seed, cases, generate)
