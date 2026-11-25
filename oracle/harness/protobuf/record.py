import hashlib
import os
import pathlib
import random
import struct
import subprocess
import sys
from cases import generate

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
profile = pathlib.Path(sys.argv[1])
log = (profile / 'Default/shared_proto_db/metadata/000003.log').read_bytes()


def read_varint(data, pos):
    value = 0
    for shift in range(0, 70, 7):
        byte = data[pos]
        pos += 1
        value |= (byte & 127) << shift
        if byte < 128:
            return value, pos
    raise ValueError('invalid varint')


def profile_values(data):
    pos = 0
    while pos + 7 <= len(data):
        _, length, record_type = struct.unpack_from('<IHB', data, pos)
        pos += 7
        if record_type != 1 or pos + length > len(data):
            raise ValueError('unsupported or truncated LevelDB log record')
        batch = data[pos:pos + length]
        pos += length
        if len(batch) < 12:
            raise ValueError('truncated write batch')
        count = struct.unpack_from('<I', batch, 8)[0]
        offset = 12
        for _ in range(count):
            kind = batch[offset]
            offset += 1
            key_length, offset = read_varint(batch, offset)
            offset += key_length
            if kind != 1:
                raise ValueError('expected a LevelDB value')
            length, offset = read_varint(batch, offset)
            yield batch[offset:offset + length]
            offset += length
        if offset != len(batch):
            raise ValueError('trailing write batch data')
    if pos != len(data):
        raise ValueError('trailing log data')


def blob(data):
    digest = hashlib.sha256(data).hexdigest()
    (store / digest).write_bytes(data)
    return digest, len(data)


peer = subprocess.Popen(['.work/tmp/protobuf/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
peer.stdin.write(b'MGB1')
peer.stdin.flush()
if peer.stdout.read(4) != b'MGB1':
    raise ValueError('invalid oracle header')
rows = []
seen_blobs = set()
inputs = set()


def add(lane, name, data):
    if data in inputs or lane == 'holdout' and data in seen_blobs:
        return False
    peer.stdin.write(struct.pack('>I', len(data)) + data)
    peer.stdin.flush()
    header = peer.stdout.read(4)
    if len(header) != 4:
        raise ValueError('short oracle header')
    size = struct.unpack('>I', header)[0]
    output = peer.stdout.read(size)
    if len(output) != size:
        raise ValueError('short oracle output')
    inputs.add(data)
    if lane == 'seen':
        seen_blobs.update((data, output))
    a, b = blob(data), blob(output)
    rows.append(f'protobuf {lane} {lane}-{name} {a[0]} {a[1]} {b[0]} {b[1]}')
    return True


print('profile metadata log sha256', blob(log)[0])
for index, value in enumerate(profile_values(log)):
    add('seen', f'profile-{index}', bytes([0]) + value)
for depth in (1, 30, 99, 100, 101, 110):
    add('seen', f'group-depth-{depth}', b'\x00' + b'\x3b' * depth + b'\x3c' * depth)
for size in (127, 128, 16383, 16384, 1048576):
    add('seen', f'large-length-{size}', b'\x01\x12' + __import__('cases').varint(size) + bytes(size))
for lane, seed in [('seen', 56301), ('holdout', 56302)]:
    rng = random.Random(seed)
    added = 0
    while added < 500:
        if add(lane, f'generated-{added}', generate(rng)):
            added += 1
peer.stdin.close()
if peer.wait():
    raise SystemExit('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('protobuf ')]
lock.write_text('\n'.join(existing + rows) + '\n')
print('recorded', len(rows), 'items, including', sum('profile-' in row for row in rows), 'profile values')
