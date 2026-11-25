import hashlib
import os
import pathlib
import random
import subprocess
from compare import ask, peer, request

root = pathlib.Path('/lane')
os.chdir(root)
store = pathlib.Path('/store/sha256')
store.mkdir(parents=True, exist_ok=True)
scratch = root / '.work/tmp/zstd'
dictionary = (b'shared dictionary content and common phrases: ' * 512)
dict_path = scratch / 'dictionary'
dict_path.write_bytes(dictionary)
rng = random.Random(71421)
oracle = peer(str(scratch / 'oracle'))
rows = []

def blob(data):
    digest = hashlib.sha256(data).hexdigest()
    (store / digest).write_bytes(data)
    return digest, len(data)

def add(lane, name, chunks, raw_dict=b'', output_size=32768):
    if lane == 'holdout':
        output_size -= 1
    data = request(chunks, raw_dict, output_size)
    result = ask(oracle, data)
    a, b = blob(data), blob(result)
    if lane == 'holdout' and any(row.split()[1] == 'seen' and row.split()[5] == b[0] for row in rows):
        return
    rows.append(f'zstd {lane} {lane}-{name} {a[0]} {a[1]} {b[0]} {b[1]}')

def compress(data, level, options=()):
    return subprocess.run(['zstd', '-q', '-c', f'-{level}', *options], input=data, stdout=subprocess.PIPE, check=True).stdout

for lane, base in [('seen', b'seen text phrase '), ('holdout', b'holdout text phrase ')]:
    inputs = [b'', base, base * 12000, rng.randbytes(4096), bytes(range(256)) * 1024]
    for i, data in enumerate(inputs):
        for level in (1, 3, 9):
            encoded = compress(data, level, ('--no-check',) if i % 2 else ())
            add(lane, f'level-{i}-{level}', [encoded])
            add(lane, f'chunk-{i}-{level}', [encoded[j:j + 17] for j in range(0, len(encoded), 17)], output_size=256)
        encoded = compress(data, 3)
        add(lane, f'truncated-{i}', [encoded[:-1]])
        add(lane, f'trailing-{i}', [encoded + b'bad!'])
        add(lane, f'multiframe-{i}', [encoded + compress(base, 1)])
        add(lane, f'skippable-{i}', [bytes.fromhex('502a4d1803000000') + b'abc' + encoded])
        damaged = bytearray(encoded)
        if damaged:
            damaged[-1] ^= 1
        add(lane, f'corrupt-{i}', [damaged])
    dict_data = dictionary[-500:] * (2 if lane == 'seen' else 3)
    encoded = compress(dict_data, 5, ('-D', str(dict_path)))
    add(lane, 'dictionary', [encoded], dictionary)
    add(lane, 'dictionary-chunks', [encoded[j:j + 11] for j in range(0, len(encoded), 11)], dictionary, 128)
    add(lane, 'empty-input', [b''] if lane == 'seen' else [b'', b''])
    add(lane, 'invalid', [b'not a zstd frame' + base])
    long_frame = compress(base * 600000, 1, ('--long=24',))
    add(lane, 'large-window', [long_frame])

oracle.stdin.close()
if oracle.wait():
    raise SystemExit('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('zstd ')]
lock.write_text('\n'.join(existing + rows) + '\n')
print('recorded', len(rows))
