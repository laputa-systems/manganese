import atexit
import hashlib
import os
import pathlib
import struct
import subprocess
import tempfile

import corpus

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
scratch = pathlib.Path('.work/tmp/regex')
scratch.mkdir(parents=True, exist_ok=True)
oracle = subprocess.Popen(['.work/tmp/regex/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)


def stop_oracle():
    if oracle.poll() is None:
        oracle.kill()
    oracle.wait()


atexit.register(stop_oracle)
oracle.stdin.write(b'MGB1')
oracle.stdin.flush()
if oracle.stdout.read(4) != b'MGB1':
    raise RuntimeError('oracle protocol header')


def ask(data):
    oracle.stdin.write(struct.pack('>I', len(data)) + data)
    oracle.stdin.flush()
    header = oracle.stdout.read(4)
    if len(header) != 4:
        raise RuntimeError('oracle terminated')
    length = struct.unpack('>I', header)[0]
    result = oracle.stdout.read(length)
    if len(result) != length:
        raise RuntimeError('short oracle response')
    return result


def blob(data):
    digest = hashlib.sha256(data).hexdigest()
    path = store / digest
    if path.exists():
        if path.read_bytes() != data:
            raise RuntimeError('artifact digest collision')
        return digest, len(data)
    with tempfile.NamedTemporaryFile(dir=scratch, prefix='blob-', delete=False) as temporary:
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
            if path.read_bytes() != data:
                raise RuntimeError('artifact digest collision')
    finally:
        temporary_path.unlink()
    return digest, len(data)


rows = []
seen_inputs = set()


def add(lane, request):
    if lane == 'holdout' and request in seen_inputs:
        return False
    source = blob(request)
    result = blob(ask(request))
    if lane == 'seen':
        seen_inputs.add(request)
    rows.append('regex {} regex-{}-{} {} {} {} {}'.format(
        lane, lane, len(rows), source[0], source[1], result[0], result[1]))
    return True


for lane, count in (('seen', 1200), ('holdout', 640)):
    for request in corpus.cases(lane, count):
        add(lane, request)

oracle.stdin.close()
if oracle.wait():
    raise RuntimeError('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('regex ')]
if len(existing) != len(lock.read_text().splitlines()):
    raise RuntimeError('regex goldens already exist')
lock.write_text('\n'.join(existing + rows) + '\n')
print('recorded', len(rows), 'regex goldens')
