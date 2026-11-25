import struct


def field(value):
    if value is None:
        return struct.pack('>I', 0xffffffff)
    data = value.encode('utf-16', 'surrogatepass').decode('utf-16', 'replace').encode('utf-8')
    return struct.pack('>I', len(data)) + data


def request(value, base, other, private):
    return field(value) + field(base) + field(other) + bytes([private])
