import random

from wire import MAX_SEGMENT, encode

USER_AGENT = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/156.0.0.0 Safari/537.36'
ACCEPT_LANGUAGE = 'en-US,en;q=0.9'
NAVIGATION_ACCEPT = 'text/html,application/xhtml+xml,application/xml;q=0.9,image/jxl,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7'

URLS = [
    'http://http1.test/', 'http://http1.test/index.html', 'http://http1.test/a/b?c=d&e=f', 'http://http1.test:8080/',
    'http://http1.test:81/x', 'http://localhost/', 'http://localhost:8000/api', 'http://127.0.0.1/', 'http://127.0.0.1:8080/x?y',
    'http://127.1.2.3/', 'http://10.0.0.1/', 'http://[::1]/', 'http://[::1]:8080/p', 'http://[2001:db8::1]/', 'http://sub.localhost/',
    'http://a.b.localhost:3000/', 'http://localhost./', 'http://user:pass@http1.test/secret', 'http://http1.test/%20a?q=%22x%22',
    'http://http1.test/page#fragment', 'http://xn--bcher-kva.test/', 'http://localhost.test/', 'http://http1.test/?',
]
REFERRERS = ['http://http1.test/', 'http://ref.test/path?q', 'https://secure.test/', 'http://localhost:8000/from']
METHODS = ['GET', 'GET', 'GET', 'POST', 'PUT', 'HEAD', 'DELETE', 'OPTIONS', 'PATCH', 'PROPFIND', 'get', 'Post']
CALLER_HEADER_SETS = [
    [],
    [('Upgrade-Insecure-Requests', '1'), ('User-Agent', USER_AGENT), ('Accept', NAVIGATION_ACCEPT)],
    [('User-Agent', USER_AGENT), ('Accept', '*/*')],
    [('sec-ch-ua', '"Chromium";v="156", "Not-A.Brand";v="24"'), ('sec-ch-ua-mobile', '?0'), ('sec-ch-ua-platform', '"Linux"'),
     ('User-Agent', USER_AGENT), ('Accept', '*/*'), ('Sec-Fetch-Site', 'same-origin'), ('Sec-Fetch-Mode', 'cors'), ('Sec-Fetch-Dest', 'empty')],
    [('Origin', 'http://http1.test'), ('Content-Type', 'application/json')],
    [('Accept', 'image/avif,image/webp,*/*'), ('X-Custom', 'value')],
]
ODD_HEADERS = [
    ('x-lower', 'value'), ('X-UPPER', 'VALUE'), ('X-Empty', ''), ('X-Spaces', '  padded  '), ('X-Control', 'a\x01b\x7fc'),
    ('X-High', 'caf\xe9 \xff'), ('X-Tab', '\tt\t'), ('X-Quote', '"quoted, value"'), ('x-dup', 'first'), ('X-Dup', 'second'),
    ('Host', 'override.test'), ('connection', 'close'), ('Content-Length', '99'), ('Transfer-Encoding', 'chunked'),
    ('Accept-Encoding', 'identity'), ('accept-encoding', 'br'), ('Accept-Encoding', '"gzip"'), ('Accept-Encoding', 'gzip;q=0, deflate'),
    ('Accept-Language', 'fr-FR'), ('user-agent', 'custom-agent/1.0'), ('Cookie', 'caller=1'), ('Referer', 'http://dropped.test/'),
    ('Range', 'bytes=0-10'), ('Pragma', 'no-cache'), ('Cache-Control', 'max-age=10'), ('Expect', '100-continue'),
    ('Proxy-Connection', 'keep-alive'), ('!#$%&\'*+-.^_`|~', 'token'), ('X-' + 'n' * 200, 'long-name'), ('X-Long', 'v' * 3000),
]

