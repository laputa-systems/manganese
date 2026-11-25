#!/usr/bin/env -S python3 -B
import os
import pathlib
import random
import struct
from compare import ask, peer, request

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
seeds = []
for row in pathlib.Path('goldens.lock').read_text().splitlines():
    fields = row.split()
    if len(fields) != 7 or fields[:2] != ['brotli', 'seen'] or fields[2].startswith(('seen-decode-5-', 'seen-one-5-', 'seen-resource-5-', 'seen-patch-5-')):
        continue
    rec = (store / fields[3]).read_bytes()
    if len(rec) < 11:
        raise RuntimeError('short seen record')
    mode, count, output_size, dict_size = struct.unpack('>BHII', rec[:11])
    pos = 11
    dictionary = rec[pos:pos + dict_size]
    pos += dict_size
    chunks = []
    for _ in range(count):
        length = struct.unpack('>I', rec[pos:pos + 4])[0]
        pos += 4
        chunks.append(rec[pos:pos + length])
        pos += length
    if pos != len(rec):
        raise RuntimeError('invalid seen record')
    seeds.append((mode, output_size, dictionary, b''.join(chunks)))
if not seeds:
    raise RuntimeError('no seen inputs')
seed = int(os.environ.get('SEED') or 10023)
cases = int(os.environ.get('CASES') or 10000)
rng = random.Random(seed)
a, b = peer(os.environ['FUZZ_ORACLE']), peer(os.environ['FUZZ_CANDIDATE'])
print(f'fuzz box=brotli seed={seed} cases={cases}', flush=True)
for index in range(cases):
    mode, output_size, dictionary, encoded = rng.choice(seeds)
    data = bytearray(encoded)
    for _ in range(rng.randrange(1, 5)):
        op = rng.randrange(4)
        if op == 0 and data:
            data[rng.randrange(len(data))] ^= 1 << rng.randrange(8)
        elif op == 1 and data:
            del data[rng.randrange(len(data))]
        elif op == 2:
            data.insert(rng.randrange(len(data) + 1), rng.randrange(256))
        elif data:
            del data[rng.randrange(len(data)):]
    if mode == 0:
        width = min(32768, max((len(data) + 65534) // 65535, rng.choice([1, 2, 7, 17, 1024, max(1, len(data))])))
        chunks = [bytes(data[i:i + width]) for i in range(0, len(data), width)] or [b'']
        output_size = rng.choice([64, 257, 1024, 4096, output_size])
    else:
        chunks = [bytes(data)]
    rec = request(mode, output_size, dictionary, chunks)
    expected, actual = ask(a, rec), ask(b, rec)
    if expected != actual:
        print('divergence seed', seed, 'case', index, 'request', rec.hex(), 'oracle', expected.hex(), 'candidate', actual.hex(), flush=True)
        raise SystemExit(1)
for process in (a, b):
    process.stdin.close()
    if process.wait():
        raise SystemExit('peer failed')
print(f'fuzz passed {cases} cases seed={seed}')
