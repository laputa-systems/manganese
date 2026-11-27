import random
import struct
import zlib

LOCAL_SIG = 0x04034b50
CENTRAL_SIG = 0x02014b50
EOCD_SIG = 0x06054b50
EOCD64_SIG = 0x06064b50
LOCATOR64_SIG = 0x07064b50
DESCRIPTOR_SIG = 0x08074b50


def raw_deflate(data, level=6):
    compressor = zlib.compressobj(level, zlib.DEFLATED, -15)
    return compressor.compress(data) + compressor.flush()


def dos_time(year=2024, month=6, day=15, hour=12, minute=30, second=20):
    return ((hour & 31) << 11) | ((minute & 63) << 5) | ((second & 62) >> 1)


def dos_date(year=2024, month=6, day=15):
    return (((max(year, 1980) - 1980) & 127) << 9) | ((month & 15) << 5) | (day & 31)


class Entry:
    def __init__(self, name, data=b'', method=8, flags=0, made_by=0x031E,
                 external=0o600 << 16, extra=b'', local_extra=b'',
                 date=None, time=None, descriptor=None, crc=None,
                 csize=None, usize=None, local_name=None, bad_local_sig=False):
        self.name = name if isinstance(name, bytes) else name.encode()
        self.data = data
        self.method = method
        self.flags = flags
        self.made_by = made_by
        self.external = external
        self.extra = extra
        self.local_extra = local_extra
        self.date = dos_date() if date is None else date
        self.time = dos_time() if time is None else time
        self.descriptor = descriptor
        self.crc = zlib.crc32(data) & 0xffffffff if crc is None else crc
        self.payload = raw_deflate(data) if method == 8 else data
        self.csize = len(self.payload) if csize is None else csize
        self.usize = len(data) if usize is None else usize
        self.local_name = self.name if local_name is None else local_name
        self.bad_local_sig = bad_local_sig


def zip64_extra(usize=None, csize=None, offset=None):
    fields = []
    if usize is not None:
        fields.append(struct.pack('<QQ', usize, csize if csize is not None else 0))
    elif csize is not None:
        fields.append(struct.pack('<Q', csize))
    if offset is not None:
        fields.append(struct.pack('<Q', offset))
    body = b''.join(fields)
    return struct.pack('<HH', 0x0001, len(body)) + body


def unicode_path_extra(name, crc):
    body = bytes([1]) + struct.pack('<I', crc) + name
    return struct.pack('<HH', 0x7075, len(body)) + body


def build(entries, comment=b'', prefix=b'', suffix=b'', fake_eocd64=False,
          eocd64=False, zero_sizes_in_local=False, zip64_local=False,
          central_count=None, eocd_offset=None, cd_offset=None, cd_size=None):
    blob = bytearray(prefix)
    offsets = []
    for entry in entries:
        offsets.append(len(blob))
        flags = entry.flags
        local_usize = 0xffffffff if zip64_local else entry.usize
        local_csize = 0xffffffff if zip64_local else entry.csize
        if zero_sizes_in_local or entry.descriptor is not None:
            local_usize = local_csize = 0
        local_extra = entry.local_extra
        if zip64_local:
            local_extra += zip64_extra(entry.usize, entry.csize)
        sig = LOCAL_SIG if not entry.bad_local_sig else 0x04034b51
        blob += struct.pack('<IHHHHHIIIHH', sig, 45 if zip64_local else 20,
                            flags, entry.method, entry.time, entry.date,
                            entry.crc, local_csize, local_usize,
                            len(entry.local_name), len(local_extra))
        blob += entry.local_name + local_extra + entry.payload
        if entry.descriptor is not None:
            blob += entry.descriptor
    cd_start = len(blob)
    for entry, offset in zip(entries, offsets):
        extra = entry.extra
        if entry.usize >= 0xffffffff or entry.csize >= 0xffffffff or offset >= 0xffffffff or eocd64:
            extra += zip64_extra(entry.usize, entry.csize, offset)
        blob += struct.pack('<IHHHHHHIIIHHHHHII', CENTRAL_SIG, entry.made_by, 20,
                            entry.flags, entry.method, entry.time, entry.date,
                            entry.crc, entry.csize, entry.usize,
                            len(entry.name), len(extra), 0, 0, 0,
                            entry.external, offset)
        blob += entry.name + extra
    cd_size = len(blob) - cd_start if cd_size is None else cd_size
    count = len(entries) if central_count is None else central_count
    if eocd64:
        record_offset = len(blob)
        blob += struct.pack('<IQHHIIQQQQ', EOCD64_SIG, 44, 45, 45, 0, 0,
                            count, count, cd_size, cd_start)
        blob += struct.pack('<IIQI', LOCATOR64_SIG, 0, record_offset, 1)
    if fake_eocd64:
        blob += struct.pack('<IHHHHIIH', EOCD_SIG, 0, 0, count, count,
                            cd_size, cd_start, 0) + b'FAKE'
    offset = cd_start if eocd_offset is None else eocd_offset
    blob += struct.pack('<IHHHHIIH', EOCD_SIG, 0, 0, count, count,
                        cd_size, offset if cd_offset is None else cd_offset,
                        len(comment))
    blob += comment + suffix
    return bytes(blob)


