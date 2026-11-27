import hashlib
import os
import pathlib
import subprocess

import corpus

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
scratch = pathlib.Path('.work/tmp/structured-data-store')
scratch.mkdir(parents=True, exist_ok=True)
oracle = subprocess.Popen(['.work/tmp/structured-data/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)


def stop():
    if oracle.poll() is None:
        oracle.kill()
    oracle.wait()


import atexit
atexit.register(stop)
oracle.stdin.write(b'MGB1')
oracle.stdin.flush()
if oracle.stdout.read(4) != b'MGB1':
    raise RuntimeError('oracle protocol header')


def ask(data):
    oracle.stdin.write(corpus.u32(len(data)) + data)
    oracle.stdin.flush()
    size = oracle.stdout.read(4)
    if len(size) != 4:
        raise RuntimeError('oracle terminated')
    length = int.from_bytes(size, 'big')
    result = oracle.stdout.read(length)
    if len(result) != length:
        raise RuntimeError('short oracle response')
    return result


def save(data):
    digest = hashlib.sha256(data).hexdigest()
    target = store / digest
    if target.exists():
        if target.read_bytes() != data:
            raise RuntimeError(f'corrupt stored blob {digest}')
        return digest, len(data)
    temporary = scratch / f'{digest}.{os.getpid()}'
    temporary.write_bytes(data)
    try:
        try:
            os.link(temporary, target)
        except FileExistsError:
            if target.read_bytes() != data:
                raise RuntimeError(f'corrupt stored blob {digest}')
    finally:
        temporary.unlink()
    return digest, len(data)


rows = []
seen_inputs = set()
seen_outputs = set()
for lane, seed in (('seen', 4103), ('holdout', 8719)):
    used = set()
    skipped = 0
    for name, data in corpus.cases(lane, seed):
        digest = hashlib.sha256(data).digest()
        if digest in used or (lane == 'holdout' and (digest in seen_inputs or digest in seen_outputs)):
            skipped += 1
            continue
        used.add(digest)
        source = save(data)
        result = save(ask(data))
        if lane == 'seen':
            seen_inputs.add(digest)
            seen_outputs.add(bytes.fromhex(result[0]))
        rows.append(f'structured-data {lane} {lane}-{name} {source[0]} {source[1]} {result[0]} {result[1]}')
    print(f'recorded {lane} skipped {skipped}', flush=True)

oracle.stdin.close()
if oracle.wait():
    raise RuntimeError('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('structured-data ')]
if len(existing) != len(lock.read_text().splitlines()):
    raise RuntimeError('structured-data goldens already exist')
temporary = scratch / f'goldens.lock.{os.getpid()}'
temporary.write_text('\n'.join(existing + rows) + '\n')
os.replace(temporary, lock)
temporary.unlink(missing_ok=True)
print('recorded', len(rows), 'structured-data goldens')
