"""Canonicalize a TLS ClientHello into the tls-handshake box's v1 output form.

The transformation removes values BoringSSL draws from its RNG and removes the
per-connection extension order that SSL_set_permute_extensions randomizes. It
keeps every field a server or a network observer can otherwise see: the cipher
list in order, the compression methods, and each extension's type and body.
This module is shared by the anchor, which canonicalizes the ClientHello the
frozen browser sends through the trusted HTTPS fixture.
"""

import struct

GREASE = 0xFACA


def grease(value):
    return (value >> 8) == (value & 0xFF) and (value & 0x0F) == 0x0A


def normalized_code(value):
    return GREASE if grease(value) else value


def u16(data, offset):
    return int.from_bytes(data[offset:offset + 2], 'big'), offset + 2


def handshake_message(data):
    offset, handshake = 0, b''
    while offset + 5 <= len(data):
        if data[offset] != 22:
            raise ValueError('not a handshake record')
        size = int.from_bytes(data[offset + 3:offset + 5], 'big')
        offset += 5
        if offset + size > len(data):
            raise ValueError('short record')
        handshake += data[offset:offset + size]
        offset += size
    if len(handshake) < 4 or handshake[0] != 1:
        raise ValueError('not a ClientHello')
    size = int.from_bytes(handshake[1:4], 'big')
    if size + 4 != len(handshake):
        raise ValueError('bad ClientHello length')
    return handshake[4:]


def normalize_body(kind, body):
    if grease(kind):
        return bytes(len(body))
    if kind == 0xFE0D:
        return b'\x00'
    if kind == 0x0033:
        if len(body) < 2:
            return body
        out, offset = body[:2], 2
        while offset + 4 <= len(body):
            group, size = u16(body, offset)
            offset += 4
            if offset + size > len(body):
                break
            out += struct.pack('>H', normalized_code(group)) + struct.pack('>H', size) + bytes(size)
            offset += size
        return out
    if kind in (0x000D, 0x000A, 0x002B):
        start = 1 if kind == 0x002B else 2
        if len(body) >= start + 2:
            value, _ = u16(body, start)
            value = normalized_code(value)
            return body[:start] + struct.pack('>H', value) + body[start + 2:]
    return body


def canonicalize(data):
    body = handshake_message(data)
    offset = 34
    session = body[offset]
    offset += 1 + session
    size, offset = u16(body, offset)
    ciphers = [normalized_code(int.from_bytes(body[offset + i:offset + i + 2], 'big')) for i in range(0, size, 2)]
    offset += size
    compression = body[offset]
    offset += 1
    methods = body[offset:offset + compression]
    offset += compression
    size, offset = u16(body, offset)
    extensions = []
    end = offset + size
    while offset + 4 <= end:
        kind, offset = u16(body, offset)
        length, offset = u16(body, offset)
        value = body[offset:offset + length]
        offset += length
        kind = normalized_code(kind)
        extensions.append((kind, normalize_body(kind, value)))
    if offset != end or offset != len(body):
        raise ValueError('bad extension length')
    extensions.sort(key=lambda item: (item[0], item[1]))
    out = bytearray([1])
    out += struct.pack('>H', len(ciphers))
    for cipher in ciphers:
        out += struct.pack('>H', cipher)
    out.append(len(methods))
    out += methods
    out += struct.pack('>H', len(extensions))
    for kind, value in extensions:
        out += struct.pack('>H', kind) + struct.pack('>H', len(value)) + value
    return bytes(out)
