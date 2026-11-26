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


def cases():
    result = []
    sizes = [(1, 1), (3, 5), (8, 8), (17, 13), (64, 63), (129, 97)]
    qualities = [0.01, 0.5, 0.75, 0.92, 0.95, 1.0]
    patterns = ['flat', 'ramp-h', 'noise', 'alpha-gradient']
    for index, ((width, height), quality, pattern) in enumerate(
            (size, quality, pattern) for size in sizes for quality in qualities for pattern in patterns):
        result.append((f'{width}x{height}-q{quality}-{pattern}', width, height, quality,
                       corpus.pixels(width, height, 4200 + index, pattern)))
    return result


def request(width, height, quality, bgra):
    return b'\x01' + struct.pack('>III', width, height, quality) + bgra


def main():
    items = cases()
    run_dir = launch.new_run_dir('jpeg-encode-anchor', 'headless')
    browser_inputs = []
    for name, width, height, quality, bgra in items:
        rgba = bytearray()
        for b, g, r, a in zip(*[iter(bgra)] * 4):
            rgba.extend((r, g, b, a))
        browser_inputs.append([width, height, quality, b64(bytes(rgba))])
    (run_dir / 'inputs.json').write_text(json.dumps(browser_inputs))
    archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
    session = launch.open_session(run_dir, archive)
    browser = None
    oracle2 = None
    candidate = None
    try:
        _, browser = session.start_browser('headless')
        result = json.loads(session.sidecar(['python3', '-B', '/jpeg/anchor-sidecar.py', '/inputs/inputs.json'],
                                            {root / 'oracle/harness/jpeg-encode': '/jpeg:ro', run_dir: '/inputs:ro'}, timeout=600))
        if len(result) != len(items):
            raise RuntimeError('missing browser results')
        oracle2 = peer(str(root / '.work/tmp/jpeg-encode/oracle'))
        candidate = peer(str(root / '.work/tmp/target/x86_64-unknown-linux-musl/debug/jpeg-encode'))
        mismatches = []
        checked = 0
        for (name, width, height, quality, _), (readback, encoded) in zip(items, result):
            pixels = bytearray(base64.b64decode(readback))
            bgra = bytearray()
            for r, g, b, a in zip(*[iter(pixels)] * 4):
                bgra.extend((b, g, r, a))
            record = request(width, height, int(quality * 100 + 0.5), bytes(bgra))
            wanted = ask(oracle2, record)
            actual = base64.b64decode(encoded)
            checked += 1
            if wanted != actual:
                mismatches.append(f'{name} oracle/browser: {len(wanted)} vs {len(actual)} bytes')
            elif ask(candidate, record) != actual:
                mismatches.append(f'{name} candidate/browser')
        if mismatches:
            raise RuntimeError(f'{len(mismatches)} anchor mismatches: ' + ' '.join(mismatches[:10]))
        print(f'anchor {checked} JPEG exports agree with the frozen browser and the source harness')
        shutdown = session.probe('--close')
        if not shutdown.get('shutdown', {}).get('ok') or browser.wait(timeout=15) != 0:
            raise RuntimeError('browser shutdown failed')
    finally:
        for process in (oracle2, candidate):
            if process is None:
                continue
            try:
                if process.stdin is not None and not process.stdin.closed:
                    process.stdin.close()
                if process.poll() is None:
                    process.kill()
                process.wait(timeout=15)
            except OSError as error:
                print(f'peer cleanup: {error}', file=sys.stderr)
        if browser is not None and browser.poll() is None:
            browser.kill()
            browser.wait()
        session.close()


if __name__ == '__main__':
    main()
