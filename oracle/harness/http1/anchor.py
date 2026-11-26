import base64
import json
import pathlib
import struct
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(root / 'oracle/launch'))
import launch
from wire import encode

ORACLE = root / '.work/tmp/http1/oracle'
ACCEPT_LANGUAGE = 'en-US,en;q=0.9'
NET_HEADERS = {'host', 'connection', 'content-length', 'transfer-encoding', 'pragma', 'cache-control', 'referer', 'accept-encoding', 'accept-language', 'cookie'}
PAGE_HTML = b'<!doctype html><title>page</title>'
PAGE = b'HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\nContent-Length: %d\r\n\r\n%s' % (len(PAGE_HTML), PAGE_HTML)
CASES = [
    ('basic', 'navigate', 'c01.http1.test', 80, '/', b'HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nX-Dup: a\r\nx-dup: b\r\nX-Dup: c, d\r\nX-Fold: one\r\n  two\r\nContent-Length: 5\r\n\r\nhello', 'hang'),
    ('chunked', 'navigate', 'c02.http1.test', 80, '/chunked?x=1', b'HTTP/1.1 200 Chunked Reply\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n5;ext=1\r\nhello\r\n6   \r\n world\r\n0\r\nTrailer: x\r\n\r\n', 'close'),
    ('status-text', 'navigate', 'c03.http1.test', 80, '/', b'http/1.0 201   Spaced   Out   \r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nok', 'close'),
    ('http09', 'navigate', 'c04.http1.test', 80, '/', b'plain bytes without a status line', 'close'),
    ('informational', 'navigate', 'c05.http1.test', 80, '/', b'HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 3\r\n\r\nabc', 'hang'),
    ('length-mismatch', 'navigate', 'c06.http1.test', 80, '/', b'HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 10\r\n\r\nabcd', 'close'),
    ('bad-chunk', 'navigate', 'c07.http1.test', 80, '/', b'HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n', 'close'),
    ('multiple-length', 'navigate', 'c08.http1.test', 80, '/', b'HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab', 'close'),
    ('brotli-over-http', 'navigate', 'c09.http1.test', 80, '/', b'HTTP/1.1 200 OK\r\nContent-Encoding: br\r\nContent-Length: 0\r\n\r\n', 'close'),
    ('empty', 'navigate', 'c10.http1.test', 80, '/', b'', 'close'),
    ('localhost', 'navigate', 'localhost', 9871, '/local', b'HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nlocal', 'hang'),
    ('post', 'post', 'c12.http1.test', 80, '/submit', b'HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\ndone', 'hang'),
    ('misdirected', 'navigate', 'c13.http1.test', 80, '/', b'HTTP/1.1 421 Misdirected Request\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nno', 'close'),
]


def parse_request(raw):
    head, _, body = raw.partition(b'\r\n\r\n')
    lines = head.split(b'\r\n')
    method, target, _ = lines[0].split(b' ')
    headers = []
    for line in lines[1:]:
        name, _, value = line.partition(b': ')
        headers.append((name.decode('latin-1'), value.decode('latin-1')))
    return method.decode(), target.decode(), headers, body


def harness_input(case, raw):
    name, kind, host, port, path, response, end = case
    method, target, headers, body = parse_request(raw)
    origin = f'http://{host}' + ('' if port == 80 else f':{port}')
    request = {
        'url': origin + target,
        'method': method,
        'load_flags': 'bypass-cache',
        'headers': [(key, value) for key, value in headers if key.lower() not in NET_HEADERS],
        'referrer': next((value for key, value in headers if key.lower() == 'referer'), ''),
        'user_agent': next((value for key, value in headers if key.lower() == 'user-agent'), ''),
        'accept_language': ACCEPT_LANGUAGE,
        'segments': [response] if response else [],
        'end': end,
    }
    if any(key.lower() == 'content-length' for key, _ in headers):
        request['body'] = body
    return request


