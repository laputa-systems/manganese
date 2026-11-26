import struct

LOAD_FLAGS = {'none': 0, 'bypass-cache': 1, 'validate-cache': 2}
ENDS = {'close': 0, 'reset': 1, 'hang': 2}
MAX_SEGMENT = 16384


def short(value):
    if isinstance(value, str):
        value = value.encode('latin-1')
    if len(value) > 0xffff:
        raise ValueError('field longer than 65535 bytes')
    return struct.pack('>H', len(value)) + value


def long(value):
    return struct.pack('>I', len(value)) + value


def encode(case):
    out = bytearray([1])
    out += short(case['url'])
    out += short(case.get('method', 'GET'))
    out.append(LOAD_FLAGS[case.get('load_flags', 'none')])
    body = case.get('body')
    if body is None:
        out.append(0)
    elif isinstance(body, bytes):
        out.append(1)
        out += long(body)
    else:
        out.append(2)
        out += struct.pack('>H', len(body))
        for chunk in body:
            out += long(chunk)
    headers = case.get('headers', [])
    out += struct.pack('>H', len(headers))
    for name, value in headers:
        out += short(name) + short(value)
    out += short(case.get('referrer', ''))
    out += short(case.get('user_agent', ''))
    out += short(case.get('accept_language', ''))
    out += short(case.get('cookie', ''))
    segments = case['segments']
    out += struct.pack('>H', len(segments))
    for segment in segments:
        if not segment or len(segment) > MAX_SEGMENT:
            raise ValueError('segment length outside 1 to 16384')
        out += long(segment)
    out.append(ENDS[case.get('end', 'close')])
    return bytes(out)
