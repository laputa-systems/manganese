import hashlib
import itertools
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tarfile

root = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(root / 'oracle/launch'))
import launch

run_dir = launch.new_run_dir('leveldb-anchor', 'headless')
archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
session = launch.open_session(run_dir, archive)
browser = None
try:
    _, browser = session.start_browser('headless')
    result = json.loads(session.sidecar(['python3', '-B', '/leveldb/anchor-sidecar.py'],
                                       {root / 'oracle/harness/leveldb': '/leveldb:ro'}, timeout=120))
    shutdown = session.probe('--close')
    if not shutdown.get('shutdown', {}).get('ok') or browser.wait(timeout=15) != 0:
        raise RuntimeError('browser shutdown failed')
finally:
    if browser is not None and browser.poll() is None:
        browser.kill()
        browser.wait()
    session.close()

databases = [path.parent for path in session.profile.rglob('CURRENT')
             if 'Local Storage' in path.parts or 'IndexedDB' in path.parts]
if not any('Local Storage' in path.parts for path in databases):
    raise RuntimeError('localStorage database missing')
if not any('IndexedDB' in path.parts for path in databases):
    raise RuntimeError('IndexedDB database missing')

fixture = run_dir / 'profile-databases.tar'
with tarfile.open(fixture, 'w') as tar:
    for path in sorted(databases):
        tar.add(path, arcname=str(path.relative_to(session.profile)))
digest = hashlib.sha256(fixture.read_bytes()).hexdigest()
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
shutil.copyfile(fixture, store / digest)

oracle = root / '.work/tmp/leveldb/oracle'
candidate = root / '.work/tmp/target/x86_64-unknown-linux-musl/debug/leveldb'


copy_numbers = itertools.count()


def agree(command: str, path: pathlib.Path, label: str) -> list[str]:
    number = next(copy_numbers)
    copies = [run_dir / f'copy-{number}-{role}' for role in ('oracle', 'candidate')]
    for copy in copies:
        if path.is_dir():
            shutil.copytree(path, copy)
        else:
            shutil.copyfile(path, copy)
    reference = subprocess.run([str(oracle), command, str(copies[0])], capture_output=True, check=True, timeout=120).stdout
    actual = subprocess.run([str(candidate), command, str(copies[1])], capture_output=True, check=True, timeout=120).stdout
    if not reference or reference != actual:
        raise RuntimeError(f'{label} mismatch: {path.relative_to(session.profile)}')
    return reference.decode().splitlines()


entries = {'Local Storage': 0, 'IndexedDB': 0}
for database in sorted(databases):
    kind = 'Local Storage' if 'Local Storage' in database.parts else 'IndexedDB'
    if kind == 'Local Storage':
        lines = agree('read', database, 'database')
        if not lines or lines[-1] != 'status 0' or len(lines) < 2:
            raise RuntimeError(f'Local Storage database did not read back: {lines[-1:]}')
        print(f'anchor {database.relative_to(session.profile)}: {len(lines) - 1} entries agree')
    for file in sorted(database.iterdir()):
        if file.suffix in ('.ldb', '.sst'):
            lines = agree('read-table', file, 'table')
            count = sum(1 for line in lines if not line.startswith(('status ', 'open ')))
        elif file.suffix == '.log':
            lines = agree('read-log', file, 'log')
            count = sum(1 for line in lines if line.startswith(('put ', 'delete ')))
        else:
            continue
        if lines[-1] != 'status 0':
            raise RuntimeError(f'{file.relative_to(session.profile)} did not read cleanly: {lines[-1]}')
        entries[kind] += count
        print(f'anchor {file.relative_to(session.profile)}: {count} records agree')
for kind, count in entries.items():
    if count == 0:
        raise RuntimeError(f'no {kind} records were read from table or log files')
print(f'profile workload {result}; database archive sha256:{digest}')
