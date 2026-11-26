import hashlib
import os
import pathlib
import struct
import subprocess
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from wire import lane

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
    size = struct.unpack('>I', process.stdout.read(4))[0]
    return process.stdout.read(size)


def blob(data):
    digest = hashlib.sha256(data).hexdigest()
    path = store / digest
    if path.exists():
        if path.read_bytes() != data:
            raise SystemExit(f'corrupt artifact {digest}')
        return digest, len(data)
    staged = None
    try:
        with tempfile.NamedTemporaryFile(prefix='.cert-verify-', dir=store, delete=False) as temporary:
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


def read_cases(path):
    data = path.read_bytes()
    offset = 0
    while offset < len(data):
        size = struct.unpack('>H', data[offset:offset + 2])[0]
        label = data[offset + 2:offset + 2 + size].decode()
        offset += 2 + size
        size = struct.unpack('>I', data[offset:offset + 4])[0]
        yield label, data[offset + 4:offset + 4 + size]
        offset += 4 + size


oracle = peer('.work/tmp/cert-verify/oracle')
rows = []
seen_inputs = {}
for label, request in read_cases(pathlib.Path('.work/tmp/cert-verify/cases.bin')):
    if request in seen_inputs:
        continue
    seen_inputs[request] = label
    response = ask(oracle, request)
    if response.startswith(b'invalid '):
        raise SystemExit(f'{label}: the oracle rejected the record as {response.decode().strip()}')
    source, target = blob(request), blob(response)
    rows.append(f'cert-verify {lane(label)} {label} {source[0]} {source[1]} {target[0]} {target[1]}')
oracle.stdin.close()
if oracle.wait() != 0:
    raise SystemExit('oracle failed')
lock = pathlib.Path('goldens.lock')
previous = lock.read_text().splitlines()
missing = set(line for line in previous if line.startswith('cert-verify ')) - set(rows)
if missing:
    raise SystemExit(f'refusing to remove or change {len(missing)} existing cert-verify goldens')
existing = [line for line in previous if not line.startswith('cert-verify ')]
staged_lock = pathlib.Path('.work/tmp/cert-verify-goldens.lock')
staged_lock.write_text('\n'.join(existing + sorted(rows, key=lambda row: (row.split()[1], row.split()[2]))) + '\n')
os.replace(staged_lock, lock)
print('recorded', len(rows), 'holdout', sum(row.split()[1] == 'holdout' for row in rows))
