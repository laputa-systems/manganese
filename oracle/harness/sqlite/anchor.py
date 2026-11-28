import hashlib
import json
import os
import pathlib
import shutil
import sys
import tarfile

root = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(root / 'oracle/launch'))
sys.path.insert(0, str(root / 'oracle/harness/sqlite'))
import launch
import profiles
import protocol

run_dir = launch.new_run_dir('sqlite-anchor', 'headless')
archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
session = launch.open_session(run_dir, archive)
browser = None
try:
    _, browser = session.start_browser('headless')
    result = json.loads(session.sidecar(['python3', '-B', '/sqlite/anchor-sidecar.py'],
                                       {root / 'oracle/harness/sqlite': '/sqlite:ro'}, timeout=180))
    shutdown = session.probe('--close')
    if not shutdown.get('shutdown', {}).get('ok') or browser.wait(timeout=15) != 0:
        raise RuntimeError('browser shutdown failed')
finally:
    if browser is not None and browser.poll() is None:
        browser.kill()
        browser.wait()
    session.close()

databases = profiles.find(session.profile)
if not databases:
    raise RuntimeError('no SQLite databases in the profile')
fixture = run_dir / 'profile-databases.tar'
with tarfile.open(fixture, 'w') as tar:
    for path in databases:
        for suffix in ('', '-journal', '-wal'):
            sidecar = path.with_name(path.name + suffix)
            if sidecar.exists():
                tar.add(sidecar, arcname=str(sidecar.relative_to(session.profile)))
digest = hashlib.sha256(fixture.read_bytes()).hexdigest()
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
shutil.copyfile(fixture, store / digest)

oracle = str(root / '.work/tmp/sqlite/oracle')
candidate = str(root / '.work/tmp/target/x86_64-unknown-linux-musl/debug/sqlite')
mismatches = []
for path in databases:
    name = str(path.relative_to(session.profile))
    files = {kind: path.with_name(path.name + suffix).read_bytes()
             for kind, suffix in ((0, ''), (1, '-journal'), (2, '-wal')) if path.with_name(path.name + suffix).exists()}
    request = profiles.unchanged_script(path.name, files).encode()
    expected = protocol.run(oracle, [request], str(run_dir / 'oracle-scratch'))[0]
    actual = protocol.run(candidate, [request], str(run_dir / 'candidate-scratch'))[0]
    reading = dict(protocol.decode(request, expected))
    if reading['open'][0] != 1 or reading['digest'] != (1, hashlib.sha256(files[0]).hexdigest()[:16]):
        raise RuntimeError(f'the source oracle did not open {name} unchanged: {reading["open"]} {reading["digest"]}')
    if expected != actual:
        mismatches.append((name, protocol.compare(request, expected, actual)))
        print(f'anchor {name}: MISMATCH {mismatches[-1][1]}')
        continue
    tables = [table for table in reading['dump']['tables'] if not table.startswith('sqlite_')]
    reopened = []
    for role, program in (('oracle', oracle), ('candidate', candidate)):
        write = profiles.write_script(path.name, files, tables).encode()
        exported = dict(protocol.decode(write, protocol.run(program, [write], str(run_dir / f'{role}-write'))[0]))['export']
        written = {kind: data for kind, (present, data) in enumerate(exported) if present}
        check = profiles.reopen_script(path.name, written).encode()
        reopened.append(protocol.run(oracle, [check], str(run_dir / f'{role}-reopen'))[0])
    if reopened[0] != reopened[1]:
        mismatches.append((name, protocol.compare(check, reopened[0], reopened[1])))
        print(f'anchor {name}: REOPEN MISMATCH {mismatches[-1][1]}')
        continue
    print(f'anchor {name}: {len(tables)} tables agree, file unchanged, candidate writes reopen in the oracle')
print(f'profile workload {result}; database archive sha256:{digest}; {len(databases) - len(mismatches)}/{len(databases)} agree')
if mismatches:
    raise SystemExit(1)
