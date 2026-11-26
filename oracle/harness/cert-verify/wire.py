import hashlib
import struct
from dataclasses import dataclass, field

ANCHOR = 0
ANCHOR_ENFORCED = 1
ANCHOR_CONSTRAINED = 2
LEAF = 3
ANCHOR_OR_LEAF = 4
HINT = 5
DISTRUSTED_SPKI = 6
CONSTRAINED_KINDS = (ANCHOR_CONSTRAINED, LEAF, ANCHOR_OR_LEAF)


@dataclass
class Trust:
    kind: int
    der: bytes
    dns: list = field(default_factory=list)
    cidrs: list = field(default_factory=list)


@dataclass
class Record:
    time: int
    hostname: bytes
    chain: list
    trust: list = field(default_factory=list)
    scts: bytes = b''
    ocsp: bytes = b''


def encode(record):
    out = bytearray([1])
    out += struct.pack('>q', record.time)
    out += struct.pack('>H', len(record.hostname)) + record.hostname
    out += struct.pack('>H', len(record.chain))
    for der in record.chain:
        out += struct.pack('>I', len(der)) + der
    out += struct.pack('>H', len(record.trust))
    for entry in record.trust:
        out += bytes([entry.kind]) + struct.pack('>I', len(entry.der)) + entry.der
        if entry.kind in CONSTRAINED_KINDS:
            out += struct.pack('>H', len(entry.dns))
            for name in entry.dns:
                out += struct.pack('>H', len(name)) + name
            out += struct.pack('>H', len(entry.cidrs))
            for address, mask in entry.cidrs:
                out += bytes([len(address)]) + address + mask
    if record.scts or record.ocsp:
        out += struct.pack('>H', len(record.scts)) + record.scts
        out += struct.pack('>H', len(record.ocsp)) + record.ocsp
    return bytes(out)


class Reader:
    def __init__(self, data):
        self.data, self.offset = data, 0

    def take(self, size):
        if self.offset + size > len(self.data):
            raise ValueError('short record')
        chunk = self.data[self.offset:self.offset + size]
        self.offset += size
        return chunk

    def uint(self, size):
        return int.from_bytes(self.take(size), 'big')


def decode(data):
    reader = Reader(data)
    if reader.uint(1) != 1:
        raise ValueError('version')
    time = struct.unpack('>q', reader.take(8))[0]
    hostname = reader.take(reader.uint(2))
    chain = [reader.take(reader.uint(4)) for _ in range(reader.uint(2))]
    trust = []
    for _ in range(reader.uint(2)):
        kind = reader.uint(1)
        entry = Trust(kind, reader.take(reader.uint(4)))
        if kind in CONSTRAINED_KINDS:
            entry.dns = [reader.take(reader.uint(2)) for _ in range(reader.uint(2))]
            for _ in range(reader.uint(2)):
                width = reader.uint(1)
                entry.cidrs.append((reader.take(width), reader.take(width)))
        trust.append(entry)
    scts, ocsp = b'', b''
    if reader.offset < len(data):
        scts = reader.take(reader.uint(2))
        if reader.offset < len(data):
            ocsp = reader.take(reader.uint(2))
    if reader.offset != len(data):
        raise ValueError('trailing bytes')
    return Record(time, hostname, chain, trust, scts, ocsp)


REGRESSIONS = frozenset({'ec-intermediate-p-521'})


def lane(label):
    if label in REGRESSIONS:
        return 'seen'
    return 'holdout' if hashlib.sha256(label.encode()).digest()[0] < 90 else 'seen'
