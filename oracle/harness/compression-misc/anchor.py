import json
import pathlib
import sys

import compare
import corpus

root = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(root / 'oracle/launch'))
import launch

run_dir = launch.new_run_dir('compression-misc-anchor', 'headless')
archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
session = launch.open_session(run_dir, archive)
browser = None
try:
    _, browser = session.start_browser('headless')
    workload = json.loads(session.sidecar(
        ['python3', '-B', '/leveldb/anchor-sidecar.py'],
        {root / 'oracle/harness/leveldb': '/leveldb:ro'}, timeout=120))
    shutdown = session.probe('--close')
    if not shutdown.get('shutdown', {}).get('ok') or browser.wait(timeout=15) != 0:
        raise RuntimeError('browser shutdown failed')
finally:
    if browser is not None and browser.poll() is None:
        browser.kill()
        browser.wait()
    session.close()


def varint(data, position):
    value = 0
    shift = 0
    for _ in range(10):
        byte = data[position]
        position += 1
        value |= (byte & 0x7f) << shift
        if byte < 0x80:
            return value, position
        shift += 7
    raise RuntimeError('bad varint')


def block_handle(data, position):
    offset, position = varint(data, position)
    size, position = varint(data, position)
    return offset, size, position


def index_candidates(table, value):
    found = []
    for position in range(len(value)):
        try:
            offset, next_position = varint(value, position)
            size, _ = varint(value, next_position)
        except IndexError:
            continue
        if (0 < offset and 0 < size and offset + size + 5 <= len(table)
                and table[offset + size] == 1):
            found.append((offset, size))
    return found


def decode(payload):
    request = corpus.decode_request(payload, 1 << 20)
    expected = compare.ask(oracle, request)
    actual = compare.ask(candidate, request)
    if expected != actual or expected[0] != 0:
        raise RuntimeError('oracle or candidate rejected or disagreed on a browser table block')
    return expected[5:]


def blocks(table):
    if len(table) < 48 or int.from_bytes(table[-8:], 'little') != 0xdb4775248b80fb57:
        raise RuntimeError('bad table footer')
    _, _, position = block_handle(table[-48:], 0)
    index_offset, index_size, _ = block_handle(table[-48:], position)
    index = table[index_offset:index_offset + index_size]
    if table[index_offset + index_size] == 1:
        index = decode(index)
    handles = []
    position = 0
    while position < len(index):
        key_length, position = varint(index, position)
        position += key_length
        value_length, position = varint(index, position)
        value = index[position:position + value_length]
        position += value_length
        handles.extend(index_candidates(table, value))
    for offset, size in sorted(set(handles)):
        kind = table[offset + size]
        yield kind, table[offset:offset + size]


oracle = compare.peer(compare.oracle)
candidate = compare.peer(compare.candidate)
decoded_blocks = 0
compressed_blocks = 0
marker_seen = False
databases = [path.parent for path in session.profile.rglob('CURRENT')
             if 'Local Storage' in path.parts or 'IndexedDB' in path.parts]
if not any('Local Storage' in path.parts for path in databases):
    raise RuntimeError('Local Storage database missing')
if not any('IndexedDB' in path.parts for path in databases):
    raise RuntimeError('IndexedDB database missing')
for database in databases:
    for table in sorted(database.glob('*.ldb')):
        for kind, payload in blocks(table.read_bytes()):
            if kind != 1:
                continue
            request = corpus.decode_request(payload, 1 << 20)
            expected = compare.ask(oracle, request)
            actual = compare.ask(candidate, request)
            if expected != actual:
                raise RuntimeError(f'Snappy block mismatch in {table.name}')
            if expected[0] != 0:
                continue
            plaintext = expected[5:]
            markers = (b'local-storage-', b'l\x00o\x00c\x00a\x00l\x00', b'profile-',
                       b'indexed-value-', b'i\x00n\x00d\x00e\x00x\x00e\x00d\x00', b'bulk-')
            if any(marker in plaintext for marker in markers):
                marker_seen = True
            recompress = corpus.compress_request(plaintext)
            if compare.ask(oracle, recompress) != compare.ask(candidate, recompress):
                raise RuntimeError(f'Snappy recompression mismatch in {table.name}')
            decoded_blocks += 1
            compressed_blocks += len(payload)
for peer in (oracle, candidate):
    peer.stdin.close()
    if peer.wait():
        raise RuntimeError('peer exited nonzero')
if decoded_blocks == 0 or not marker_seen:
    raise RuntimeError('no browser-written Snappy table block with profile text was decoded')
print(f'frozen browser workload {workload}')
print(f'compression-misc anchor: {decoded_blocks} Snappy table blocks, '
      f'{compressed_blocks} compressed bytes, browser text recovered, oracle and candidate agree')
