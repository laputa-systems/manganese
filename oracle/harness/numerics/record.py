import hashlib
import os
import pathlib
import random
import struct
import subprocess
from cases import generate_base, generate_blink, generate_json, generate_json_writer, generate_blink_wide

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
proc = subprocess.Popen(['.work/tmp/numerics/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
proc.stdin.write(b'MGB1')
proc.stdin.flush()
if proc.stdout.read(4) != b'MGB1': raise SystemExit('bad oracle handshake')
rows = []
seen_blobs = set()


def blob(data: bytes) -> tuple[str, int]:
    digest = hashlib.sha256(data).hexdigest()
    path = store / digest
    if path.exists():
        if path.read_bytes() != data: raise RuntimeError(f'corrupt existing blob {digest}')
    else:
        path.write_bytes(data)
    return digest, len(data)


def add(lane: str, request: bytes) -> bool:
    if lane == 'holdout' and request in seen_blobs: return False
    proc.stdin.write(struct.pack('>I', len(request)) + request)
    proc.stdin.flush()
    header = proc.stdout.read(4)
    if len(header) != 4: raise RuntimeError('short oracle response')
    length = struct.unpack('>I', header)[0]
    response = proc.stdout.read(length)
    if len(response) != length: raise RuntimeError('short oracle response')
    if lane == 'seen': seen_blobs.update((request, response))
    a, b = blob(request), blob(response)
    rows.append(f'numerics {lane} {lane}-{len(rows)} {a[0]} {a[1]} {b[0]} {b[1]}')
    return True


for lane, seed in [('seen', 44101), ('holdout', 44102)]:
    rng = random.Random(seed)
    lane_variant = 0 if lane == 'seen' else 1
    floats = [0, 1, 0x0010000000000000, 0x3ff0000000000000, 0x3ff4000000000000, 0x4340000000000000, 0x7fefffffffffffff, 0x7ff0000000000000, 0x7ff8000000000000, 0x43093b6e37e9d2e2]
    for bits in floats:
        bits ^= lane_variant << 63
        for op in range(5):
            for digits in ((1, 2, 3, 18, 101) if op == 1 else (2, 3, 18, 121) if op == 3 else (0, 1, 2, 18, 121) if op == 4 else (1,)):
                add(lane, struct.pack('>BQH', op, bits, digits))
    malformed = [b'\x80', b'\xc0\x80', b'\xe0\x80\x80', b'\xed\xa0\x80', b'\xf0\x80\x80\x80', b'\xf4\x90\x80\x80', b'\xf5\x80\x80\x80', b'\xe2\x82', b'\xef\xb7\x90', b'\xef\xbf\xbe']
    for data in malformed:
        add(lane, b'\x09' + bytes([65 + lane_variant]) + data)
    for units in ([0xd800], [0xdc00], [0xd800, 0x41], [0xdc00, 0xd800], [0xd83d, 0xde00], [0, 0xffff]):
        add(lane, b'\x06' + struct.pack('>H', 65 + lane_variant) + b''.join(struct.pack('>H', unit) for unit in units))
    for data in (b'', b' ', b'+', b'-', b'1e', b'1e-', b'.5', b'-0', b'1e309', b'1e-999', b' 1', b'1 ', b'0x20', b'18446744073709551616', b'-9223372036854775809'):
        data += b'!' * lane_variant
        for op in (5, 8): add(lane, bytes([op]) + data)
        for kind in range(8): add(lane, bytes([7, kind]) + data)
    count = 0
    while count < 1500:
        count += add(lane, generate_base(rng))
for lane, seed in [('seen', 55101), ('holdout', 55102)]:
    rng = random.Random(seed)
    count = 0
    while count < 500:
        count += add(lane, generate_blink(rng))
for lane, seed in [('seen', 66101), ('holdout', 66102)]:
    rng = random.Random(seed)
    values = (b'0', b'-0', b'1.0', b'01', b'1e', b'1e999', b'1e-999', b'2147483647', b'2147483648', b'-2147483648', b'-2147483649', b'18446744073709551615', b'18446744073709551616')
    for value in values:
        add(lane, b'\x0c' + (b' ' if lane == 'holdout' else b'') + value)
    count = 0
    while count < 500:
        count += add(lane, generate_json(rng))
for lane, seed in [('seen', 77101), ('holdout', 77102)]:
    rng = random.Random(seed)
    for bits in (0, 1, 0x3ff0000000000000, 0x43e0000000000000, 0x7fefffffffffffff, 0x7ff0000000000000, 0x7ff8000000000000):
        bits ^= (1 << 63) if lane == 'holdout' else 0
        for option in range(2): add(lane, struct.pack('>BQBB', 13, bits, option, 0))
    count = 0
    while count < 500:
        count += add(lane, generate_json_writer(rng))
for lane, seed in [('seen', 88101), ('holdout', 88102)]:
    rng = random.Random(seed)
    for length in (0, 1, 63, 64, 65, 127, 128):
        units = [ord('1')] * length + ([0x00a0] if lane == 'seen' else [0x2000])
        add(lane, b'\x0e' + b''.join(struct.pack('>H', unit) for unit in units))
    count = 0
    while count < 500:
        count += add(lane, generate_blink_wide(rng))
proc.stdin.close()
if proc.wait(): raise SystemExit('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('numerics ')]
lock.write_text('\n'.join(existing + rows) + '\n')
print('recorded', len(rows), 'items')