def harness(records):
    process = subprocess.Popen([str(ORACLE)], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    process.stdin.write(b'MGB1')
    process.stdin.flush()
    if process.stdout.read(4) != b'MGB1':
        raise RuntimeError('harness wrote an invalid protocol header')
    outputs = []
    for record in records:
        process.stdin.write(struct.pack('>I', len(record)) + record)
        process.stdin.flush()
        outputs.append(process.stdout.read(struct.unpack('>I', process.stdout.read(4))[0]))
    process.stdin.close()
    if process.wait():
        raise RuntimeError('harness failed')
    return outputs


def parse_output(output):
    result = {'sent': [], 'headers': {}}
    rest = output
    while rest.startswith(b'sent '):
        line, rest = rest.split(b'\n', 1)
        size = int(line.split()[1])
        result['sent'].append(rest[:size])
        rest = rest[size + 1:]
    while rest:
        line, rest = rest.split(b'\n', 1)
        key, _, value = line.partition(b' ')
        if key == b'body':
            size = int(value)
            result['body'] = rest[:size]
            rest = rest[size + 1:]
        elif key == b'header':
            name, _, header_value = value.decode('latin-1').partition(': ')
            headers = result['headers']
            headers[name] = headers[name] + '\n' + header_value if name in headers else header_value
        else:
            result[key.decode()] = value.decode('latin-1')
    return result


def status_text(line):
    parts = line.split(' ', 2)
    return parts[2] if len(parts) == 3 else ''


def expected_view(result):
    if result['start'] != 'OK':
        return {'error': 'net::' + result['start']}
    view = {'response': {'status': int(result['code']), 'statusText': status_text(result['line']), 'headers': result['headers']}}
    if result['end'] == 'OK':
        view['body'] = base64.b64encode(result['body']).decode()
    else:
        view['error'] = 'net::' + result['end']
    return view


def main():
    if not ORACLE.is_file():
        raise SystemExit(f'{ORACLE} is missing; run make http1-harness')
    sidecar_cases = [{'name': name, 'kind': kind, 'host': host, 'port': port, 'path': path, 'response': base64.b64encode(response).decode(),
                      'end': end, 'body': 'payload', 'page': base64.b64encode(PAGE).decode() if kind == 'post' else None}
                     for name, kind, host, port, path, response, end in CASES]
    run_dir = launch.new_run_dir('http1-anchor', 'headless')
    archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
    session = launch.open_session(run_dir, archive)
    browser = None
    try:
        _, browser = session.start_browser('headless')
        output = session.sidecar(['python3', '-B', '/http1/anchor-sidecar.py', json.dumps(sidecar_cases)], {root / 'oracle/harness/http1': '/http1:ro'}, timeout=600)
        (run_dir / 'observed.json').write_text(output)
        shutdown = session.probe('--close')
        if not shutdown.get('shutdown', {}).get('ok') or browser.wait(timeout=15) != 0:
            raise RuntimeError('browser shutdown failed')
    finally:
        if browser is not None and browser.poll() is None:
            browser.kill()
            browser.wait()
        session.close()
    result = json.loads(output)
    failures = []
    records, names = [], []
    for case in CASES:
        captured = [request for request in (base64.b64decode(item) for item in result['captured'][case[0]])
                    if parse_request(request)[1] == case[4]]
        if not captured:
            failures.append(f'{case[0]}: the frozen browser sent no request')
            continue
        records.append(encode(harness_input(case, captured[0])))
        names.append((case, captured))
    for (case, captured), output in zip(names, harness(records)):
        harnessed = parse_output(output)
        if harnessed['sent'] != captured:
            failures.append(f'{case[0]}: request bytes differ\n  browser {[item.decode("latin-1") for item in captured]!r}\n  harness {[item.decode("latin-1") for item in harnessed["sent"]]!r}')
        observed = result['observed'][case[0]]
        expected = expected_view(harnessed)
        if observed != expected:
            failures.append(f'{case[0]}: response view differs\n  browser {json.dumps(observed)}\n  harness {json.dumps(expected)}')
    for failure in failures:
        print(failure, file=sys.stderr)
    if failures:
        raise SystemExit(f'anchor {len(CASES) - len(failures)}/{len(CASES)} agree')
    print(f'anchor {len(CASES)}/{len(CASES)}: request bytes through the SOCKS5 egress and CDP response views agree with the source harness')


if __name__ == '__main__':
    main()
