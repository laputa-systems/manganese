import struct

MAX_REQUESTS = 8
PRIORITIES = range(0, 6)


def short(value):
    if isinstance(value, str):
        value = value.encode('latin-1')
    if len(value) > 0xffff:
        raise ValueError('field longer than 65535 bytes')
    return struct.pack('>H', len(value)) + value


def long(value):
    if isinstance(value, str):
        value = value.encode('latin-1')
    return struct.pack('>I', len(value)) + value


def frame(kind, flags, stream, payload):
    return len(payload).to_bytes(3, 'big') + bytes([kind, flags]) + struct.pack('>I', stream) + payload


def hpack_string(value):
    value = value.encode() if isinstance(value, str) else value
    if len(value) < 127:
        return bytes([len(value)]) + value
    return bytes([127]) + value + b'\x00'


STATUS_INDEX = {200: 8, 204: 9, 206: 10, 304: 11, 400: 12, 404: 13, 500: 14}


def decode(record):
    cursor = 2
    count = record[1]
    requests = []
    for _ in range(count):
        fields = {}
        for name in ('method', 'scheme', 'host', 'path'):
            size = int.from_bytes(record[cursor:cursor + 2], 'big')
            cursor += 2
            fields[name] = record[cursor:cursor + size].decode('latin-1')
            cursor += size
        header_count = int.from_bytes(record[cursor:cursor + 2], 'big')
        cursor += 2
        headers = []
        for _ in range(header_count):
            size = int.from_bytes(record[cursor:cursor + 2], 'big')
            cursor += 2
            name = record[cursor:cursor + size].decode('latin-1')
            cursor += size
            size = int.from_bytes(record[cursor:cursor + 2], 'big')
            cursor += 2
            value = record[cursor:cursor + size].decode('latin-1')
            cursor += size
            headers.append((name, value))
        fields['headers'] = headers
        fields['priority'] = record[cursor]
        cursor += 1
        fields['status'] = int.from_bytes(record[cursor:cursor + 2], 'big')
        cursor += 2
        fields['reset'] = record[cursor]
        cursor += 1
        if fields['reset']:
            fields['error'] = int.from_bytes(record[cursor:cursor + 4], 'big')
            cursor += 4
        fields['trailers'] = record[cursor]
        cursor += 1
        requests.append(fields)
    settings_count = int.from_bytes(record[cursor:cursor + 2], 'big')
    cursor += 2
    settings = []
    for _ in range(settings_count):
        settings.append((int.from_bytes(record[cursor:cursor + 2], 'big'),
                         int.from_bytes(record[cursor + 2:cursor + 6], 'big')))
        cursor += 6
    window = int.from_bytes(record[cursor:cursor + 4], 'big')
    cursor += 4
    end = 'reset' if record[cursor] else 'close'
    return {'requests': requests, 'settings': settings, 'window_update': window, 'end': end}


STATUS_INDEX = {200: 8, 204: 9, 206: 10, 304: 11, 400: 12, 404: 13, 500: 14}


def header_block(status):
    if status in STATUS_INDEX:
        return bytes([0x80 | STATUS_INDEX[status]])
    return b'\x00' + hpack_string(b':status') + hpack_string(str(status))


def server_frames(requests, settings, delta):
    payload = b''.join(struct.pack('>HI', identifier, value) for identifier, value in settings)
    frames = [frame(4, 0, 0, payload)]
    if delta:
        frames.append(frame(8, 0, 0, struct.pack('>I', delta)))
    for index, item in enumerate(requests):
        stream = 2 * index + 1
        flags = 5
        if item.get('trailers'):
            flags = 4
        frames.append(frame(1, flags, stream, header_block(item.get('status', 200))))
        if item.get('trailers'):
            frames.append(frame(1, 5, stream, b''))
        if item.get('reset', 0):
            frames.append(frame(3, 0, stream, struct.pack('>I', item.get('error', 8))))
    return b''.join(frames)


def encode(case):
    requests = case['requests']
    if not 1 <= len(requests) <= MAX_REQUESTS:
        raise ValueError('request count outside 1 to 8')
    out = bytearray([1, len(requests)])
    for item in requests:
        out += short(item.get('method', 'GET'))
        out += short(item.get('scheme', 'https'))
        out += short(item.get('host', 'verified.https.test'))
        out += short(item.get('path', '/'))
        headers = item.get('headers', [])
        if len(headers) > 0xffff:
            raise ValueError('too many headers')
        out += struct.pack('>H', len(headers))
        for name, value in headers:
            out += short(name) + short(value)
        priority = item.get('priority', 2)
        if priority not in PRIORITIES:
            raise ValueError('priority outside 0 to 5')
        out.append(priority)
        status = item.get('status', 200)
        if not 100 <= status <= 599:
            raise ValueError('status outside 100 to 599')
        out += struct.pack('>H', status)
        reset = item.get('reset', 0)
        if reset not in (0, 1):
            raise ValueError('reset outside 0 to 1')
        out.append(reset)
        if reset:
            out += struct.pack('>I', item.get('error', 8))
        out.append(1 if item.get('trailers') else 0)
    settings = case.get('settings', [])
    if len(settings) > 0xffff:
        raise ValueError('too many settings')
    out += struct.pack('>H', len(settings))
    for identifier, value in settings:
        out += struct.pack('>HI', identifier, value)
    out += struct.pack('>I', case.get('window_update', 65535))
    end = case.get('end', 'close')
    out.append({'close': 0, 'reset': 1}[end])
    out += long(server_frames(requests, settings, case.get('window_update', 65535)))
    return bytes(out)
