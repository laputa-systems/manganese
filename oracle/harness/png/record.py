import hashlib
import os
import pathlib
import struct
import subprocess

from corpus import decode_request, encode_request, make_png

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ['MANGANESE_STORE']) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
oracle = subprocess.Popen(['.work/tmp/png/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
oracle.stdin.write(b'MGB1')
oracle.stdin.flush()
if oracle.stdout.read(4) != b'MGB1':
    raise RuntimeError('oracle protocol header')


def ask(data):
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


def blob(data):
    digest = hashlib.sha256(data).hexdigest()
    path = store / digest
    if path.exists() and path.read_bytes() != data:
        raise RuntimeError('artifact digest collision')
    if not path.exists():
        path.write_bytes(data)
    return digest, len(data)


rows = []


def add(lane, name, data):
    source = blob(data)
    result = blob(ask(data))
    rows.append(f'png {lane} {lane}-{name} {source[0]} {source[1]} {result[0]} {result[1]}')


layouts = [(0, 1), (0, 2), (0, 4), (0, 8), (0, 16), (2, 8), (2, 16),
           (3, 1), (3, 2), (3, 4), (3, 8), (4, 8), (4, 16), (6, 8), (6, 16)]
for lane, seed in [('seen', 31), ('holdout', 97)]:
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
        add(lane, f'truncated-{index}', decode_request(base[:length], [length // 2]))
    for index, position in enumerate((0, 12, 25, len(base) // 2, len(base) - 1)):
        broken = bytearray(base)
        broken[position] ^= 0x80
        add(lane, f'corrupt-{index}', decode_request(broken, [len(broken) // 2]))
    for mode in range(3):
        for width, height in ((1, 1), (3, 5), (64, 63), (256, 129)):
            for opaque in (False, True):
                add(lane, f'encode-{mode}-{width}-{height}-{int(opaque)}', encode_request(width, height, mode, seed, opaque))

fixtures = pathlib.Path('/work/src/third_party/skia/resources/images')
names = [('seen', 'color_wheel_with_profile.png'), ('seen', 'plte_trns_gama.png'),
         ('seen', 'apng-test-suite--blend-ops--over-on-solid-and-transparent.png'),
         ('seen', 'pngsuite/basn2c16.png'), ('holdout', 'purple-displayprofile.png'),
         ('holdout', 'apng-test-suite--basic--ignoring-default-image.png'),
         ('holdout', 'interlaced-multiframe-with-blending.png'), ('holdout', 'pngsuite/basn3p01.png')]
for lane, name in names:
    data = (fixtures / name).read_bytes()
    add(lane, 'fixture-' + name.replace('/', '-').replace('.', '-'), decode_request(data, [8, 33, len(data) // 2]))

oracle.stdin.close()
if oracle.wait():
    raise RuntimeError('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('png ')]
if len(existing) != len(lock.read_text().splitlines()):
    raise RuntimeError('PNG goldens already exist')
lock.write_text('\n'.join(existing + rows) + '\n')
print('recorded', len(rows), 'PNG goldens')
