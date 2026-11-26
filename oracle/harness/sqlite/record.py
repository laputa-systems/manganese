import hashlib
import os
import pathlib
import struct
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import cases

os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
scratch = root / '.work/tmp/sqlite/record'
scratch.mkdir(parents=True, exist_ok=True)

oracle = subprocess.Popen(['.work/tmp/sqlite/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, env=dict(os.environ, GOLDENS_SCRATCH=str(scratch)))
oracle.stdin.write(b'MGB1')
oracle.stdin.flush()
if oracle.stdout.read(4) != b'MGB1':
    raise SystemExit('oracle handshake failed')


def ask(request):
    oracle.stdin.write(struct.pack('>I', len(request)) + request)
    oracle.stdin.flush()
    header = oracle.stdout.read(4)
    if len(header) != 4:
        raise SystemExit(f'oracle stopped after a request: {oracle.stderr.read().decode(errors="replace")}')
    length = struct.unpack('>I', header)[0]
    response = oracle.stdout.read(length)
    if len(response) != length:
        raise SystemExit('oracle wrote a truncated response')
    return response


def blob(data):
    digest = hashlib.sha256(data).hexdigest()
    path = store / digest
    if not path.exists():
        path.write_bytes(data)
    return digest, len(data)


rows = []
seen_blobs = set()
for lane in ('seen', 'holdout'):
    skipped = 0
    for name, request in cases.scenarios(lane, ask):
        input_blob = blob(request)
        if lane == 'holdout' and input_blob[0] in seen_blobs:
            skipped += 1
            continue
        result = blob(ask(request))
        rows.append(f'sqlite {lane} {lane}-{name} {input_blob[0]} {input_blob[1]} {result[0]} {result[1]}')
        seen_blobs.add(input_blob[0])
        seen_blobs.add(result[0])
    print(f'{lane}: {sum(row.split()[1] == lane for row in rows)} items, {skipped} skipped', flush=True)

oracle.stdin.close()
if oracle.wait():
    raise SystemExit(f'oracle failed: {oracle.stderr.read().decode(errors="replace")}')

lock = root / 'goldens.lock'
existing = [line for line in lock.read_text().splitlines() if not line.startswith('sqlite ')]
lock.write_text('\n'.join(existing + rows) + '\n')
print(f'recorded {len(rows)} sqlite goldens')