STATUS_LINES = [
    b'HTTP/1.1 200 OK', b'HTTP/1.0 200 OK', b'HTTP/1.1 200', b'HTTP/1.1 200 ', b'HTTP/1.1 200  Spaced  Out  ', b'http/1.1 200 ok',
    b'Http/1.1 201 Created', b'HTTP/2.0 200 OK', b'HTTP/2 200 OK', b'HTTP/9.9 200 Future', b'HTTP/1.2 200 OK', b'HTTP/0.9 200 OK',
    b'HTTP/1.1 404 Not Found', b'HTTP/1.1 500 Internal Server Error', b'HTTP/1.1 999999999999 Huge', b'HTTP/1.1 0 Zero',
    b'HTTP/1.1 00200 Padded', b'HTTP/1.1 abc', b'HTTP/1.1', b'HTTP/1.1  301 Moved', b'HTTP/1.1\t200 Tab', b'HTTP 200 OK',
    b'HTTP/1.x 200 OK', b'HTTP/.1 200 OK', b'HTTP/1. 200 OK', b'HTTP/11.1 200 OK', b'HTTP/1.1 2x0 Mixed', b'HTTP/1.1 200 OK\t',
    b'HTTP/1.1 200 caf\xe9', b'xHTTP/1.1 200 Slop', b'\r\nHTTP/1.1 200 Leading', b'abcdHTTP/1.1 200 FourSlop', b'abcdeHTTP/1.1 200 FiveSlop',
]
STATUS_CODES = [200, 201, 204, 205, 206, 301, 302, 303, 304, 307, 308, 400, 401, 403, 404, 407, 408, 421, 425, 500, 503]
CONTENT_LENGTHS = ['{n}', '{n}', '{n}', ' {n}', '{n} ', '+{n}', '-0', '0x5', '', 'abc', '99999999999999999999', '{n}, {n}', '{n}, {m}', '-1', '{n}\x0b']
TRANSFER_ENCODINGS = ['chunked', 'Chunked', 'gzip, chunked', 'chunked, gzip', 'identity', ' chunked ', 'chunked;q=1', 'x-chunked']
CONTENT_ENCODINGS = ['gzip', 'deflate', 'br', 'zstd', 'x-gzip', 'identity', 'GZIP', 'gzip, deflate', 'gzip, ,deflate', 'foo', 'gzip;q=1',
                     '"gzip"', 'gzip br', '', 'compress', 'gzip, zstd']
RESPONSE_HEADERS = [
    (b'Content-Type', b'text/html; charset=utf-8'), (b'Server', b'http1-box'), (b'Date', b'Wed, 26 Nov 2026 00:00:00 GMT'),
    (b'Set-Cookie', b'a=b; Path=/'), (b'Set-Cookie', b'c=d, e=f'), (b'Cache-Control', b'no-cache, no-store'), (b'Vary', b'Accept-Encoding'),
    (b'X-Empty', b''), (b'X-Spaces', b'   inner   value   '), (b'X-Commas', b'a, b,, c ,'), (b'X-Quoted', b'"a,b", c'),
    (b'WWW-Authenticate', b'Basic realm="x", Digest realm="y"'), (b'Location', b'/elsewhere'), (b'Content-Disposition', b'attachment; filename="a,b"'),
    (b'Connection', b'keep-alive'), (b'Connection', b'close'), (b'Connection', b'Keep-Alive, close'), (b'Proxy-Connection', b'close'),
    (b'Keep-Alive', b'timeout=5'), (b'X-High', b'\xe9\xff'), (b'X-Control', b'\x01\x7f'), (b'UPPER-CASE', b'Value'), (b'x-lower', b'v'),
]
MALFORMED_LINES = [
    b'NoColonHere', b'Bad Name: value', b'Name : spaced-before-colon', b': empty-name', b' leading: space', b'\tleading: tab',
    b'Na\x00me: nul', b'X-A:no-space', b'X-B:   ', b'X-C: v\x00alue', b'(paren): bad-token', b'X-D: value\r', b'X-Unicode-\xe9: v',
]


