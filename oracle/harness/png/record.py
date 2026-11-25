import hashlib
import os
import pathlib
import random
import struct
import subprocess

from corpus import decode_request, encode_request, make_png, mutate, random_png

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ['MANGANESE_STORE']) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
oracle = subprocess.Popen(['.work/tmp/png/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
oracle.stdin.write(b'MGB1')
oracle.stdin.flush()
if oracle.stdout.read(4) != b'MGB1':
    raise RuntimeError('oracle protocol header')


def ask(data: bytes) -> bytes:
    oracle.stdin.write(struct.pack('>I', len(data)) + data)
    oracle.stdin.flush()
    size = oracle.stdout.read(4)
    if len(size) != 4:
        raise RuntimeError('oracle terminated')
    count = struct.unpack('>I', size)[0]
    result = oracle.stdout.read(count)
    if len(result) != count:
        raise RuntimeError('short oracle response')
    return result


def blob(data: bytes) -> tuple[str, int]:
    digest = hashlib.sha256(data).hexdigest()
    path = store / digest
    if path.exists() and path.read_bytes() != data:
        raise RuntimeError('artifact digest collision')
    if not path.exists():
        path.write_bytes(data)
    return digest, len(data)


rows = []


def add(lane: str, name: str, data: bytes) -> None:
    source = blob(data)
    result = blob(ask(data))
    rows.append(f'png {lane} {lane}-{name} {source[0]} {source[1]} {result[0]} {result[1]}')


layouts = [(0, 1), (0, 2), (0, 4), (0, 8), (0, 16), (2, 8), (2, 16),
           (3, 1), (3, 2), (3, 4), (3, 8), (4, 8), (4, 16), (6, 8), (6, 16)]
fixtures = sorted(pathlib.Path('.work/tmp/png/fixtures').glob('*.png'))
lanes = [('seen', 31, 20260101, 150), ('holdout', 97, 20260202, 120)]
for lane, seed, random_seed, random_count in lanes:
    for color, depth in layouts:
        for interlaced in (False, True):
            for filter_kind in range(5):
                data = make_png(11 + seed % 3, 9 + seed % 5, depth, color, interlaced, filter_kind, seed)
                add(lane, f'layout-{color}-{depth}-{int(interlaced)}-{filter_kind}', decode_request(data, [8, 32, len(data) // 2]))
    for index, color_chunk in enumerate(('gamma', 'srgb', 'chromaticity', 'icc')):
        data = make_png(9, 7, 8, 6, False, index, seed, color_chunk=color_chunk)
        add(lane, f'color-{color_chunk}', decode_request(data, [8, 33, len(data) // 2]))
    for interlaced in (False, True):
        data = make_png(9, 8, 8, 6, interlaced, seed % 5, seed, animated=True)
        add(lane, f'apng-{int(interlaced)}', decode_request(data, [8, 33, len(data) // 4, len(data) // 2, 3 * len(data) // 4]))
    data = make_png(1024, 513, 8, 6, False, 4, seed)
    add(lane, 'large', decode_request(data, [33, len(data) // 2]))
    base = make_png(17, 13, 8, 6, False, 1, seed, animated=True)
    for index, length in enumerate((1, 8, 20, 33, len(base) // 3, len(base) // 2, len(base) - 12, len(base) - 1)):
        add(lane, f'truncated-{index}', decode_request(base[:length], [length // 2] if lane == 'seen' else [length // 3, length // 2]))
    for index, position in enumerate((0, 12, 25, len(base) // 2, len(base) - 1)):
        broken = bytearray(base)
        broken[position] ^= 0x80
        add(lane, f'corrupt-{index}', decode_request(broken, [len(broken) // 2]))
    for operation in range(5):
        for width, height in ((1, 1), (3, 5), (64, 63), (256, 129)):
            for kind in range(3):
                add(lane, f'encode-{operation}-{width}-{height}-{kind}', encode_request(width, height, operation, seed + operation, kind))
    for operation in range(5):
        add(lane, f'encode-large-{operation}', encode_request(300 + seed % 7, 200 + seed % 5, operation, seed, operation % 3))
    rng = random.Random(random_seed)
    for index in range(random_count):
        data = random_png(rng)
        if index % 3 == 2:
            data = mutate(data, rng)
        cuts = sorted(rng.randrange(len(data) + 1) for _ in range(rng.randrange(0, 5)))
        add(lane, f'random-{index}', decode_request(data, cuts))
    for index, path in enumerate(fixtures):
        if (index % 3 == 0) == (lane == 'holdout'):
            data = path.read_bytes()
            name = f'{index}-' + path.stem.replace('.', '-').lower()
            add(lane, f'fixture-{name}', decode_request(data, [8, 33, len(data) // 2]))
            add(lane, f'fixture-whole-{name}', decode_request(data, []))

oracle.stdin.close()
if oracle.wait():
    raise RuntimeError('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('png ')]
if len(existing) != len(lock.read_text().splitlines()):
    raise RuntimeError('PNG goldens already exist')
lock.write_text('\n'.join(existing + rows) + '\n')
print('recorded', len(rows), 'PNG goldens')
