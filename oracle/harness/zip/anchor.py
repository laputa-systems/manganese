import base64
import hashlib
import json
import pathlib
import struct
import subprocess
import sys

import corpus

root = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(root / 'oracle/launch'))
import launch


def peer(path):
    process = subprocess.Popen([path], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    try:
        process.stdin.write(b'MGB1')
        process.stdin.flush()
        if process.stdout.read(4) != b'MGB1':
            raise RuntimeError('oracle protocol header')
    except BaseException:
        stop_peer(process)
        raise
    return process


def stop_peer(process):
    try:
        if process.stdin is not None and not process.stdin.closed:
            process.stdin.close()
        if process.poll() is None:
            process.kill()
        process.wait(timeout=15)
    except OSError as error:
        print(f'oracle cleanup: {error}', file=sys.stderr)


def ask(process, data):
    record = corpus.encode_request(data)
    process.stdin.write(struct.pack('>I', len(record)) + record)
    process.stdin.flush()
    length = struct.unpack('>I', process.stdout.read(4))[0]
    result = process.stdout.read(length)
    if len(result) != length:
        raise RuntimeError('short oracle response')
    return result


def make_case(size, pattern, level, index):
    data = corpus.data_of(size, pattern, index * 17 + level)
    entry = corpus.Entry(f'e{index}.txt', data, method=8)
    entry.payload = corpus.raw_deflate(data, level)
    entry.csize = len(entry.payload)
    blob = corpus.build([entry])
    name_len = struct.unpack_from('<H', blob, 26)[0]
    extra_len = struct.unpack_from('<H', blob, 28)[0]
    start = 30 + name_len + extra_len
    compressed = blob[start:start + entry.csize]
    return blob, data, compressed


items = []
for size in (0, 1, 17, 64, 4096):
    for pattern in ('zero', 'seq', 'random'):
        for level in (1, 6, 9):
            items.append(make_case(size, pattern, level, len(items)))

encoded = [base64.b64encode(item[2]).decode() for item in items]
run_dir = launch.new_run_dir('zip-anchor', 'headless')
archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
session = launch.open_session(run_dir, archive)
browser = None
oracle = None
try:
    _, browser = session.start_browser('headless')
    result = json.loads(session.sidecar(
        ['python3', '-B', '/zip/anchor-sidecar.py', json.dumps(encoded)],
        {root / 'oracle/harness/zip': '/zip:ro'}, timeout=120))
    oracle = peer(str(root / '.work/tmp/zip/oracle'))
    expected = []
    for blob, data, compressed in items:
        output = ask(oracle, blob)
        lines = dict(line.split(' ', 1) for line in output.decode().splitlines() if ' ' in line)
        kind, length, digest = lines['payload'].split()
        expected.append((kind, int(length), digest))
    oracle.stdin.close()
    if oracle.wait():
        raise RuntimeError('oracle failed')
    if len(result) != len(items):
        raise RuntimeError(f'anchor count {len(result)} != {len(items)}')
    for index, (item, observed, wanted) in enumerate(zip(items, result, expected, strict=True)):
        data = base64.b64decode(observed)
        actual = ('ok' if hashlib.sha256(data).hexdigest() == wanted[2] and len(data) == wanted[1] else 'fail',
                  len(data), hashlib.sha256(data).hexdigest())
        if actual != wanted or item[1] != data:
            raise RuntimeError('anchor mismatch: ' + json.dumps({
                'index': index, 'size': len(item[1]), 'harness': wanted, 'browser': actual}))
    print(f'anchor {len(items)}/{len(items)}: deflate-raw entry payloads')
    shutdown = session.probe('--close')
    if not shutdown.get('shutdown', {}).get('ok') or browser.wait(timeout=15) != 0:
        raise RuntimeError('browser shutdown failed')
finally:
    if oracle is not None:
        stop_peer(oracle)
    if browser is not None and browser.poll() is None:
        browser.kill()
        browser.wait()
    session.close()
