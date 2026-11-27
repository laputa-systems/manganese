#!/usr/bin/env -S python3 -B
import os
import pathlib
import random
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import fuzzdriver
import wire

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
seeds = []
for row in pathlib.Path('goldens.lock').read_text().splitlines():
    fields = row.split()
    if len(fields) == 7 and fields[:2] == ['tls-handshake', 'seen']:
        seeds.append((store / fields[3]).read_bytes())
if not seeds:
    raise SystemExit('no tls-handshake seen inputs in goldens.lock')
seed, cases = fuzzdriver.budget()
rng = random.Random(seed)

SAFE_HOST = b'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-.'
FIXED_HOSTS = [b'verified.https.test', b'example.test', b'localhost', b'a', b'x' * 253]
FIXED_PROTOS = [b'h2', b'http/1.1', b'http/1.0', b'ws', b'wss', b'p' * 32]


def host_bytes(size):
    return bytes(rng.choice(SAFE_HOST) for _ in range(size))


def mutate(record):
    host, protos = wire.decode(record)
    for _ in range(rng.randrange(1, 5)):
        choice = rng.randrange(6)
        if choice == 0 and len(host) < 253:
            position = rng.randrange(len(host) + 1)
            host = host[:position] + bytes([rng.choice(SAFE_HOST)]) + host[position:]
        elif choice == 1 and len(host) > 1:
            position = rng.randrange(len(host))
            host = host[:position] + host[position + 1:]
        elif choice == 2:
            host = host[:rng.randrange(1, len(host) + 1)]
        elif choice == 3 and host:
            position = rng.randrange(len(host))
            host = host[:position] + bytes([rng.choice(SAFE_HOST)]) + host[position + 1:]
        elif choice == 4:
            host = rng.choice(FIXED_HOSTS)
        else:
            host = host_bytes(rng.randrange(1, 40)) or b'a'
    host = host[:253] or b'a'
    count = rng.randrange(1, 9)
    protos = [rng.choice(FIXED_PROTOS)[:rng.randrange(1, 33)] or b'x' for _ in range(count)]
    return wire.encode(host, protos)


def generate(_oracle):
    return mutate(rng.choice(seeds))


fuzzdriver.run('tls-handshake', seed, cases, generate)
