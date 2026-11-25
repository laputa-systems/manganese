import struct


def field(value):
    if value is None:
        return struct.pack('>I', 0xffffffff)
    data = value.encode('utf-16', 'surrogatepass').decode('utf-16', 'replace').encode('utf-8')
    return struct.pack('>I', len(data)) + data


def request(value, base, other, private, setter=None):
    result = field(value) + field(base) + field(other) + bytes([private])
    if setter is not None:
        result += field(setter[0]) + field(setter[1])
    return result
