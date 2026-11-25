import json
import os
import pathlib
import struct
import subprocess

from wire import request


def peer(path):
    process = subprocess.Popen([path], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    process.stdin.write(b'MGB1')
    process.stdin.flush()
    if process.stdout.read(4) != b'MGB1': raise ValueError('invalid header')
    return process


def ask(process, data):
    process.stdin.write(struct.pack('>I', len(data)) + data)
    process.stdin.flush()
    header = process.stdout.read(4)
    if len(header) != 4: raise ValueError('missing response')
    return process.stdout.read(struct.unpack('>I', header)[0])


def decode(data):
    if data == b'\0': return {'valid': False}
    values = {'valid': True, 'opaque': data[1]}
    data = data[2:]
    for name in ('spec', 'scheme', 'username', 'password', 'host', 'port', 'path', 'query', 'fragment', 'origin', 'domain'):
        length = struct.unpack('>I', data[:4])[0]
        data = data[4:]
        if length == 0xffffffff:
            values[name] = None
        else:
            values[name] = data[:length].decode('utf-8')
            data = data[length:]
    values['same'] = data[0]
    return values


oracle = peer('.work/tmp/url/oracle')
candidate = peer('.work/tmp/target/x86_64-unknown-linux-musl/debug/url')
cases = json.loads(pathlib.Path('.work/tmp/url/urltestdata.json').read_text())
for index, case in enumerate(cases):
    if not isinstance(case, dict) or (index % 3 == 0) != (os.environ.get('URL_COMPARE_HOLDOUT') == '1'): continue
    data = request(case['input'], case.get('base'), None, index % 2)
    expected = decode(ask(oracle, data))
    actual = decode(ask(candidate, data))
    if expected != actual:
        difference = {key: [expected.get(key), actual.get(key)] for key in expected.keys() | actual.keys() if expected.get(key) != actual.get(key)}
        print(index, repr(case['input']), repr(case.get('base')), repr(difference), flush=True)
for process in (oracle, candidate):
    process.stdin.close()
    if process.wait(): raise SystemExit('peer failed')
