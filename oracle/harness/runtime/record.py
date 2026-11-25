import hashlib
import os
import pathlib
import sys
import tempfile

from cases import concurrency_and_bursts, corpus, extended, replies, reply_cancellations, shutdown_cases, shutdown_nested, tracked_callbacks

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / 'checksums'))
from compare import ask, peer

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
oracle = peer('.work/tmp/runtime/oracle')
rows = []
seen = set()


def blob(data: bytes) -> tuple[str, int]:
    digest = hashlib.sha256(data).hexdigest()
    path = store / digest
    if path.exists():
        if path.read_bytes() != data:
            raise SystemExit(f'corrupt artifact {digest}')
        return digest, len(data)
    staged = None
    try:
        with tempfile.NamedTemporaryFile(prefix='.runtime-', dir=store, delete=False) as temporary:
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


for lane, seed, count, extra, additional, shutdown, cancel, nested, parallel, tracked in [('seen', 88201, 120, 12, 20, 12, 6, 6, 8, 6), ('holdout', 88202, 72, 8, 15, 9, 4, 4, 8, 4)]:
    requests = corpus(seed, count) + replies(seed + 100, extra) + extended(seed + 200, additional) + shutdown_cases(seed + 300, shutdown) + reply_cancellations(seed + 400, cancel) + shutdown_nested(seed + 500, nested) + concurrency_and_bursts(seed + 600, parallel) + tracked_callbacks(seed + 700, tracked)
    for index, request in enumerate(requests):
        if request in seen:
            raise SystemExit(f'duplicate {lane} input {index}')
        seen.add(request)
        response = ask(oracle, request)
        source, target = blob(request), blob(response)
        rows.append(f'runtime {lane} {lane}-{index:03d} {source[0]} {source[1]} {target[0]} {target[1]}')
oracle.stdin.close()
if oracle.wait() != 0:
    raise SystemExit('oracle failed')
lock = pathlib.Path('goldens.lock')
previous = lock.read_text().splitlines()
missing = set(line for line in previous if line.startswith('runtime ')) - set(rows)
if missing:
    raise SystemExit(f'refusing to remove or change {len(missing)} existing runtime goldens')
existing = [line for line in previous if not line.startswith('runtime ')]
staged_lock = pathlib.Path('.work/tmp/runtime-goldens.lock')
staged_lock.write_text('\n'.join(existing + rows) + '\n')
os.replace(staged_lock, lock)
print('recorded', len(rows))
