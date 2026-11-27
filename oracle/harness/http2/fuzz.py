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
    if len(fields) == 7 and fields[:2] == ['http2', 'seen']:
        seeds.append((store / fields[3]).read_bytes())
if not seeds:
    raise SystemExit('no http2 seen inputs in goldens.lock')
seed, cases = fuzzdriver.budget()
rng = random.Random(seed)

METHODS = ['GET', 'POST', 'HEAD', 'OPTIONS', 'PUT', 'DELETE', 'PATCH']
PATHS = ['/', '/index.html', '/a/b/c?d=e', '/%20space', '/x' * 40, '/empty?']
HOSTS = ['verified.https.test', 'a.test', 'www.example.com', 'localhost', 'x' * 40 + '.test']
VALUES = [
    ('user-agent', 'Mozilla/5.0 (X11; Linux x86_64) Manganese/156'),
    ('accept-language', 'en-US,en;q=0.9'),
    ('accept-encoding', 'gzip, deflate, br'),
    ('cookie', 'a=1; b=2; c=3'),
    ('cookie', 'single=value'),
    ('referer', 'https://verified.https.test/'),
    ('x-custom-header', 'custom value with spaces'),
    ('authorization', 'Bearer token-value-1234'),
    ('x-empty', ''),
    ('if-none-match', '"etag"'),
]
SETTINGS = [
    [],
    [(1, 4096), (2, 0), (3, 100), (4, 65535), (5, 16384), (6, 65536)],
    [(1, 0)],
    [(1, 8192), (4, 131072), (5, 32768)],
    [(2, 1), (3, 1)],
    [(4, 0), (6, 1024)],
    [(1, 65536), (3, 8), (4, 1048576)],
]
STATUSES = [200, 204, 206, 301, 304, 400, 404, 500]


def mutate(record):
    case = wire.decode(record)
    item = case['requests'][0]
    for _ in range(rng.randrange(1, 5)):
        choice = rng.randrange(7)
        if choice == 0:
            item['method'] = rng.choice(METHODS)
        elif choice == 1:
            item['path'] = rng.choice(PATHS)
        elif choice == 2:
            item['host'] = rng.choice(HOSTS)
        elif choice == 3:
            item['priority'] = rng.randrange(0, 6)
        elif choice == 4:
            item['status'] = rng.choice(STATUSES)
        elif choice == 5:
            item['headers'] = [rng.choice(VALUES) for _ in range(rng.randrange(0, 7))]
        else:
            case['settings'] = rng.choice(SETTINGS)
    case['window_update'] = rng.choice([0, 65535, 262144])
    case['end'] = rng.choice(['close', 'reset'])
    item['reset'] = 0
    item['trailers'] = rng.randrange(12) == 0
    return wire.encode(case)


def generate(_oracle):
    return mutate(rng.choice(seeds))


fuzzdriver.run('http2', seed, cases, generate)