def segment(data, rng, style=None):
    if not data:
        return []
    style = style or rng.choice(['whole', 'whole', 'random', 'random', 'tiny', 'head-bytes', 'head-split'])
    if style == 'whole':
        cuts = list(range(MAX_SEGMENT, len(data), MAX_SEGMENT))
    elif style == 'tiny':
        cuts = list(range(1, min(len(data), 64))) + list(range(64, len(data), MAX_SEGMENT))
    elif style == 'head-bytes':
        head = data.find(b'\r\n\r\n')
        head = len(data) if head < 0 else head + 4
        cuts = list(range(1, min(head, 300))) + list(range(min(head, 300), len(data), 4096))
    elif style == 'head-split':
        end = data.find(b'\r\n\r\n')
        points = {3, 7, 8, 9}
        if end >= 0:
            points |= {end + 1, end + 2, end + 3, end + 4}
        cuts = sorted(point for point in points if 0 < point < len(data)) + list(range(MAX_SEGMENT, len(data), MAX_SEGMENT))
    else:
        cuts = sorted(set(rng.randrange(1, len(data)) for _ in range(rng.randrange(1, 8)) if len(data) > 1)) + list(range(MAX_SEGMENT, len(data), MAX_SEGMENT))
    cuts = sorted(set(cut for cut in cuts if 0 < cut < len(data)))
    pieces, start = [], 0
    for cut in cuts + [len(data)]:
        while cut - start > MAX_SEGMENT:
            pieces.append(data[start:start + MAX_SEGMENT])
            start += MAX_SEGMENT
        if cut > start:
            pieces.append(data[start:cut])
            start = cut
    return pieces


def chunked(body, rng, style):
    out = bytearray()
    position = 0
    while position < len(body):
        size = min(len(body) - position, rng.choice([1, 3, 7, 16, 100, 1000, 4096]))
        size_text = format(size, rng.choice(['x', 'X']))
        if style == 'extension':
            size_text += ';name=value'
        elif style == 'padded':
            size_text += '   '
        elif style == 'leading-zeros':
            size_text = '000' + size_text
        out += size_text.encode() + b'\r\n' + body[position:position + size] + b'\r\n'
        position += size
    if style == 'bad-size':
        out += b'zz\r\n'
    elif style == 'missing-crlf':
        out = out[:-2] + b'X\r\n'
    elif style == 'lf-only':
        out = out.replace(b'\r\n', b'\n')
    elif style == 'negative':
        out += b'-1\r\n'
    elif style == 'huge':
        out += b'FFFFFFFFFFFFFFFFFF\r\n'
    elif style == 'overflow':
        out += b'7FFFFFFFFFFFFFFF\r\n'
    elif style == 'empty-size':
        out += b'\r\n'
    elif style == 'long-line':
        out += b'5;' + b'e' * 17000 + b'\r\nhello\r\n'
    if style not in ('incomplete',):
        out += b'0\r\n'
        if style == 'trailers':
            out += b'Trailer-A: x\r\nTrailer-B: y\r\n'
        out += b'\r\n'
        if style == 'after':
            out += b'HTTP/1.1 200 OK\r\n\r\n'
    return bytes(out)


def response_head(status, headers, line_ending=b'\r\n'):
    return status + line_ending + b''.join(name + b': ' + value + line_ending for name, value in headers) + line_ending


