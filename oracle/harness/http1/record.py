import hashlib
import os
import pathlib
import struct
import subprocess
import sys
import tempfile

from cases import generate

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)


def peer(path):
    process = subprocess.Popen([path], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    process.stdin.write(b'MGB1')
    process.stdin.flush()
    if process.stdout.read(4) != b'MGB1':
        raise SystemExit(f'{path} wrote an invalid protocol header')
    return process


def ask(process, record):
    process.stdin.write(struct.pack('>I', len(record)) + record)
    process.stdin.flush()
    length = struct.unpack('>I', process.stdout.read(4))[0]
    return process.stdout.read(length)


def blob(data):
    digest = hashlib.sha256(data).hexdigest()
    path = store / digest
    if path.exists():
        if path.read_bytes() != data:
            raise SystemExit(f'corrupt artifact {digest}')
        return digest, len(data)
    staged = None
    try:
        with tempfile.NamedTemporaryFile(prefix='.http1-', dir=store, delete=False) as temporary:
            staged = pathlib.Path(temporary.name)
            temporary.write(data)
            temporary.flush()
            os.fsync(temporary.fileno())
        try:
            os.link(staged, path)
        except FileExistsError:
            if path.read_bytes() != data:
                raise SystemExit(f'corrupt artifact {digest}')
    finally:
        if staged is not None:
            staged.unlink(missing_ok=True)
    return digest, len(data)


oracle = peer('.work/tmp/http1/oracle')
rows = []
seen = set()
for lane in ('seen', 'holdout'):
    for name, request in generate(lane):
        if request in seen:
            continue
        seen.add(request)
        response = ask(oracle, request)
        if response.startswith(b'invalid '):
            raise SystemExit(f'{name}: the oracle rejected the record as {response.decode().strip()}')
        source, target = blob(request), blob(response)
        rows.append(f'http1 {lane} {name} {source[0]} {source[1]} {target[0]} {target[1]}')
oracle.stdin.close()
if oracle.wait() != 0:
    raise SystemExit('oracle failed')
lock = pathlib.Path('goldens.lock')
previous = lock.read_text().splitlines()
missing = set(line for line in previous if line.startswith('http1 ')) - set(rows)
if missing:
    raise SystemExit(f'refusing to remove or change {len(missing)} existing http1 goldens')
existing = [line for line in previous if not line.startswith('http1 ')]
staged_lock = pathlib.Path('.work/tmp/http1-goldens.lock')
staged_lock.write_text('\n'.join(existing + rows) + '\n')
os.replace(staged_lock, lock)
print('recorded', len(rows), 'holdout', sum(row.split()[1] == 'holdout' for row in rows))
