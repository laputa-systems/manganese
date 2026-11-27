import hashlib
import os
import pathlib
import random
import struct
import subprocess

from corpus import decode_request, make_gif, mutate, random_gif

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
oracle = subprocess.Popen(['.work/tmp/gif/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)


def stop_oracle():
    if oracle.poll() is None:
        oracle.kill()
    oracle.wait()


import atexit
atexit.register(stop_oracle)
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
seen_inputs: set[str] = set()


def add(lane: str, name: str, data: bytes) -> None:
    source = blob(data)
    if lane == 'holdout' and source[0] in seen_inputs:
        return
    result = blob(ask(data))
    if lane == 'seen':
        seen_inputs.add(source[0])
    rows.append(f'gif {lane} {lane}-{name} {source[0]} {source[1]} {result[0]} {result[1]}')


fixtures = sorted(path for path in pathlib.Path('.work/tmp/gif/fixtures').rglob('*.gif')
                  if path.stat().st_size < 40000 and 'anim' not in path.name.lower() and 'wheel' not in path.name.lower())
for lane, seed, rng_seed, random_count in (('seen', 10101, 20260103, 180), ('holdout', 20202, 20260204, 180)):
    for colors in (2, 4, 8, 16, 32, 256):
        for interlace in (False, True):
            for transparency in (False, True):
                data = make_gif(11 + colors % 5, 9, colors, seed + colors, interlace=interlace,
                                transparency=transparency, block_size=7)
                add(lane, f'static-{colors}-{int(interlace)}-{int(transparency)}',
                    decode_request(data, []))
    data = make_gif(23, 19, 8, seed, frames=1, transparency=True, disposal=2, block_size=1)
    for index, length in enumerate((1, 5, 12, 30, len(data) // 3, len(data) // 2, len(data) - 20, len(data) - 1)):
        add(lane, f'truncated-{index}', decode_request(data[:length], []))
    for index, position in enumerate((0, 5, 12, 25, len(data) // 2, len(data) - 1)):
        broken = bytearray(data)
        broken[position] ^= 0x40
        add(lane, f'corrupt-{index}', decode_request(broken, []))
    rng = random.Random(rng_seed)
    for index in range(random_count):
        data = random_gif(rng)
        if index % 3 == 2:
            data = mutate(data, rng)
        add(lane, f'random-{index}', decode_request(data, []))
    for index, path in enumerate(fixtures):
        if (index % 3 == 0) == (lane == 'holdout'):
            data = path.read_bytes()
            name = f'fixture-{index}-' + path.stem.replace('.', '-').lower()
            add(lane, name, decode_request(data, []))

oracle.stdin.close()
if oracle.wait():
    raise RuntimeError('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('gif ')]
lock.write_text('\n'.join(existing + rows) + '\n')
print('recorded', len(rows), 'seen', sum(' seen ' in row for row in rows), 'holdout', sum(' holdout ' in row for row in rows))