def body_bytes(rng):
    size = rng.choice([0, 1, 5, 17, 100, 1000, 4095, 4096, 5000, 20000, 40000])
    pattern = bytes(rng.randrange(256) for _ in range(64))
    return (pattern * (size // 64 + 1))[:size]


def random_response(rng):
    code = rng.choice(STATUS_CODES)
    version = rng.choice([b'HTTP/1.1', b'HTTP/1.1', b'HTTP/1.0'])
    status = version + b' ' + str(code).encode() + rng.choice([b' Reason', b'', b' OK'])
    if rng.randrange(8) == 0:
        status = rng.choice(STATUS_LINES)
    headers = [rng.choice(RESPONSE_HEADERS) for _ in range(rng.randrange(0, 5))]
    body = body_bytes(rng)
    framing = rng.choice(['length', 'length', 'chunked', 'chunked', 'close', 'odd-length', 'odd-te', 'both'])
    payload = body
    if framing == 'length':
        length = len(body) + rng.choice([0, 0, 0, -1, 5, -len(body)])
        headers.append((b'Content-Length', str(max(length, 0)).encode()))
    elif framing == 'odd-length':
        value = rng.choice(CONTENT_LENGTHS).format(n=len(body), m=len(body) + 1)
        headers.append((rng.choice([b'Content-Length', b'content-length', b'CONTENT-LENGTH']), value.encode('latin-1')))
        if rng.randrange(3) == 0:
            headers.append((b'Content-Length', str(len(body)).encode()))
    elif framing in ('chunked', 'odd-te', 'both'):
        te = 'chunked' if framing != 'odd-te' else rng.choice(TRANSFER_ENCODINGS)
        headers.append((b'Transfer-Encoding', te.encode()))
        if framing == 'both':
            headers.append((b'Content-Length', str(len(body) + rng.choice([0, 3])).encode()))
        payload = chunked(body, rng, rng.choice(['plain', 'plain', 'plain', 'extension', 'padded', 'leading-zeros', 'bad-size', 'missing-crlf',
                                                  'lf-only', 'negative', 'huge', 'overflow', 'empty-size', 'trailers', 'after', 'incomplete', 'long-line']))
    if rng.randrange(5) == 0:
        headers.append((b'Content-Encoding', rng.choice(CONTENT_ENCODINGS).encode()))
    if rng.randrange(6) == 0:
        headers.insert(rng.randrange(len(headers) + 1), (b'X-Fold', b'first'))
        headers.insert(rng.randrange(len(headers) + 1), (b' continued', b'line'))
    if rng.randrange(10) == 0:
        headers.append(rng.choice([(b'Location', b'/a'), (b'Location', b'/b'), (b'Content-Disposition', b'inline'), (b'Location', b'')]))
    line_ending = rng.choice([b'\r\n'] * 6 + [b'\n', b'\r'])
    head = response_head(status, headers, line_ending)
    if rng.randrange(10) == 0:
        head = head[:-len(line_ending)] + rng.choice(MALFORMED_LINES) + line_ending + line_ending
    informational = b''
    if rng.randrange(6) == 0:
        informational = rng.choice([b'HTTP/1.1 100 Continue\r\n\r\n', b'HTTP/1.1 103 Early Hints\r\nLink: </s.css>; rel=preload\r\n\r\n',
                                    b'HTTP/1.1 102 Processing\r\n\r\n', b'HTTP/1.1 101 Switching\r\nUpgrade: x\r\n\r\n',
                                    b'HTTP/1.1 100 Continue\r\nContent-Length: 5\r\n\r\n', b'HTTP/1.1 199 Odd\r\nTransfer-Encoding: chunked\r\n\r\n'])
    trailing = rng.choice([b''] * 6 + [b'extra', b'HTTP/1.1 200 OK\r\n\r\n'])
    return informational + head + payload + trailing


def random_request(rng):
    case = {'url': rng.choice(URLS), 'method': rng.choice(METHODS), 'load_flags': rng.choice(['none'] * 6 + ['bypass-cache', 'validate-cache'])}
    body_kind = rng.choice(['none'] * 5 + ['bytes', 'bytes', 'chunked'])
    if body_kind == 'bytes':
        case['body'] = bytes(rng.randrange(32, 127) for _ in range(rng.choice([0, 1, 10, 100, 1200, 1300, 1400, 5000, 20000])))
    elif body_kind == 'chunked':
        case['body'] = [bytes(rng.randrange(32, 127) for _ in range(size)) for size in rng.sample([1, 5, 100, 16372, 16373, 20000], rng.randrange(0, 3))]
    headers = list(rng.choice(CALLER_HEADER_SETS))
    for _ in range(rng.choice([0, 0, 0, 1, 2, 4])):
        headers.insert(rng.randrange(len(headers) + 1), rng.choice(ODD_HEADERS))
    case['headers'] = headers
    if rng.randrange(3) == 0:
        case['referrer'] = rng.choice(REFERRERS)
    case['user_agent'] = rng.choice([USER_AGENT, USER_AGENT, '', 'agent/1.0'])
    case['accept_language'] = rng.choice([ACCEPT_LANGUAGE, ACCEPT_LANGUAGE, '', 'de'])
    if rng.randrange(4) == 0:
        case['cookie'] = rng.choice(['a=b', 'session=abc; theme=dark'])
    return case


def random_case(rng):
    case = random_request(rng)
    response = random_response(rng)
    if rng.randrange(25) == 0:
        response = rng.choice([b'', b'hello', b'hello world, this is HTTP/0.9', b'ICY 200 OK\r\n\r\nstream', b'HTT', b'\x00\x01\x02\x03\x04\x05\x06\x07\x08'])
    if rng.randrange(40) == 0:
        response = b'HTTP/1.1 200 OK\r\n' + b''.join(b'X-Big-%d: ' % i + b'b' * 8000 + b'\r\n' for i in range(rng.choice([30, 33, 40]))) + b'\r\n'
    case['segments'] = segment(response, rng)
    case['end'] = rng.choice(['close', 'close', 'close', 'hang', 'hang', 'reset'])
    return case


def conformance_cases():
    plain = {'user_agent': USER_AGENT, 'accept_language': ACCEPT_LANGUAGE}
    named = []

    def add(name, segments, end='close', **request):
        case = {**plain, 'url': 'http://http1.test/', **request}
        case['segments'] = [piece[start:start + MAX_SEGMENT] for piece in segments for start in range(0, len(piece), MAX_SEGMENT)]
        case['end'] = end
        named.append((name, case))

    ok = b'HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello'
    for index, url in enumerate(URLS):
        add(f'url-{index:02d}', [ok], 'hang', url=url)
    for index, method in enumerate(['GET', 'POST', 'PUT', 'HEAD', 'DELETE', 'OPTIONS', 'get', 'Post']):
        add(f'method-{index:02d}', [ok], 'hang', method=method)
    for index, flags in enumerate(['bypass-cache', 'validate-cache']):
        add(f'load-{index:02d}', [ok], 'hang', load_flags=flags)
    for index, body in enumerate([b'', b'x', b'a' * 1300, b'a' * 1400, b'a' * 20000]):
        add(f'upload-bytes-{index:02d}', [ok], 'hang', method='POST', body=body)
    for index, pieces in enumerate([[], [b'x'], [b'a' * 16372], [b'a' * 16373], [b'ab', b'cd', b'ef'], [b'a' * 20000, b'b' * 20000]]):
        add(f'upload-chunked-{index:02d}', [ok], 'hang', method='PUT', body=pieces)
    for index, headers in enumerate(CALLER_HEADER_SETS):
        add(f'caller-set-{index:02d}', [ok], 'hang', headers=headers)
    for index, header in enumerate(ODD_HEADERS):
        add(f'caller-odd-{index:02d}', [ok], 'hang', headers=[('Accept', '*/*'), header])
    add('referrer', [ok], 'hang', referrer='http://ref.test/path?q')
    add('cookie', [ok], 'hang', cookie='a=b; c=d')
    add('no-agent', [ok], 'hang', user_agent='', accept_language='')
    for index, status in enumerate(STATUS_LINES):
        add(f'status-{index:02d}', [status + b'\r\nContent-Length: 2\r\n\r\nok'], 'close')
    for index, code in enumerate(STATUS_CODES):
        add(f'code-{index:02d}', [b'HTTP/1.1 %d Code\r\nContent-Length: 3\r\n\r\nabc' % code], 'hang')
    for index, value in enumerate(CONTENT_LENGTHS):
        head = b'HTTP/1.1 200 OK\r\nContent-Length: ' + value.format(n=5, m=6).encode('latin-1') + b'\r\n\r\nhello'
        add(f'content-length-{index:02d}', [head], 'close')
    for index, value in enumerate(TRANSFER_ENCODINGS):
        add(f'transfer-encoding-{index:02d}', [b'HTTP/1.1 200 OK\r\nTransfer-Encoding: ' + value.encode() + b'\r\n\r\n5\r\nhello\r\n0\r\n\r\n'], 'close')
    add('transfer-encoding-http10', [b'HTTP/1.0 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n'], 'close')
    for index, value in enumerate(CONTENT_ENCODINGS):
        add(f'content-encoding-{index:02d}', [b'HTTP/1.1 200 OK\r\nContent-Encoding: ' + value.encode() + b'\r\nContent-Length: 0\r\n\r\n'], 'hang')
        add(f'content-encoding-localhost-{index:02d}', [b'HTTP/1.1 200 OK\r\nContent-Encoding: ' + value.encode() + b'\r\nContent-Length: 0\r\n\r\n'], 'hang', url='http://localhost/')
    add('content-encoding-redirect', [b'HTTP/1.1 302 Found\r\nLocation: /x\r\nContent-Encoding: br\r\nContent-Length: 0\r\n\r\n'], 'hang')
    for index, (name, value) in enumerate(RESPONSE_HEADERS):
        add(f'response-header-{index:02d}', [b'HTTP/1.1 200 OK\r\n' + name + b': ' + value + b'\r\nContent-Length: 0\r\n\r\n'], 'hang')
    for index, line in enumerate(MALFORMED_LINES):
        add(f'malformed-line-{index:02d}', [b'HTTP/1.1 200 OK\r\n' + line + b'\r\nContent-Length: 2\r\n\r\nok'], 'hang')
    for index, style in enumerate(['plain', 'extension', 'padded', 'leading-zeros', 'bad-size', 'missing-crlf', 'lf-only', 'negative', 'huge',
                                    'overflow', 'empty-size', 'trailers', 'after', 'incomplete', 'long-line']):
        body = chunked(b'0123456789abcdef' * 20, random.Random(index), style)
        add(f'chunked-{index:02d}', [b'HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n' + body], 'close')
        add(f'chunked-hang-{index:02d}', segment(b'HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n' + body, random.Random(index), 'random'), 'hang')
    for index, end in enumerate(['close', 'reset', 'hang']):
        add(f'end-length-exact-{index}', [ok], end)
        add(f'end-length-short-{index}', [b'HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nhello'], end)
        add(f'end-length-extra-{index}', [ok + b'EXTRA'], end)
        add(f'end-length-extra-split-{index}', [ok, b'EXTRA'], end)
        add(f'end-close-delimited-{index}', [b'HTTP/1.1 200 OK\r\n\r\nbody until close'], end)
        add(f'end-headers-only-{index}', [b'HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n'], end)
        add(f'end-truncated-head-{index}', [b'HTTP/1.1 200 OK\r\nX-A: b'], end)
        add(f'end-truncated-status-{index}', [b'HTTP/1.1 20'], end)
        add(f'end-nothing-{index}', [], end)
        add(f'end-short-09-{index}', [b'hi'], end)
        add(f'end-09-{index}', [b'hello there'], end)
        add(f'end-informational-truncated-{index}', [b'HTTP/1.1 100 Continue\r\n'], end)
        add(f'end-informational-only-{index}', [b'HTTP/1.1 100 Continue\r\n\r\n'], end)
        add(f'end-keepalive-10-{index}', [b'HTTP/1.0 200 OK\r\nConnection: keep-alive\r\nContent-Length: 5\r\n\r\nhello'], end)
        add(f'end-close-11-{index}', [b'HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 5\r\n\r\nhello'], end)
        add(f'end-head-{index}', [b'HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\n'], end, method='HEAD')
        add(f'end-204-extra-{index}', [b'HTTP/1.1 204 No Content\r\n\r\nextra'], end)
    add('informational-chain', [b'HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 103 Hints\r\nLink: <a>\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok'], 'hang')
    add('informational-split', [b'HTTP/1.1 100 Continue\r\n\r\n', b'HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok'], 'hang')
    add('informational-09', [b'HTTP/1.1 100 Continue\r\n\r\nplain body'], 'close')
    add('informational-content-encoding', [b'HTTP/1.1 100 Continue\r\nContent-Encoding: br\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n'], 'hang')
    add('misdirected', [b'HTTP/1.1 421 Misdirected Request\r\nContent-Length: 4\r\n\r\nnope'], 'hang')
    add('misdirected-post', [b'HTTP/1.1 421 Misdirected Request\r\nContent-Length: 0\r\n\r\n'], 'close', method='POST', body=b'payload')
    add('proxy-auth', [b'HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm="x"\r\nContent-Length: 0\r\n\r\n'], 'hang')
    add('server-auth', [b'HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm="x"\r\nContent-Length: 4\r\n\r\ndeny'], 'hang')
    add('put-09', [b'just some bytes'], 'close', method='PUT', body=b'x')
    add('icy-port', [b'ICY 200 OK\r\nicy-name: radio\r\n\r\naudio'], 'close', url='http://http1.test:8000/')
    add('icy-lower-port', [b'icy 200 OK\r\n\r\naudio'], 'close', url='http://http1.test:8000/')
    add('nonicy-port', [b'hello world'], 'close', url='http://http1.test:8000/')
    add('big-head-under', [b'HTTP/1.1 200 OK\r\n'] + [b'X-B: ' + b'b' * 16000 + b'\r\n'] * 15 + [b'Content-Length: 0\r\n\r\n'], 'hang')
    add('big-head-over', [b'HTTP/1.1 200 OK\r\n'] + [b'X-B: ' + b'b' * 16000 + b'\r\n'] * 17, 'hang')
    add('nul-in-head', [b'HTTP/1.1 200 OK\r\nX-A: a\x00b\r\nContent-Length: 0\r\n\r\n'], 'hang')
    add('lf-only', [b'HTTP/1.1 200 OK\nContent-Length: 2\n\nok'], 'hang')
    add('cr-only', [b'HTTP/1.1 200 OK\rContent-Length: 2\r\r\n\nok'], 'hang')
    add('obs-fold', [b'HTTP/1.1 200 OK\r\nX-Fold: a\r\n  b\r\n\tc\r\nContent-Length: 0\r\n\r\n'], 'hang')
    add('multiple-location', [b'HTTP/1.1 302 Found\r\nLocation: /a\r\nLocation: /b\r\nContent-Length: 0\r\n\r\n'], 'hang')
    add('same-location', [b'HTTP/1.1 302 Found\r\nLocation: /a\r\nLocation: /a\r\nContent-Length: 0\r\n\r\n'], 'hang')
    add('multiple-disposition', [b'HTTP/1.1 200 OK\r\nContent-Disposition: a\r\nContent-Disposition: b\r\nContent-Length: 0\r\n\r\n'], 'hang')
    add('chunked-multiple-length', [b'HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n0\r\n\r\n'], 'hang')
    for index in range(4):
        rng = random.Random(1000 + index)
        data = b'HTTP/1.1 200 OK\r\nContent-Length: 30000\r\n\r\n' + bytes(rng.randrange(256) for _ in range(30000))
        add(f'large-body-{index}', segment(data, rng, ['whole', 'random', 'tiny', 'head-split'][index]), 'hang')
    return named


def generate(lane):
    seed = {'seen': 4401, 'holdout': 4402}[lane]
    rng = random.Random(seed)
    items = []
    if lane == 'seen':
        items += [(f'seen-{name}', case) for name, case in conformance_cases()]
        items += [(f'seen-random-{index:03d}', random_case(rng)) for index in range(300)]
    else:
        items += [(f'holdout-random-{index:03d}', random_case(rng)) for index in range(320)]
    return [(name, encode(case)) for name, case in items]
