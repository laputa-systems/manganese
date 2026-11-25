import base64
import json
import pathlib
import sys
root = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(root / 'oracle/launch'))
import launch
from compare import ask, peer, request

inputs = [b'', b'a', b'hello world\n' * 100, bytes(range(256)) * 8]
encoded = [base64.b64encode(value).decode() for value in inputs]
run_dir = launch.new_run_dir('zlib-anchor', 'headless')
archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
session = launch.open_session(run_dir, archive)
browser = None
try:
    _, browser = session.start_browser('headless')
    result = json.loads(session.sidecar(['python3', '-B', '/zlib/anchor-sidecar.py', json.dumps(encoded)], {root / 'oracle/harness/zlib': '/zlib:ro'}, timeout=120))
    oracle = peer(str(root / '.work/tmp/zlib/oracle'))
    expected = [base64.b64encode(ask(oracle, request(0, 6, window, 8, 0, [(value, 4)]))[8:]).decode() for window in (15, -15, 31) for value in inputs]
    oracle.stdin.close()
    if oracle.wait(): raise RuntimeError('oracle failed')
    if result != expected: raise RuntimeError('anchor mismatch: ' + json.dumps({'browser':result, 'harness':expected}))
    print('anchor 12/12: deflate, deflate-raw, gzip')
    shutdown = session.probe('--close')
    if not shutdown.get('shutdown', {}).get('ok') or browser.wait(timeout=15) != 0: raise RuntimeError('browser shutdown failed')
finally:
    if browser is not None and browser.poll() is None:
        browser.kill()
        browser.wait()
    session.close()
