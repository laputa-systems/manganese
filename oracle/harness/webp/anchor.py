import base64
import json
import pathlib
import struct
import sys

import corpus

root = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(root / 'oracle/launch'))
sys.path.insert(0, str(root / 'oracle/harness/zlib'))
import launch
from compare import ask, peer


def b64(data):
    return base64.b64encode(data).decode()


def expectation(reply):
    offset = 0

    def take():
        nonlocal offset
        value = struct.unpack_from('>I', reply, offset)[0]
        offset += 4
        return value

    if not take():
        return {'failed': True, 'width': 0, 'height': 0, 'pixels': None}, 'decode failed'
    width, height, has_alpha, frames = (take() for _ in range(4))
    frame_width, frame_height, size = take(), take(), take()
    bgra = reply[offset:offset + size]
    if size != frame_width * frame_height * 4 or (frame_width, frame_height) != (width, height):
        return {'failed': False, 'width': width, 'height': height, 'pixels': 'unchecked'}, 'unchecked pixels'
    if has_alpha:
        return {'failed': False, 'width': width, 'height': height, 'pixels': 'unchecked'}, f'alpha={has_alpha} frames={frames}, pixels unchecked'
    rgba = bytearray()
    for b, g, r, a in zip(*[iter(bgra)] * 4):
        rgba.extend((r, g, b, a))
    return {'failed': False, 'width': width, 'height': height, 'pixels': b64(bytes(rgba))}, f'alpha=0 frames={frames}'


def anchor_images():
    images = []
    for index, (width, height, lossless, quality) in enumerate((
        (17, 13, 0, 1), (17, 13, 0, 50), (17, 13, 0, 92), (17, 13, 0, 100),
        (3, 5, 0, 80), (8, 8, 1, 100), (31, 29, 1, 100), (64, 63, 0, 75),
        (9, 7, 1, 100), (33, 17, 0, 95), (1, 1, 0, 90), (16, 16, 1, 100),
    )):
        images.append((f'generated-{index}', corpus.encode(400000 + index, width, height, lossless, quality)))
    fixtures = corpus.fixtures / 'libwebp-seeds'
    for name in ('lossless1.webp', 'alpha_filter_0_method_0.webp', 'lossy_alpha1.webp'):
        path = fixtures / name
        if path.is_file():
            images.append((name, path.read_bytes()))
    return images


images = anchor_images()
oracle = peer(str(root / '.work/tmp/webp/oracle'))
expected = []
for name, data in images:
    wanted, note = expectation(ask(oracle, corpus.request(data)))
    expected.append(wanted)
    print(f'{name}: {note}', flush=True)
oracle.stdin.close()
if oracle.wait():
    raise RuntimeError('oracle failed')

run_dir = launch.new_run_dir('webp-anchor', 'headless')
(run_dir / 'inputs.json').write_text(json.dumps([b64(data) for _, data in images]))
archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
session = launch.open_session(run_dir, archive)
browser = None
try:
    _, browser = session.start_browser('headless')
    result = json.loads(session.sidecar(['python3', '-B', '/webp/anchor-sidecar.py', '/inputs/inputs.json'],
                                        {root / 'oracle/harness/webp': '/webp:ro', run_dir: '/inputs:ro'}, timeout=600))
    if len(result) != len(images):
        raise RuntimeError('missing browser results')
    mismatches = []
    for (name, _), actual, wanted in zip(images, result, expected):
        if wanted['pixels'] == 'unchecked':
            actual = dict(actual, pixels='unchecked' if actual['pixels'] else None)
        if actual != wanted:
            mismatches.append(name)
            print('mismatch', name, json.dumps({key: (value if key != 'pixels' or value is None or len(value) < 64 else 'len=' + str(len(value))) for key, value in actual.items()}),
                  json.dumps({key: (value if key != 'pixels' or value is None or len(value) < 64 else 'len=' + str(len(value))) for key, value in wanted.items()}), flush=True)
    if mismatches:
        raise RuntimeError(f'{len(mismatches)} anchor mismatches: ' + ' '.join(mismatches))
    print(f'anchor {len(images)} WebP decodes agree; {sum(x["failed"] for x in expected)} failed decodes, '
          f'{sum(x["pixels"] == "unchecked" for x in expected)} with unchecked pixels')
    shutdown = session.probe('--close')
    if not shutdown.get('shutdown', {}).get('ok') or browser.wait(timeout=15) != 0:
        raise RuntimeError('browser shutdown failed')
finally:
    if browser is not None and browser.poll() is None:
        browser.kill()
        browser.wait()
    session.close()
