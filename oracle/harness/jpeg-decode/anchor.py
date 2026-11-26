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

ORIENTED = {5, 6, 7, 8}


def b64(data):
    return base64.b64encode(data).decode()


def oriented(rgba, width, height, orientation):
    if orientation == 1:
        return bytes(rgba), width, height
    out_width, out_height = (height, width) if orientation in ORIENTED else (width, height)
    result = bytearray(len(rgba))
    for y in range(out_height):
        for x in range(out_width):
            sx, sy = {2: (width - 1 - x, y), 3: (width - 1 - x, height - 1 - y), 4: (x, height - 1 - y),
                      5: (y, x), 6: (y, height - 1 - x), 7: (width - 1 - y, height - 1 - x),
                      8: (width - 1 - y, x)}[orientation]
            source = (sy * width + sx) * 4
            target = (y * out_width + x) * 4
            result[target:target + 4] = rgba[source:source + 4]
    return bytes(result), out_width, out_height


def expectation(reply):
    offset = 0

    def take():
        nonlocal offset
        value = struct.unpack_from('>I', reply, offset)[0]
        offset += 4
        return value

    if take() != 2:
        raise RuntimeError('anchor input is not sniffed as JPEG')
    available, header_failed = take(), take()
    if not available:
        return {'failed': True, 'width': 0, 'height': 0, 'pixels': None}, 'header-failed' if header_failed else 'header-incomplete'
    width, height, orientation, density_width, density_height = (take() for _ in range(5))
    icc = take()
    if icc != 0xffffffff:
        offset += icc
    if not take():
        raise RuntimeError('no frame decoder after the header')
    frame_failed, status = take(), take()
    natural = (density_height, density_width) if orientation in ORIENTED else (density_width, density_height)
    take()
    frame_width, frame_height, size = take(), take(), take()
    bgra = reply[offset:offset + size]
    if not size:
        transparent = b64(bytes(natural[0] * natural[1] * 4))
        return {'failed': False, 'width': natural[0], 'height': natural[1], 'pixels': transparent}, f'no frame, failed={frame_failed}'
    if icc != 0xffffffff or (density_width, density_height) != (width, height):
        return {'failed': False, 'width': natural[0], 'height': natural[1], 'pixels': 'unchecked'}, 'unchecked pixels'
    rgba = bytearray()
    for b, g, r, a in zip(*[iter(bgra)] * 4):
        rgba.extend((r, g, b, a))
    pixels, _, _ = oriented(rgba, frame_width, frame_height, orientation)
    return {'failed': False, 'width': natural[0], 'height': natural[1], 'pixels': b64(pixels)}, f'status={status} failed={frame_failed} orientation={orientation}'


def anchor_images():
    images = []
    tests = corpus.FIXTURES / 'libjpeg-turbo'
    for name in ('testorig.jpg', 'testout_420_islow_prog.jpg', 'testout_444_islow.jpg', 'testout_gray_islow.jpg',
                 'testout_rgb_islow.jpg', 'testout_422_ifast_opt.jpg', 'testimgari.jpg', 'testorig12.jpg'):
        images.append((name, (tests / name).read_bytes()))
    for h, v in ((1, 1), (2, 1), (1, 2), (2, 2), (4, 1), (4, 2), (3, 1), (1, 4)):
        images.append((f'sampling-{h}x{v}', corpus.image(37, 29, h * 10 + v, sampling=[(h, v), (1, 1), (1, 1)])))
    images.append(('chroma-larger', corpus.image(31, 23, 5, sampling=[(1, 1), (2, 2), (1, 2)])))
    images.append(('progressive-restart', corpus.image(45, 33, 6, progressive=True, restart=2)))
    images.append(('cmyk', corpus.image(23, 19, 7, jpeg_space=corpus.CMYK)))
    images.append(('ycck', corpus.image(23, 19, 8, jpeg_space=corpus.YCCK)))
    images.append(('lossless', corpus.image(21, 17, 9, predictor=1, sampling=[(1, 1)] * 3)))
    for orientation in range(1, 9):
        base = corpus.image(13, 7, 10 + orientation)
        images.append((f'orientation-{orientation}', corpus.insert_after_soi(base, [(0xE1, corpus.exif(orientation))])))
    base = corpus.image(40, 32, 12, progressive=True)
    images += [('truncated-header', base[:60]), ('truncated-scan', base[:len(base) * 2 // 3])]
    base = corpus.image(40, 32, 13)
    images.append(('truncated-baseline', base[:len(base) // 2]))
    return images


images = anchor_images()
oracle = peer(str(root / '.work/tmp/jpeg-decode/oracle'))
expected = []
for name, data in images:
    wanted, note = expectation(ask(oracle, corpus.request(data)))
    expected.append(wanted)
    print(f'{name}: {note}', flush=True)
oracle.stdin.close()
if oracle.wait():
    raise RuntimeError('oracle failed')

run_dir = launch.new_run_dir('jpeg-decode-anchor', 'headless')
(run_dir / 'inputs.json').write_text(json.dumps([b64(data) for _, data in images]))
archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
session = launch.open_session(run_dir, archive)
browser = None
try:
    _, browser = session.start_browser('headless')
    result = json.loads(session.sidecar(['python3', '-B', '/jpeg/anchor-sidecar.py', '/inputs/inputs.json'],
                                        {root / 'oracle/harness/jpeg-decode': '/jpeg:ro', run_dir: '/inputs:ro'}, timeout=600))
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
    print(f'anchor {len(images)} JPEG decodes agree; {sum(x["failed"] for x in expected)} failed decodes, '
          f'{sum(x["pixels"] == "unchecked" for x in expected)} with color-managed or density-scaled pixels unchecked')
    shutdown = session.probe('--close')
    if not shutdown.get('shutdown', {}).get('ok') or browser.wait(timeout=15) != 0:
        raise RuntimeError('browser shutdown failed')
finally:
    if browser is not None and browser.poll() is None:
        browser.kill()
        browser.wait()
    session.close()
