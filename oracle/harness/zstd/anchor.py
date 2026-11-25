import base64
import json
import pathlib
import struct
import subprocess
import sys
root = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(root / 'oracle/launch'))
import launch
from compare import ask, peer, request

scratch = root / '.work/tmp/zstd'
tag = subprocess.run(['sh', 'oracle/source-oracle', 'image-tag'], cwd=root, stdout=subprocess.PIPE, text=True, check=True).stdout.strip()
encoded = subprocess.run(['docker', 'run', '--rm', '--mount', f'type=bind,src={root / "oracle/harness/zstd"},dst=/zstd,readonly', tag, 'python3', '-B', '/zstd/anchor-streams.py'], stdout=subprocess.PIPE, text=True, check=True)
streams = [(name, base64.b64decode(value)) for name, value in json.loads(encoded.stdout)]


def expected(stream: bytes) -> tuple[int, str]:
    process = peer(str(scratch / 'oracle'))
    chunks = [stream[i:i + 32768] for i in range(0, len(stream), 32768)] or [b'']
    reply = ask(process, request(chunks))
    process.stdin.close()
    if process.wait():
        raise RuntimeError('oracle failed')
    status = struct.unpack_from('>I', reply)[0]
    body, position = b'', 4
    for _ in chunks:
        length = struct.unpack_from('>I', reply, position)[0]
        position += 4
        body += reply[position:position + length]
        position += length
    if position != len(reply):
        raise RuntimeError('invalid oracle response')
    return status, base64.b64encode(body).decode()


run_dir = launch.new_run_dir('zstd-anchor', 'headless')
archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
session = launch.open_session(run_dir, archive)
browser = None
try:
    _, browser = session.start_browser('headless', ('--proxy-server=direct://',))
    payload = json.dumps([base64.b64encode(stream).decode() for _, stream in streams])
    result = json.loads(session.sidecar(['python3', '-B', '/zstd/anchor-sidecar.py', payload], {root / 'oracle/harness/zstd': '/zstd:ro'}, timeout=180))
    observed = result['observed']
    harness = [expected(stream) for _, stream in streams]
    headers = result['accept_encoding']
    if len(observed) != len(streams) or len(headers) != len(streams) or any('zstd' not in [part.strip() for part in header.split(',')] for header in headers.values()):
        raise RuntimeError('browser did not advertise zstd on every localhost request: ' + json.dumps(headers))
    mismatch = {}
    for index, ((name, _), actual, (status, body)) in enumerate(zip(streams, observed, harness)):
        if (status and actual is not None) or (not status and actual != body):
            mismatch[name] = {'index': index, 'browser': None if actual is None else len(base64.b64decode(actual)), 'oracle_status': status, 'oracle_bytes': len(base64.b64decode(body))}
    if mismatch:
        raise RuntimeError('anchor mismatch: ' + json.dumps(mismatch))
    print(f'anchor {len(streams)}/{len(streams)}: localhost Content-Encoding zstd, Accept-Encoding includes zstd, {sum(value is None for value in observed)} rejected streams')
    shutdown = session.probe('--close')
    if not shutdown.get('shutdown', {}).get('ok') or browser.wait(timeout=15) != 0:
        raise RuntimeError('browser shutdown failed')
finally:
    if browser is not None and browser.poll() is None:
        browser.kill()
        browser.wait()
    session.close()
