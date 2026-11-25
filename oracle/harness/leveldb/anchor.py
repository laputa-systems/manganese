import hashlib
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
for index, database in enumerate(sorted(databases)):
    copies = [run_dir / f'copy-{index}-{role}' for role in ('oracle', 'candidate')]
    for copy in copies:
        shutil.copytree(database, copy)
    reference = subprocess.run([str(oracle), 'read', str(copies[0])], capture_output=True, check=True, timeout=120).stdout
    actual = subprocess.run([str(candidate), 'read', str(copies[1])], capture_output=True, check=True, timeout=120).stdout
    if not reference or reference != actual:
        raise RuntimeError(f'profile database mismatch: {database.relative_to(session.profile)}')
    print(f'anchor {database.relative_to(session.profile)}: {len(reference.splitlines())} entries agree')
print(f'profile workload {result}; database archive sha256:{digest}')
