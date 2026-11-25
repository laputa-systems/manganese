import hashlib
import os
import pathlib
import random
import struct
import subprocess
import tempfile
from cases import generate_base, generate_blink, generate_json, generate_json_writer, generate_blink_wide, generate_format, generate_base_utf, generate_wide, generate_int_format, generate_wide_number

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
        return digest, len(data)
    with tempfile.NamedTemporaryFile(dir='.work/tmp/numerics', prefix='blob-', delete=False) as temporary:
        temporary_path = pathlib.Path(temporary.name)
        try:
            temporary.write(data)
            temporary.flush()
            os.fsync(temporary.fileno())
        except BaseException:
            temporary_path.unlink()
            raise
    try:
        try:
            os.link(temporary_path, path)
        except FileExistsError:
            if path.read_bytes() != data: raise RuntimeError(f'corrupt existing blob {digest}')
    finally:
        temporary_path.unlink()
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
for lane, seed in [('seen', 99101), ('holdout', 99102)]:
    rng = random.Random(seed)
    for value in (0.0, 1.0, 1e-6, 1e-5, 123456.5, 123457.5, 1e12, 1e20):
        value = -value if lane == 'holdout' else value
        bits = struct.unpack('>Q', struct.pack('>d', value))[0]
        add(lane, struct.pack('>BQBB', 15, bits, 0, 0))
        for mode in range(4):
            for digits in (0, 2, 6, 20): add(lane, struct.pack('>BQBB', 16, bits, mode, digits))
    count = 0
    while count < 500:
        count += add(lane, generate_format(rng))
for lane, seed in [('seen', 101101), ('holdout', 101102)]:
    rng = random.Random(seed)
    malformed = [b'\x80', b'\xc0\x80', b'\xe0\x80\x80', b'\xed\xa0\x80', b'\xf0\x80\x80\x80', b'\xf4\x90\x80\x80', b'\xf5\x80\x80\x80', b'\xe2\x82', b'\xef\xb7\x90', b'\xef\xbf\xbe']
    for data in malformed: add(lane, b'\x11' + (b'A' if lane == 'seen' else b'B') + data)
    for units in ([0xd800], [0xdc00], [0xd800, 0x41], [0xdc00, 0xd800], [0xd83d, 0xde00], [0, 0xffff]):
        add(lane, b'\x12' + struct.pack('>H', 65 if lane == 'seen' else 66) + b''.join(struct.pack('>H', unit) for unit in units))
    count = 0
    while count < 500:
        count += add(lane, generate_base_utf(rng))
for lane, seed in [('seen', 111101), ('holdout', 111102)]:
    rng = random.Random(seed)
    suffix = 65 if lane == 'seen' else 66
    for value in (0, 0x7f, 0x80, 0xd7ff, 0xd800, 0xdfff, 0xe000, 0xffff, 0x10000, 0x10ffff, 0x110000, 0xffffffff):
        for op in (20, 21): add(lane, bytes([op]) + struct.pack('>II', value, suffix))
    count = 0
    while count < 500:
        count += add(lane, generate_wide(rng))
for lane, seed in [('seen', 121101), ('holdout', 121102)]:
    rng = random.Random(seed)
    for value in (0, 1, 0x7fffffff, 0x80000000, 0xffffffff, 0x7fffffffffffffff, 0x8000000000000000, 0xffffffffffffffff):
        value ^= (1 << 63) if lane == 'holdout' else 0
        for op in range(23, 26):
            for kind in range(4): add(lane, struct.pack('>BBQ', op, kind, value))
    count = 0
    while count < 500:
        count += add(lane, generate_int_format(rng))
for lane, seed in [('seen', 131101), ('holdout', 131102)]:
    rng = random.Random(seed)
    for whitespace in (0x09, 0x20, 0x85, 0xa0, 0x1680, 0x180e, 0x2000, 0x2028, 0x205f, 0x3000, 0xfeff):
        digits = [0x31] if lane == 'seen' else [0x32]
        data = b''.join(struct.pack('>H', value) for value in [whitespace, *digits])
        add(lane, b'\x1a' + data)
        for kind in range(8): add(lane, b'\x1b' + bytes([kind]) + data)
    count = 0
    while count < 500:
        count += add(lane, generate_wide_number(rng))
proc.stdin.close()
if proc.wait(): raise SystemExit('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('numerics ')]
with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8', dir='.work/tmp/numerics', prefix='goldens-', delete=False) as temporary:
    temporary_path = pathlib.Path(temporary.name)
    try:
        temporary.write('\n'.join(existing + rows) + '\n')
        os.fchmod(temporary.fileno(), lock.stat().st_mode & 0o777)
        temporary.flush()
        os.fsync(temporary.fileno())
    except BaseException:
        temporary_path.unlink()
        raise
try:
    os.replace(temporary_path, lock)
finally:
    temporary_path.unlink(missing_ok=True)
print('recorded', len(rows), 'items')
