import base64
import json
import pathlib
import struct
import sys

from corpus import chunk, decode_request, make_png

root = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(root / 'oracle/launch'))
sys.path.insert(0, str(root / 'oracle/harness/zlib'))
import launch
from compare import ask, peer


def b64(data: bytes) -> str:
    return base64.b64encode(data).decode()


def binary_palette(data: bytes) -> bytes:
    start = data.index(b'tRNS')
    size = struct.unpack('>I', data[start - 4:start])[0]
    return data[:start - 4] + chunk(b'tRNS', bytes(0 if i % 2 == 0 else 255 for i in range(size))) + data[start + size + 8:]


images = [
    ('grayscale-trns', make_png(11, 9, 8, 0, False, 0, 3)),
    ('grayscale-16', make_png(11, 9, 16, 0, False, 1, 3)),
    ('rgb-16', make_png(11, 9, 16, 2, False, 2, 3)),
    ('palette-trns', binary_palette(make_png(11, 9, 4, 3, False, 3, 3))),
    ('adam7', make_png(11, 9, 8, 2, True, 4, 3)),
    ('apng-first', make_png(11, 9, 8, 2, False, 0, 3, animated=True)),
]
base = make_png(11, 9, 8, 2, False, 0, 3)
images += [('truncated-header', base[:20]), ('truncated-data', base[:len(base) // 2])]
exports = []
for width, height, transparent in ((1, 1, False), (11, 9, False), (13, 7, True)):
    pixels = bytes(v for i in range(width * height) for v in ((i * 37) % 256, (i * 71) % 256, (i * 19) % 256, 0 if transparent and i % 3 == 0 else 255))
    exports.append((width, height, pixels))

oracle = peer(str(root / '.work/tmp/png/oracle'))
expected = []
for name, data in images:
    reply = ask(oracle, decode_request(data, []))
    available, failed, width, height, count, _ = struct.unpack_from('>6I', reply)
    pixels = None
    if not name.startswith('truncated-') and (available, failed, count) != (1, 0, 1 if name != 'apng-first' else 7): raise RuntimeError('unexpected complete image state ' + name)
    if name == 'truncated-header' and (failed, count) != (1, 0): raise RuntimeError('unexpected truncated header state')
    if name == 'truncated-data' and (failed, count) != (1, 1): raise RuntimeError('unexpected truncated data state')
    if count:
        fields = struct.unpack_from('>11I', reply, 24)
        if not name.startswith('truncated-') and fields[0] != 3: raise RuntimeError('expected complete frame ' + name)
        if name == 'truncated-data' and fields[0] != 2: raise RuntimeError('expected partial frame')
        size = fields[10]
        bgra = reply[68:68 + size]
        if len(bgra) != size: raise RuntimeError('short frame ' + name)
        if size:
            if failed and fields[0] == 2:
                width, height = fields[8:10]
            rgba = bytearray()
            for b, g, r, a in zip(*[iter(bgra)] * 4):
                if a not in (0, 255): raise RuntimeError('inexact alpha ' + name)
                rgba.extend((r, g, b, a))
            pixels = b64(rgba)
    expected.append({'failed': bool(failed and pixels is None), 'width': width, 'height': height, 'pixels': pixels})
    print(f'{name}: harness failed={failed} frame_status={fields[0] if count else None} size={width}x{height}', flush=True)
encoded = []
for width, height, pixels in exports:
    bgra = bytes(v for r, g, b, a in zip(*[iter(pixels)] * 4) for v in ((b if a else 0), (g if a else 0), (r if a else 0), a))
    encoded.append(b64(ask(oracle, b'\x01' + struct.pack('>III', width, height, 0) + bgra)))
oracle.stdin.close()
if oracle.wait(): raise RuntimeError('oracle failed')

run_dir = launch.new_run_dir('png-anchor', 'headless')
archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
session = launch.open_session(run_dir, archive)
browser = None
try:
    _, browser = session.start_browser('headless')
    payload = [[b64(data) for _, data in images], [[w, h, b64(p)] for w, h, p in exports]]
    result = json.loads(session.sidecar(['python3', '-B', '/png/anchor-sidecar.py', json.dumps(payload)], {root / 'oracle/harness/png': '/png:ro'}, timeout=120))
    if len(result['decoded']) != len(images) or len(result['encoded']) != len(exports): raise RuntimeError('missing browser results')
    for (name, _), actual, wanted in zip(images, result['decoded'], expected):
        if actual != wanted: raise RuntimeError('decode mismatch ' + name + ': ' + json.dumps({'browser': actual, 'harness': wanted}))
    for index, (actual, wanted) in enumerate(zip(result['encoded'], encoded)):
        if actual != [wanted, wanted]: raise RuntimeError('export mismatch ' + str(index) + ': ' + json.dumps({'browser': actual, 'harness': wanted}))
    print(f'anchor {len(images)} decode cases and {len(exports)} canvas exports agree; {sum(x["failed"] for x in expected)} failed decodes')
    shutdown = session.probe('--close')
    if not shutdown.get('shutdown', {}).get('ok') or browser.wait(timeout=15) != 0: raise RuntimeError('browser shutdown failed')
finally:
    if browser is not None and browser.poll() is None:
        browser.kill()
        browser.wait()
    session.close()
