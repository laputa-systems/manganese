import random

from wire import encode

HOSTS = ['verified.https.test', 'a.test', 'www.example.com', 'x' * 40 + '.test']
METHODS = ['GET', 'POST', 'HEAD', 'OPTIONS', 'PUT', 'DELETE']
PATHS = ['/', '/index.html', '/a/b/c?d=e', '/%20space', '/x' * 60]
HEADER_POOL = [
    ('user-agent', 'Mozilla/5.0 (X11; Linux x86_64) Manganese/156'),
    ('accept-language', 'en-US,en;q=0.9'),
    ('accept-encoding', 'gzip, deflate, br'),
    ('cache-control', 'no-cache'),
    ('cookie', 'a=1; b=2; c=3'),
    ('cookie', 'single=value'),
    ('referer', 'https://verified.https.test/'),
    ('x-custom-header', 'custom value with spaces'),
    ('sec-ch-ua', '"Chromium";v="156"'),
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
STATUSES = [200, 204, 301, 400, 404, 500]
ERRORS = [0, 2, 5, 8]


def request(index, rng):
    item = {
        'method': rng.choice(METHODS),
        'scheme': 'https',
        'host': rng.choice(HOSTS),
        'path': rng.choice(PATHS),
        'headers': [],
        'priority': rng.randrange(0, 6),
        'status': rng.choice(STATUSES),
        'reset': 0,
        'trailers': 0,
    }
    count = rng.randrange(0, 7)
    for _ in range(count):
        item['headers'].append(rng.choice(HEADER_POOL))
    if rng.randrange(12) == 0:
        item['reset'] = 1
        item['error'] = rng.choice(ERRORS)
    if rng.randrange(14) == 0:
        item['trailers'] = 1
    return item


def case(name, rng):
    count = 1
    return {
        'name': name,
        'requests': [request(index, rng) for index in range(count)],
        'settings': rng.choice(SETTINGS),
        'window_update': rng.choice([0, 65535, 262144]),
        'end': rng.choice(['close', 'close', 'close', 'reset']),
    }


def generate(lane):
    rng = random.Random(0x4842 if lane == 'seen' else 0x4843)
    cases = []
    for index in range(300 if lane == 'seen' else 160):
        name = f'{lane}-{index:04d}'
        cases.append((name, encode(case(name, rng))))
    return cases
