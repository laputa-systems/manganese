import hashlib
import os
import pathlib
import struct
import subprocess

import corpus

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
oracle = subprocess.Popen(['.work/tmp/media-demux/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
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
names = set()
for lane in ('seen', 'holdout'):
    for name, data in corpus.lane_items(lane):
        item = f'{lane}-{name}'
        if item in names:
            raise RuntimeError(f'duplicate corpus item {item}')
        names.add(item)
        source, result = blob(data), blob(ask(data))
        rows.append(f'media-demux {lane} {item} {source[0]} {source[1]} {result[0]} {result[1]}')
oracle.stdin.close()
if oracle.wait():
    raise SystemExit('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('media-demux ')]
lock.write_text('\n'.join(existing + rows) + '\n')
print('recorded', len(rows))
