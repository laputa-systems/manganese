#!/usr/bin/env -S python3 -B
import copy
import os
import pathlib
import random
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import fuzzdriver
import wire

ML_DSA_OIDS = (bytes.fromhex('0609608648016503040311'), bytes.fromhex('0609608648016503040312'), bytes.fromhex('0609608648016503040313'))
SPECIAL_TIMES = (0, 1700000000, 1760000000, 1790000000, 1791207587, 1797255587, 1797255588, 1800000000, 1900000000, 2524608000, 253402300799, -62135596800)
STEPS = (1, 59, 3600, 86400, 86400 * 30, 86400 * 400)

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
seeds = []
for row in pathlib.Path('goldens.lock').read_text().splitlines():
    fields = row.split()
    if len(fields) == 7 and fields[0:2] == ['cert-verify', 'seen']:
        seeds.append(wire.decode((store / fields[3]).read_bytes()))
certificates = sorted({der for record in seeds for der in record.chain + [t.der for t in record.trust if t.kind != wire.DISTRUSTED_SPKI]
                       if not any(oid in der for oid in ML_DSA_OIDS)})
hostnames = sorted({record.hostname for record in seeds})
seeds = [record for record in seeds if not any(oid in der for der in record.chain + [t.der for t in record.trust] for oid in ML_DSA_OIDS)]
seed, cases = fuzzdriver.budget()
rng = random.Random(seed)


def read_tlv(data, offset):
    if offset + 2 > len(data):
        return None
    size = data[offset + 1]
    header = 2
    if size & 0x80:
        count = size & 0x7f
        if count == 0 or count > 4 or offset + 2 + count > len(data):
            return None
        size = int.from_bytes(data[offset + 2:offset + 2 + count], 'big')
        header += count
    end = offset + header + size
    return (offset + header, end) if end <= len(data) else None


def spki(der):
    outer = read_tlv(der, 0)
    tbs = outer and read_tlv(der, outer[0])
    if not tbs:
        return None
    offset = tbs[0]
    fields = []
    while offset < tbs[1] and len(fields) < 7:
        element = read_tlv(der, offset)
        if not element:
            return None
        fields.append((der[offset], offset, element[1]))
        offset = element[1]
    index = 6 if fields and fields[0][0] == 0xa0 else 5
    return der[fields[index][1]:fields[index][2]] if len(fields) > index else None


def mutate_bytes(der):
    data = bytearray(der)
    for _ in range(rng.randrange(1, 4)):
        choice = rng.randrange(5)
        if choice == 0:
            data[rng.randrange(len(data))] ^= 1 << rng.randrange(8)
        elif choice == 1:
            data[rng.randrange(len(data))] = rng.randrange(256)
        elif choice == 2 and len(data) > 1:
            start = rng.randrange(len(data))
            del data[start:start + rng.randrange(1, 8)]
        elif choice == 3:
            data.insert(rng.randrange(len(data) + 1), rng.randrange(256))
        else:
            data = data[:rng.randrange(1, len(data) + 1)]
        if not data:
            data = bytearray(b'\x30')
    return bytes(data)


def trust_entry(der):
    kind = rng.randrange(7)
    if kind == wire.DISTRUSTED_SPKI:
        return wire.Trust(kind, spki(der) or der)
    entry = wire.Trust(kind, der)
    if kind in wire.CONSTRAINED_KINDS and rng.randrange(2):
        entry.dns = rng.sample([b'example.test', b'other.test', b'www.example.test', b'.example.test', b'example.com', b''], rng.randrange(1, 3))
        if rng.randrange(2):
            entry.cidrs = [rng.choice([(bytes([192, 0, 2, 0]), bytes([255, 255, 255, 0])), (bytes(16), bytes(16)), (bytes([10, 0, 0, 0]), bytes([255, 0, 0, 0]))])]
    return entry


def mutate(record):
    choice = rng.randrange(12)
    if choice == 0:
        record.time = rng.choice(SPECIAL_TIMES) if rng.randrange(3) == 0 else record.time + rng.choice((-1, 1)) * rng.choice(STEPS)
        record.time = max(-62135596800, min(253402300799, record.time))
    elif choice == 1:
        record.hostname = rng.choice(hostnames)
    elif choice == 2 and len(record.chain) > 1:
        del record.chain[rng.randrange(1, len(record.chain))]
    elif choice == 3:
        record.chain.insert(rng.randrange(1, len(record.chain) + 1), rng.choice(certificates))
    elif choice == 4:
        tail = record.chain[1:]
        rng.shuffle(tail)
        record.chain[1:] = tail
    elif choice == 5:
        record.chain[0] = rng.choice(certificates)
    elif choice == 6 and record.trust:
        del record.trust[rng.randrange(len(record.trust))]
    elif choice == 7:
        record.trust.insert(rng.randrange(len(record.trust) + 1), trust_entry(rng.choice(certificates + record.chain)))
    elif choice == 8 and record.trust:
        index = rng.randrange(len(record.trust))
        old = record.trust[index]
        record.trust[index] = trust_entry(old.der) if old.kind != wire.DISTRUSTED_SPKI else old
    elif choice == 9:
        index = rng.randrange(len(record.chain))
        record.chain[index] = mutate_bytes(record.chain[index])
    elif choice == 10 and record.trust:
        index = rng.randrange(len(record.trust))
        record.trust[index].der = mutate_bytes(record.trust[index].der)
    elif choice == 11 and len(record.chain) > 1:
        index = rng.randrange(1, len(record.chain))
        record.trust.append(wire.Trust(rng.choice((wire.ANCHOR, wire.ANCHOR_ENFORCED, wire.HINT)), record.chain.pop(index)))


def generate(oracle):
    record = copy.deepcopy(rng.choice(seeds))
    for _ in range(rng.randrange(1, 5)):
        mutate(record)
    return wire.encode(record)


fuzzdriver.run('cert-verify', seed, cases, generate)
