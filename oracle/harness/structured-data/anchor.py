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

JSON_CASES = [
    '"hello"', '["a","b"]', '{"a":"x"}', '{"a":["b","c"]}', '[]', '{}',
    '"a\\nb"', '"a\\tb"', '"\\"quoted\\""', '"back\\\\slash"',
]
XML_CASES = [
    '<a/>', '<a>t</a>', '<a x="1">t</a>', '<a><b>t</b></a>',
    '<a xmlns:p="urn:x"><p:b>t</p:b></a>', '<a>&amp;&#65;</a>',
    '<a><!-- c --></a>', '<a><![CDATA[z]]></a>', '<a>line\nline2</a>',
]


def word(data, position):
    return int.from_bytes(data[position:position + 4], 'big')


def skip_blob(data, position):
    return position + 4 + word(data, position)


def skip_events(data, position, count):
    for _ in range(count):
        kind = data[position]
        position += 1
        if kind == 1:
            position = skip_blob(data, position)
            attributes = word(data, position)
            position += 4
            for _ in range(attributes * 2):
                position = skip_blob(data, position)
            namespaces = word(data, position)
            position += 4
            for _ in range(namespaces * 2):
                position = skip_blob(data, position)
        elif kind == 5:
            position = skip_blob(data, skip_blob(data, position))
        else:
            position = skip_blob(data, position)
    return position


def serialized(response):
    position = 1
    count = word(response, position)
    position = skip_events(response, position + 4, count)
    length = word(response, position)
    return response[position + 4:position + 4 + length].decode()


def main():
    run_dir = launch.new_run_dir('structured-data-anchor', 'headless')
    browser_inputs = [['json', text] for text in JSON_CASES]
    browser_inputs += [['xml', text] for text in XML_CASES]
    (run_dir / 'inputs.json').write_text(json.dumps(browser_inputs))
    archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
    session = launch.open_session(run_dir, archive)
    browser = None
    oracle = None
    candidate = None
    try:
        _, browser = session.start_browser('headless')
        result = json.loads(session.sidecar(['python3', '-B', '/structured/anchor-sidecar.py', '/inputs/inputs.json'],
                                            {root / 'oracle/harness/structured-data': '/structured:ro', run_dir: '/inputs:ro'},
                                            timeout=600))
        if len(result) != len(browser_inputs):
            raise RuntimeError('missing browser results')
        oracle = peer(str(root / '.work/tmp/structured-data/oracle'))
        candidate = peer(str(root / '.work/tmp/target/x86_64-unknown-linux-musl/debug/structured-data'))
        mismatches = []
        checked = 0
        for (kind, text), browser_value in zip(browser_inputs, result):
            if kind == 'json':
                response = ask(oracle, corpus.json_request(0, text))
                if response[0] != 1:
                    raise RuntimeError(f'oracle rejected an anchor JSON case: {text}')
                wanted = ask(oracle, corpus.write_request(0, response[1:]))
                if wanted[0] != 1:
                    raise RuntimeError('oracle rejected its own parsed value')
                expected = wanted[5:].decode()
                if ask(candidate, corpus.json_request(0, text)) != response:
                    mismatches.append(f'candidate JSON parse {text}')
                if ask(candidate, corpus.write_request(0, response[1:])) != wanted:
                    mismatches.append(f'candidate JSON write {text}')
            else:
                response = ask(oracle, corpus.xml_request(text))
                if response[0] != 1:
                    raise RuntimeError(f'oracle rejected an anchor XML case: {text}')
                expected = serialized(response)
                if expected.startswith('<?xml version="1.0"?>\n'):
                    expected = expected[len('<?xml version="1.0"?>\n'):]
                expected = expected.rstrip('\n')
                if ask(candidate, corpus.xml_request(text)) != response:
                    mismatches.append(f'candidate XML {text}')
            checked += 1
            if browser_value != expected:
                mismatches.append(f'browser/oracle {kind} {text}: {browser_value!r} vs {expected!r}')
        if mismatches:
            raise RuntimeError(f'{len(mismatches)} anchor mismatches: ' + ' | '.join(mismatches[:8]))
        print(f'anchor {checked} strict JSON and XML cases agree with the frozen browser and both peers')
        shutdown = session.probe('--close')
        if not shutdown.get('shutdown', {}).get('ok') or browser.wait(timeout=15) != 0:
            raise RuntimeError('browser shutdown failed')
    finally:
        for process in (oracle, candidate):
            if process is None:
                continue
            try:
                if process.stdin is not None and not process.stdin.closed:
                    process.stdin.close()
                if process.poll() is None:
                    process.kill()
                process.wait(timeout=15)
            except (OSError, subprocess.TimeoutExpired) as error:
                print(f'peer cleanup: {type(error).__name__}: {error}', file=sys.stderr)
        if browser is not None and browser.poll() is None:
            browser.kill()
            browser.wait()
        session.close()


if __name__ == '__main__':
    main()
