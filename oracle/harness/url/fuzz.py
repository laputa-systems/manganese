#!/usr/bin/env -S python3 -B
import pathlib
import random
import sys

from wire import request

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import fuzzdriver

seed, cases = fuzzdriver.budget()
rng = random.Random(seed)


def generate(oracle):
    scheme = rng.choice(['http', 'https', 'ftp', 'file', 'blob', 'data', 'javascript', 'about', 'chrome', 'filesystem', 'foo'])
    host = rng.choice(['example.com', 'a.blogspot.com', 'www.ck', '127.1', '[::1]', 'bücher.example', 'a..b', 'localhost', ''])
    path = rng.choice(['/', '/a/../b', '/a%2fb', '/ space', '', 'hello', '/a\\b', '/ü'])
    value = rng.choice([scheme + '://' + host + path, scheme + ':' + path, path, '../' + path])
    if rng.randrange(2): value += rng.choice(['?q=1', '?', '#frag', '#', ':80'])
    base = rng.choice([None, 'https://example.com/a/b', 'file:///tmp/a', 'foo:opaque'])
    other = rng.choice([None, 'https://example.com/', 'http://a.blogspot.com/', 'https://localhost/'])
    setter = None
    if rng.randrange(4) == 0:
        name = rng.choice(['href', 'protocol', 'username', 'password', 'host', 'hostname', 'port', 'pathname', 'search', 'hash'])
        argument = rng.choice(['', 'http', 'https:', 'file:', host, host + ':8080', path, '?q=1', '#fragment', '65536', '00080', 'x@y', 'bücher', "a'b", ' '])
        setter = (('blink:' if rng.randrange(2) else '') + name, argument)
    elif rng.randrange(4) == 0:
        setter = ('schemeful-site', '')
    return request(value, base, other, rng.randrange(2), setter)


fuzzdriver.run('url', seed, cases, generate)
