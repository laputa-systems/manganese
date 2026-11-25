import hashlib
import os
import pathlib
import random
import struct
import subprocess
import sys
from cases import field, generate, packed, varint

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
profile = pathlib.Path(sys.argv[1])
profile_archive = pathlib.Path(sys.argv[2])
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


print('profile archive sha256', blob(profile_archive.read_bytes())[0])
print('profile metadata log sha256', blob(log)[0])
for index, value in enumerate(profile_values(log)):
    add('seen', f'profile-{index}', bytes([0]) + value)
for depth in (1, 30, 99, 100, 101, 110):
    add('seen', f'group-depth-{depth}', b'\x00' + b'\x3b' * depth + b'\x3c' * depth)
    add('seen', f'nested-group-depth-{depth}', b'\x01' + packed(2, b'\x3b' * depth + b'\x3c' * depth))
for size in (127, 128, 16383, 16384, 1048576):
    add('seen', f'large-length-{size}', b'\x01\x12' + varint(size) + bytes(size))
    add('seen', f'large-value-{size}', b'\x01' + packed(2, packed(4, bytes(size))))
for last in (0, 1, 2, 127, 128, 255):
    add('seen', f'varint-tenth-{last}', b'\x00\x08' + b'\xff' * 9 + bytes([last]))
for value in (0, 4, 5, 2**31 - 1, 2**31, 2**32 - 1, 2**64 - 1):
    add('seen', f'enum-{value}', b'\x00' + field(3, 0, varint(value)))
for name, data in [
    ('zero-tag', b'\x00\x00'),
    ('max-tag', b'\x00' + field(2**29 - 1, 0, b'\x01')),
    ('tag-overflow', b'\x00' + varint(2**32) + b'\x00'),
    ('tag-overflow-nonzero', b'\x00' + varint(2**32 + 40) + b'\x01'),
    ('tag-overlong-six', b'\x00\x88\x80\x80\x80\x80\x00\x01'),
    ('length-overlong-six', b'\x00\x0a\x80\x80\x80\x80\x80\x00'),
    ('oversized-length', b'\x01\x12' + varint(2**31)),
    ('truncated-length', b'\x01\x12\x02\x0a'),
    ('wrong-wire', b'\x00' + packed(1, b'abc')),
    ('duplicate-origin', b'\x01' + packed(1, packed(1, b'https')) + packed(1, packed(2, b'example'))),
    ('invalid-utf8', b'\x01' + packed(2, packed(1, b'\xff\xfe'))),
    ('unknown-group', b'\x00\x3b\x40\x81\x00\x3c'),
    ('mismatched-group', b'\x00\x3b\x44'),
    ('sync-missing-origin', b'\x02'),
    ('sync-empty-origin', b'\x02' + packed(3, b'')),
    ('sync-missing-nested-required', b'\x02' + packed(1, packed(2, b't') + field(7, 0, b'\x01')) + packed(3, b'origin')),
    ('sync-complete', b'\x02' + packed(1, packed(2, b't') + field(7, 0, b'\x01') + field(8, 0, b'\x02')) + packed(3, b'origin')),
    ('sync-merge-options', b'\x02' + packed(1, packed(2, b't') + field(7, 0, b'\x01') + field(8, 0, b'\x02') + packed(9, field(1, 0, b'\x01') + field(7, 0, b'\x02')) + packed(9, field(1, 0, b'\x03'))) + packed(3, b'origin')),
    ('payment-unpacked', b'\x03' + field(8, 0, b'\x01') + field(8, 0, b'\x96\x01')),
    ('payment-packed', b'\x03' + packed(8, b'\x01\x96\x01')),
    ('payment-mixed', b'\x03' + packed(8, b'\x01\x96\x01') + field(8, 0, b'\x02')),
    ('payment-truncated-packed', b'\x03' + packed(8, b'\x80')),
    ('payment-overflow-packed', b'\x03' + packed(8, b'\xff' * 9 + b'\x02')),
    ('payment-nested', b'\x03' + packed(6, packed(1, b'icon') + packed(3, field(1, 0, b'\x10') + field(2, 0, b'\x20') + field(7, 0, b'\x03'))) + packed(7, b'image')),
]:
    add('seen', name, data)
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
