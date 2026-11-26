import struct

CRC_TABLE = [0] * 256
for _index in range(256):
    _value = _index << 24
    for _ in range(8):
        _value = ((_value << 1) ^ 0x04c11db7) & 0xffffffff if _value & 0x80000000 else (_value << 1) & 0xffffffff
    CRC_TABLE[_index] = int.from_bytes(_value.to_bytes(4, 'big'), 'little')


def crc(data):
    value = 0x4fa9b05f
    for byte in data[4:]:
        value = CRC_TABLE[(value & 0xff) ^ byte] ^ (value >> 8)
    return value


def page(serial, seq, granule, flags, packets):
    lacing = []
    for packet in packets:
        size = len(packet)
        while size >= 255:
            lacing.append(255)
            size -= 255
        lacing.append(size)
    header = b'OggS' + bytes([0, flags]) + struct.pack('<qII', granule, serial, seq) + bytes(4) + bytes([len(lacing)]) + bytes(lacing)
    body = b''.join(packets)
    checksum = crc(header + body)
    return header[:22] + struct.pack('>I', checksum) + header[26:] + body


def head(channels=2, pre_skip=312, rate=48000, mapping=0, version=1, size=19):
    out = b'OpusHead' + bytes([version, channels]) + struct.pack('<H', pre_skip) + struct.pack('<I', rate) + struct.pack('<h', 0) + bytes([mapping])
    return out[:size].ljust(size, b'\0')


def tags(vendor=b'test', comments=(), size=None):
    out = b'OpusTags' + struct.pack('<I', len(vendor)) + vendor + struct.pack('<I', len(comments))
    for comment in comments:
        out += struct.pack('<I', len(comment)) + comment
    return out if size is None else out[:size].ljust(size, b'\0')


def frame_duration(toc, payload=b'\0'):
    config = toc >> 3
    count = toc & 3
    if config < 12:
        frame = max(480, 960 * (config & 3))
    elif config < 16:
        frame = 480 << (config & 1)
    else:
        frame = 120 << (config & 3)
    if count == 3:
        frames = payload[1] & 0x3f if len(payload) > 1 else 0
    elif count:
        frames = 2
    else:
        frames = 1
    return frame * frames


def packet(toc, payload):
    return bytes([toc]) + payload


def opus(count=100, toc=8, payload=b'\0', pre_skip=312, channels=2, serial=0x1234, per_page=20, vendor=b'test',
         comments=(), head_size=19, tags_size=None, granule_shift=0, bos_flags=2, tag_flags=0, data_flags=None,
         drop_eos=False, drop_tags=False, end_granule=None, last_page=None):
    duration = frame_duration(toc, payload)
    out = page(serial, 0, 0, bos_flags, [head(channels=channels, pre_skip=pre_skip, size=head_size)])
    seq = 1
    if not drop_tags:
        out += page(serial, seq, 0, tag_flags, [tags(vendor, comments, tags_size)])
        seq += 1
    total = count * duration
    produced = 0
    while produced < count:
        chunk_count = min(per_page, count - produced)
        packets = [packet(toc, payload) for _ in range(chunk_count)]
        produced += chunk_count
        granule = pre_skip + produced * duration + granule_shift
        if produced == count and not drop_eos:
            flags = 4
        else:
            flags = 0
        if data_flags is not None:
            flags = data_flags
        out += page(serial, seq, granule, flags, packets)
        seq += 1
    if end_granule is not None:
        out = set_granule(out, seq - 1, end_granule)
    return out


def chained(first, second, serial=0x5678, count=60, toc=8, pre_skip=0):
    duration = frame_duration(toc)
    out = first + page(serial, 0, 0, 2, [head(channels=2, pre_skip=pre_skip)]) + page(serial, 1, 0, 0, [tags()])
    out += page(serial, 2, pre_skip + count * duration, 4, [packet(toc, b'\0') for _ in range(count)])
    return out


def corrupt_crc(data):
    data = bytearray(data)
    data[24] ^= 0x40
    return bytes(data)


def bad_version(data):
    data = bytearray(data)
    data[4] = 1
    return bytes(data)


def page_at(data, index):
    pos = 0
    for _ in range(index):
        size = data[pos + 26]
        pos += 27 + size + sum(data[pos + 27:pos + 27 + size])
    size = data[pos + 26]
    end = pos + 27 + size + sum(data[pos + 27:pos + 27 + size])
    return pos, end


def replace_page(data, index, replacement):
    start, end = page_at(data, index)
    return data[:start] + replacement + data[end:]


def set_granule(data, index, granule):
    start, end = page_at(data, index)
    page = bytearray(data[start:end])
    page[6:14] = struct.pack('<q', granule)
    page[22:26] = bytes(4)
    value = crc(page)
    page[22:26] = struct.pack('>I', value)
    return data[:start] + bytes(page) + data[end:]
