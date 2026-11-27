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


def cases():
    result = []
    for pattern, flag, text in corpus.FIXED:
        result.append({'pattern': pattern, 'flags': flag, 'subject': text, 'iterations': 3})
    rng = __import__('random').Random(4242)
    for _ in range(60):
        pattern, flag, text, iterations, _ = corpus.random_case(rng)
        result.append({'pattern': pattern, 'flags': flag, 'subject': text, 'iterations': iterations})
    return result


def main():
    items = cases()
    run_dir = launch.new_run_dir('regex-anchor', 'headless')
    (run_dir / 'inputs.json').write_text(json.dumps(items))
    archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
    session = launch.open_session(run_dir, archive)
    browser = None
    oracle2 = None
    candidate = None
    try:
        _, browser = session.start_browser('headless')
        result = json.loads(session.sidecar(
            ['python3', '-B', '/regex/anchor-sidecar.py', '/inputs/inputs.json'],
            {root / 'oracle/harness/regex': '/regex:ro', run_dir: '/inputs:ro'}, timeout=600))
        if len(result) != len(items):
            raise RuntimeError('missing browser results')
        oracle2 = peer(str(root / '.work/tmp/regex/oracle'))
        candidate = peer(str(root / '.work/tmp/target/x86_64-unknown-linux-musl/debug/regex'))
        mismatches = []
        for case, browser_result in zip(items, result):
            request = corpus.encode_request(case['pattern'], case['flags'], case['subject'], case['iterations'], 0)
            wanted = ask(oracle2, request).decode('utf-8')
            if wanted != browser_result:
                mismatches.append('{} oracle/browser: {!r} vs {!r}'.format(case['pattern'], wanted, browser_result))
            elif ask(candidate, request).decode('utf-8') != browser_result:
                mismatches.append('{} candidate/browser: {!r}'.format(case['pattern'], browser_result))
        if mismatches:
            raise RuntimeError('{} anchor mismatches: {}'.format(len(mismatches), ' '.join(mismatches[:5])))
        print('anchor {} ECMAScript RegExp results agree with the frozen browser and the source harness'.format(len(items)))
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
                print('peer cleanup: {}'.format(error), file=sys.stderr)
        if browser is not None and browser.poll() is None:
            browser.kill()
            browser.wait()
        session.close()


if __name__ == '__main__':
    main()