def data_of(size, pattern, seed=0):
    if pattern == 'zero':
        return bytes(size)
    rng = random.Random(seed)
    if pattern == 'text':
        return (b'the quick brown fox jumps over the lazy dog 0123456789\n' * (size // 52 + 1))[:size]
    if pattern == 'seq':
        return bytes((index * 7 + seed) & 255 for index in range(size))
    return bytes(rng.randrange(256) for _ in range(size))


def cases():
    result = []

    def add(name, blob):
        result.append((name, blob))

    for size in (0, 1, 2, 31, 32, 33, 255, 256, 1024, 4096, 65535, 65536):
        for pattern in ('zero', 'text', 'seq', 'random'):
            data = data_of(size, pattern, size + len(pattern))
            add(f'size-{size}-{pattern}-store', build([Entry('f.txt', data, method=0)]))
            add(f'size-{size}-{pattern}-deflate', build([Entry('f.txt', data, method=8)]))

    names = [b'plain.txt', b'dir/file.txt', b'dir/sub/', b'../up.txt', b'./dot.txt',
             b'/root.txt', b'a\\b.txt', b'c:/temp.txt', b'unicode-\xc3\xa9.txt',
             b'raw-\xff.txt', b'x' * 255, b'spaces in name.txt']
    for index, name in enumerate(names):
        data = data_of(16 + index, 'seq', index)
        add(f'name-{index}-utf8', build([Entry(name, data, flags=0x800)]))
        add(f'name-{index}-cp437', build([Entry(name, data, flags=0)]))
        add(f'name-{index}-dos-madeby', build([Entry(name, data, made_by=0x0014)]))

    for index, count in enumerate((2, 3, 5, 20)):
        entries = [Entry(f'e{j}.txt', data_of(j + 1, 'seq', j), j % 2) for j in range(count)]
        add(f'multi-{index}', build(entries))
        add(f'multi-{index}-dd', build([Entry(f'e{j}.txt', data_of(j + 1, 'seq', j),
                                              j % 2, flags=8) for j in range(count)]))

    data = b'data descriptor payload\n'
    for index, descriptor in enumerate((
            None,
            struct.pack('<IIII', DESCRIPTOR_SIG, zlib.crc32(data) & 0xffffffff, len(raw_deflate(data)), len(data)),
            struct.pack('<III', zlib.crc32(data) & 0xffffffff, len(raw_deflate(data)), len(data)),
            struct.pack('<IQQQ', DESCRIPTOR_SIG, zlib.crc32(data) & 0xffffffff, len(raw_deflate(data)), len(data)))):
        if descriptor is None:
            descriptor = struct.pack('<IIII', DESCRIPTOR_SIG, 0, 0, 0)
        add(f'descriptor-{index}', build([Entry('dd.txt', data, flags=8, descriptor=descriptor)],
                                         zero_sizes_in_local=True))

    single = [Entry('z64.txt', b'zip64 payload' * 4)]
    add('zip64-eocd', build(single, eocd64=True))
    add('zip64-local', build(single, zip64_local=True))
    add('zip64-both', build(single, eocd64=True, zip64_local=True))
    add('zip64-extra-central', build([Entry('z64e.txt', b'extra' * 8,
                                            extra=zip64_extra(len(b'extra' * 8),
                                                              len(raw_deflate(b'extra' * 8))))]))

    alias = b'alias.txt'
    add('unicode-path-extra', build([Entry(b'physical.txt', b'aliased' * 3, flags=0x800,
                                           extra=unicode_path_extra(alias, zlib.crc32(b'physical.txt')))]))

    for index, (hour, minute, second, month, day, year) in enumerate((
            (0, 0, 0, 1, 1, 1980), (23, 59, 58, 12, 31, 2107),
            (24, 60, 60, 0, 0, 1980), (12, 30, 20, 2, 29, 2024),
            (0, 0, 0, 13, 32, 1979))):
        add(f'time-{index}', build([Entry('t.txt', b'time', date=dos_date(year, month, day),
                                          time=dos_time(hour, minute, second))]))

    for index, external in enumerate((0, 0o644 << 16, 0o755 << 16, 0o120755 << 16,
                                      0o40755 << 16, 0xffffffff)):
        add(f'mode-{index}', build([Entry('mode.txt', b'mode', external=external)]))

    secret = b'encrypted payload'
    add('encrypted-zipcrypto', build([Entry('secret.txt', secret, flags=1)]))
    add('encrypted-aes', build([Entry('secret.txt', secret, flags=1, method=99)]))

    add('unsupported-method', build([Entry('bzip.txt', b'bzip data', method=12)]))
    add('empty-archive', build([]))
    add('comment', build([Entry('c.txt', b'commented')], comment=b'archive comment' * 3))
    add('prefix-junk', build([Entry('s.txt', b'sfx')], prefix=b'MZ self extracting stub\x00\x01'))
    add('trailing-junk', build([Entry('t.txt', b'trailing')], suffix=b'JUNK' * 30))
    good = build([Entry('good.txt', b'good data')])
    add('corrupt-crc', build([Entry('crc.txt', b'good data', crc=0x12345678)]))
    add('corrupt-size', build([Entry('size.txt', b'good data', usize=999)]))
    add('corrupt-truncated', good[:-40])
    add('corrupt-no-eocd', good[:good.rfind(b'PK\x05\x06')])
    add('corrupt-local-sig', build([Entry('sig.txt', b'data', bad_local_sig=True)]))
    add('corrupt-local-name', build([Entry('name.txt', b'data', local_name=b'wrong.txt')]))
    add('corrupt-local-size', build([Entry('lsize.txt', b'data', csize=99)]))
    add('corrupt-cd-offset', good[:good.rfind(b'PK\x05\x06')] +
        struct.pack('<IHHHHIIH', EOCD_SIG, 0, 0, 1, 1, 33, 99999, 0))
    add('corrupt-cd-count', build([Entry('count.txt', b'data')], central_count=2))
    add('corrupt-empty-name', build([Entry(b'', b'data')]))
    add('duplicate-names', build([Entry('dup.txt', b'first'), Entry('dup.txt', b'second')]))

    for index, extra in enumerate((b'\x01\x00\x00\x00', b'\x75\x07\x08\x00\x04',
                                   struct.pack('<HH', 0x5455, 5) + b'\x01\x00\x00\x00\x00')):
        add(f'extra-{index}', build([Entry('x.txt', b'extra data', extra=extra)]))

    return result


def random_case(rng):
    size = rng.choice((0, 1, 17, 64, 257, 1024, 4096))
    data = data_of(size, rng.choice(('zero', 'text', 'seq', 'random')), rng.randrange(1 << 30))
    name = rng.choice(names_bytes(rng))
    method = rng.choice((0, 8, 8, 8, 12, 99))
    flags = rng.choice((0, 0, 0x800, 8, 0x808, 1, 0x801))
    external = rng.choice((0, 0o600 << 16, 0o755 << 16, 0o120755 << 16))
    entry = Entry(name, data, method=method, flags=flags, external=external,
                  date=dos_date(rng.choice((1980, 2000, 2024, 2107)),
                                rng.choice((0, 1, 6, 12, 13)),
                                rng.choice((0, 1, 15, 32))),
                  time=dos_time(rng.choice((0, 5, 12, 23, 24)),
                                rng.choice((0, 30, 60)), rng.choice((0, 20, 60))))
    if flags & 8:
        entry.descriptor = struct.pack('<IIII', DESCRIPTOR_SIG,
                                       zlib.crc32(data) & 0xffffffff,
                                       len(entry.payload), len(data))
    return build([entry], comment=bytes(rng.randrange(256) for _ in range(rng.randrange(8))),
                 prefix=bytes(rng.randrange(256) for _ in range(rng.randrange(8))))


def names_bytes(rng):
    return [b'f.txt', b'd/e.txt', b'../u.txt', b'/abs.txt', b'x\\y.txt',
            b'u-\xc3\xa9.txt', b'r-\xff.txt', b'n' * rng.randrange(1, 40)]


def mutate(data, rng):
    blob = bytearray(data)
    for _ in range(rng.randrange(1, 4)):
        if not blob:
            break
        position = rng.randrange(len(blob))
        blob[position] = rng.randrange(256)
    return bytes(blob)


def encode_request(data):
    return b'\x01' + data
