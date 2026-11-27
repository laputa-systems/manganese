import base64
import json
import pathlib
import struct
import subprocess
import sys

from corpus import decode_request, make_gif

root = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(root / 'oracle/launch'))
sys.path.insert(0, str(root / 'oracle/harness/zlib'))
import launch
from compare import ask, peer


def b64(data: bytes) -> str:
    return base64.b64encode(data).decode()


images = []
for colors in (2, 4, 16, 256):
    for interlace in (False, True):
        for transparency in (False, True):
            data = make_gif(11 + colors % 5, 9, colors, 10101 + colors,
                            interlace=interlace, transparency=transparency,
                            block_size=7)
            name = f'static-{colors}-{int(interlace)}-{int(transparency)}'
            images.append((name, data))
for width, height in ((1, 1), (13, 7), (23, 19)):
    images.append((f'size-{width}-{height}', make_gif(width, height, 8, 20202, block_size=1)))
base = make_gif(11, 9, 8, 10101, block_size=7)
images.append(('truncated-header', base[:12]))

run_dir = launch.new_run_dir('gif-anchor', 'headless')
archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
session = launch.open_session(run_dir, archive)
browser = None
oracle = None
try:
    oracle = peer(str(root / '.work/tmp/gif/oracle'))
    expected = []
    for name, data in images:
        reply = ask(oracle, decode_request(data, []))
        available, failed, width, height, count, repetition = struct.unpack_from('>6I', reply)
        pixels = None
        frame_status = None
        if count:
            fields = struct.unpack_from('>11I', reply, 24)
            frame_status = fields[0]
            bgra = reply[68:68 + fields[10]]
            if len(bgra) != fields[10]:
                raise RuntimeError('short frame ' + name)
            if bgra:
                rgba = bytearray()
                for b, g, r, a in zip(*[iter(bgra)] * 4):
                    if a not in (0, 255):
                        raise RuntimeError('inexact alpha ' + name)
                    rgba.extend((r, g, b, a))
                pixels = b64(rgba)
        if name.startswith('truncated-'):
            if failed != 1:
                raise RuntimeError('truncated case did not fail ' + name)
            expected.append({'failed': True, 'width': width, 'height': height, 'pixels': None})
        else:
            if (available, failed, count, frame_status) != (1, 0, 1, 3):
                raise RuntimeError('unexpected harness state ' + name)
            expected.append({'failed': False, 'width': width, 'height': height, 'pixels': pixels})
        print(f'{name}: harness failed={failed} frame_status={frame_status} size={width}x{height}', flush=True)
    oracle.stdin.close()
    if oracle.wait():
        raise RuntimeError('oracle failed')

    _, browser = session.start_browser('headless')
    payload = [[b64(data) for _, data in images]]
    result = json.loads(session.sidecar(['python3', '-B', '/gif/anchor-sidecar.py', json.dumps(payload)],
                                        {root / 'oracle/harness/gif': '/gif:ro'}, timeout=120))
    if not isinstance(result, list):
        raise RuntimeError('browser result: ' + json.dumps(result)[:2000])
    if len(result) != len(images):
        raise RuntimeError(f'missing browser results: got {len(result)} want {len(images)}: ' + json.dumps(result)[:2000])
    for (name, _), actual, wanted in zip(images, result, expected, strict=True):
        if actual != wanted:
            raise RuntimeError('decode mismatch ' + name + ': ' + json.dumps({'browser': actual, 'harness': wanted}))
    print(f'anchor {len(images)} decode cases agree; {sum(x["failed"] for x in expected)} failed decodes')
    shutdown = session.probe('--close')
    if not shutdown.get('shutdown', {}).get('ok') or browser.wait(timeout=15) != 0:
        raise RuntimeError('browser shutdown failed')
finally:
    if oracle is not None:
        try:
            if oracle.stdin is not None and not oracle.stdin.closed:
                oracle.stdin.close()
            if oracle.poll() is None:
                oracle.kill()
            oracle.wait(timeout=15)
        except (OSError, subprocess.TimeoutExpired) as error:
            print(f'oracle cleanup: {type(error).__name__}: {error}', file=sys.stderr)
    if browser is not None and browser.poll() is None:
        browser.kill()
        browser.wait()
    session.close()
