"""Record and canonical-output wire format for the tls-handshake box.

Both peers speak MGB1. A request names the two browser inputs that change the
ClientHello within the frozen default policy: the SNI host and the ALPN list.
A response is a canonical ClientHello description that removes the values
BoringSSL draws from its RNG (client random, session id, GREASE codes, key
shares, ECH blob) and sorts extensions, because SSL_set_permute_extensions
randomizes their order on every connection.
"""

import struct

VERSION = 1
GREASE = 0xFACA
MAX_HOST = 253
MAX_PROTO = 32
MAX_ALPN = 8


def encode(host, alpn):
    host_bytes = host.encode('ascii') if isinstance(host, str) else host
    protos = [p.encode('ascii') if isinstance(p, str) else p for p in alpn]
    if not (1 <= len(host_bytes) <= MAX_HOST) or len(protos) > MAX_ALPN:
        raise ValueError('host or alpn count')
    if any(not (1 <= len(p) <= MAX_PROTO) for p in protos):
        raise ValueError('alpn protocol length')
    body = bytes([VERSION]) + struct.pack('>H', len(host_bytes)) + host_bytes + bytes([len(protos)])
    for proto in protos:
        body += bytes([len(proto)]) + proto
    return body


def decode(data):
    offset = 0
    version = data[offset]
    offset += 1
    if version != VERSION:
        raise ValueError('version')
    size = struct.unpack('>H', data[offset:offset + 2])[0]
    offset += 2
    host = data[offset:offset + size]
    offset += size
    if not (1 <= len(host) <= MAX_HOST):
        raise ValueError('host')
    count = data[offset]
    offset += 1
    if count > MAX_ALPN:
        raise ValueError('alpn count')
    protos = []
    for _ in range(count):
        size = data[offset]
        offset += 1
        proto = data[offset:offset + size]
        offset += size
        if not (1 <= len(proto) <= MAX_PROTO):
            raise ValueError('alpn length')
        protos.append(proto)
    if offset != len(data):
        raise ValueError('trailing bytes')
    return host, protos
