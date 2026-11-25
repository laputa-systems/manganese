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

scratch = root / '.work/tmp/brotli'
text = b'anchor document line\n' * 400 + bytes(range(256)) * 20
documents = [b'', b'a', text, text[:5000], bytes(range(256)) * 300, b'window-boundary-' * 20000]


def encode(data, quality, window):
    return subprocess.run([str(scratch / 'encode'), str(quality), str(window), '0'], input=data, stdout=subprocess.PIPE, check=True).stdout


streams = []
for quality, window in ((0, 16), (1, 20), (4, 22), (6, 22), (9, 24), (11, 24)):
    streams.extend(encode(data, quality, window) for data in documents)
encoded = encode(text, 6, 22)
streams += [b'', encoded[:-1], encoded[:len(encoded) // 2], encoded + b'trailing bytes', encoded[:40] + bytes([encoded[40] ^ 4]) + encoded[41:], encode(text, 6, 25), encode(text, 6, 30)]


def expected(stream):
    process = peer(str(scratch / 'oracle'))
    chunks = [stream[i:i + 32768] for i in range(0, len(stream), 32768)]
    reply = ask(process, request(0, 16384, b'', chunks))
    process.stdin.close()
    if process.wait():
        raise RuntimeError('oracle failed')
    body, position = b'', 1
    while True:
        (code,) = struct.unpack('>i', reply[position:position + 4])
        position += 4
        if code < 0:
            return None
        if code == 0:
            return base64.b64encode(body).decode()
        body += reply[position:position + code]
        position += code


run_dir = launch.new_run_dir('brotli-anchor', 'headless')
archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
session = launch.open_session(run_dir, archive)
browser = None
try:
    _, browser = session.start_browser('headless', ('--proxy-server=direct://',))
    payload = json.dumps([base64.b64encode(stream).decode() for stream in streams])
    observed = json.loads(session.sidecar(['python3', '-B', '/brotli/anchor-sidecar.py', payload], {root / 'oracle/harness/brotli': '/brotli:ro'}, timeout=180))
    harness = [expected(stream) for stream in streams]
    if observed != harness:
        raise RuntimeError('anchor mismatch: ' + json.dumps({index: [observed[index] and len(observed[index]), harness[index] and len(harness[index])] for index in range(len(streams)) if observed[index] != harness[index]}))
    print(f'anchor {len(streams)}/{len(streams)}: br response bodies, {sum(value is None for value in harness)} rejected streams')
    shutdown = session.probe('--close')
    if not shutdown.get('shutdown', {}).get('ok') or browser.wait(timeout=15) != 0:
        raise RuntimeError('browser shutdown failed')
finally:
    if browser is not None and browser.poll() is None:
        browser.kill()
        browser.wait()
    session.close()
