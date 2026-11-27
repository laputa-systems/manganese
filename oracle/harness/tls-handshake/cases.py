import hashlib
import random
import string

import wire

HOSTS = [
    'verified.https.test', 'example.test', 'www.example.com', 'a.test', 'localhost',
    'deep.sub.domain.under.example.test', 'x' * 63 + '.test', 'y' * 253, 'z',
    'upper.CASE.test', 'with-dash.test', 'a.b.c.d.e.f.g.test',
]
PROTOS = [b'h2', b'http/1.1', b'http/1.0', b'ws', b'wss', b'spdy/3.1', b'acme-tls/1', b'p' * 32]


def lane(label):
    return 'holdout' if hashlib.sha256(label.encode()).digest()[0] < 77 else 'seen'


def record(host, protos):
    return wire.encode(host, protos)


def named():
    result = []
    for index, host in enumerate(HOSTS):
        for variant, protos in enumerate(([b'h2', b'http/1.1'], [b'http/1.1'], [b'h2'])):
            result.append((f'host{index}-alpn{variant}', record(host, protos)))
    for count in range(1, 9):
        result.append((f'alpn-count{count}', record('alpn.test', PROTOS[:count])))
    for size in (1, 2, 63, 64, 100, 252, 253):
        result.append((f'host-len{size}', record('h' * size, [b'h2'])))
    for size in (1, 2, 8, 16, 32):
        result.append((f'proto-len{size}', record('proto.test', [b'q' * size])))
    return result


def random_case(rng):
    style = rng.randrange(4)
    if style == 0:
        host = rng.choice(HOSTS)
    elif style == 1:
        labels = [''.join(rng.choice(string.ascii_lowercase + '-') for _ in range(rng.randint(1, 12)))
                  for _ in range(rng.randint(1, 4))]
        host = '.'.join(labels)
    elif style == 2:
        host = ''.join(rng.choice(string.ascii_letters + string.digits + '.-') for _ in range(rng.randint(1, 60)))
    else:
        host = rng.choice(['a', 'localhost', 'example.test', 'verified.https.test']) + rng.choice(['', '.x'])
    host = host[:253] or 'a'
    count = rng.randint(1, 8)
    protos = [rng.choice(PROTOS)[:rng.randint(1, 32)] for _ in range(count)]
    protos = [p or b'x' for p in protos]
    return record(host, protos)


def cases():
    result = named()
    rng = random.Random(0x7A1)
    for index in range(120):
        result.append((f'random{index}', random_case(rng)))
    return result
