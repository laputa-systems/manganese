import struct

import flacgen

BITRATES = [
    [[0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448],
     [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384],
     [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320]],
    [[0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256],
     [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
     [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160]],
]
FREQS = [44100, 48000, 32000]
VERSION_BITS = {'1': 3, '2': 2, '2.5': 0}
MONO = 3


def lsf(version):
    return 0 if version == '1' else 1


def sample_rate(version, rate_index):
    return FREQS[rate_index] >> (lsf(version) + (version == '2.5'))


def samples_per_frame(version, layer):
    return {1: 384, 2: 1152}.get(layer, 576 if lsf(version) else 1152)


def frame_size(version, layer, bitrate_index, rate_index, padding):
    rate = sample_rate(version, rate_index)
    kbps = BITRATES[lsf(version)][layer - 1][bitrate_index]
    if layer == 1:
        return (kbps * 12000 // rate + padding) * 4
    if layer == 2:
        return kbps * 144000 // rate + padding
    return kbps * 144000 // (rate << lsf(version)) + padding


def header(version='1', layer=3, bitrate_index=9, rate_index=0, padding=0, protected=False, mode=0, mode_ext=0, emphasis=0):
    return (0x7ff << 21 | VERSION_BITS[version] << 19 | (4 - layer) << 17 | (0 if protected else 1) << 16 | bitrate_index << 12 |
            rate_index << 10 | padding << 9 | mode << 6 | mode_ext << 4 | 1 << 2 | emphasis)


def side_info_size(version, mode):
    if lsf(version):
        return 9 if mode == MONO else 17
    return 17 if mode == MONO else 32


def mpa_crc(data):
    crc = 0xffff
    for byte in data:
        crc = ((crc << 8) & 0xffff) ^ flacgen.CRC16[(crc >> 8) ^ byte]
    return crc


LOW_SEVEN_BITS = bytes(byte & 0x7f for byte in range(256))


def filler(rng, size):
    head = min(size, 24)
    return rng.randbytes(head).translate(LOW_SEVEN_BITS) + bytes(size - head)


def frame(rng, version='1', layer=3, bitrate_index=9, rate_index=0, padding=0, protected=False, mode=0, mode_ext=0, side=None, body=None,
          size=None):
    head = header(version, layer, bitrate_index, rate_index, padding, protected, mode, mode_ext)
    total = size if size is not None else frame_size(version, layer, bitrate_index, rate_index, padding)
    side = side if side is not None else bytes(side_info_size(version, mode) if layer == 3 else 0)
    out = bytearray(struct.pack('>I', head))
    if protected:
        out += struct.pack('>H', mpa_crc(out[2:4] + side))
    out += side
    if body is not None:
        out += body
    out += filler(rng, max(0, total - len(out)))
    return bytes(out[:total])


def stream(rng, count, version='1', layer=3, bitrate_index=9, rate_index=0, mode=0, padding='auto', **options):
    frames = []
    rate = sample_rate(version, rate_index)
    spf = samples_per_frame(version, layer)
    kbps = BITRATES[lsf(version)][layer - 1][bitrate_index]
    residue = 0
    for _ in range(count):
        if padding == 'auto':
            exact = spf // 8 * kbps * 1000 if layer != 1 else 12 * kbps * 1000
            residue += exact % rate
            pad = 1 if residue >= rate else 0
            residue -= rate * pad
        else:
            pad = padding
        frames.append(frame(rng, version, layer, bitrate_index, rate_index, pad, mode=mode, **options))
    return frames


def side_info(version, mode, granules):
    bits = flacgen.Bits()
    channels = 1 if mode == MONO else 2
    if lsf(version):
        bits.put(8, 0)
        bits.put(channels, 0)
    else:
        bits.put(9, 0)
        bits.put(5 if channels == 1 else 3, 0)
        bits.put(4 * channels, 0)
    for granule in granules[:1 if lsf(version) else 2]:
        for channel in range(channels):
            fields = granule[channel] if isinstance(granule, list) else granule
            big_values = fields.get('big_values', 0)
            bits.put(12, fields.get('part2_3', 0))
            bits.put(9, big_values)
            bits.put(8, fields.get('global_gain', 0))
            bits.put(9 if lsf(version) else 4, 0)
            if 'block_type' in fields:
                bits.put(1, 1)
                bits.put(2, fields['block_type'])
                bits.put(1, 0)
                bits.put(10, 0)
                bits.put(9, 0)
            else:
                bits.put(1, 0)
                bits.put(15, 0)
                bits.put(7, 0)
            if not lsf(version):
                bits.put(1, 0)
            bits.put(2, 0)
    size = side_info_size(version, mode)
    out = bytes(bits.out) + (bytes([bits.value << (8 - bits.count)]) if bits.count else b'')
    return out.ljust(size, b'\0')[:size]


def xing_offset(version, mode):
    if lsf(version):
        return 9 if mode == MONO else 17
    return 17 if mode == MONO else 32


def info_frame(rng, version='1', bitrate_index=9, rate_index=0, mode=0, tag=b'Xing', frames=None, size=None, toc=False, quality=None,
               encoder=b'LAME3.100', delays=None, crc=True, flags=None):
    total = frame_size(version, 3, bitrate_index, rate_index, 0)
    head = struct.pack('>I', header(version, 3, bitrate_index, rate_index, 0, False, mode))
    body = bytearray(head + bytes(xing_offset(version, mode)))
    body += tag
    present = (frames is not None) | (size is not None) << 1 | toc << 2 | (quality is not None) << 3
    body += struct.pack('>I', present if flags is None else flags)
    if frames is not None:
        body += struct.pack('>I', frames & 0xffffffff)
    if size is not None:
        body += struct.pack('>I', size & 0xffffffff)
    if toc:
        body += bytes(min(255, i * 256 // 100) for i in range(100))
    if quality is not None:
        body += struct.pack('>I', quality)
    if encoder is not None:
        body += encoder.ljust(9, b' ')[:9]
        body += bytes([0x05, 0x10]) + struct.pack('>I', 0x00800000) + struct.pack('>HH', 0x2000 | 47, 0x4000 | 0x200 | 12)
        body += bytes([0x03, 0x80])
        start, end = delays if delays is not None else (576, 1500)
        body += bytes([start >> 4, (start & 15) << 4 | end >> 8, end & 255])
        body += bytes([0x00, 0x00]) + struct.pack('>HIH', 0, total * (frames or 0), 0)
        checksum = crc16_lame(body)
        body += struct.pack('>H', checksum if crc else checksum ^ 0x1234)
    return bytes(body).ljust(total, b'\0')[:total]


def vbri_frame(rng, version='1', bitrate_index=9, rate_index=0, mode=0, frames=0, size=0, version_field=1):
    total = frame_size(version, 3, bitrate_index, rate_index, 0)
    body = bytearray(struct.pack('>I', header(version, 3, bitrate_index, rate_index, 0, False, mode)) + bytes(32))
    body += b'VBRI' + struct.pack('>HHHII', version_field, 1000, 75, size & 0xffffffff, frames & 0xffffffff)
    return bytes(body).ljust(total, b'\0')[:total]


def _crc16_lame_table():
    table = []
    for byte in range(256):
        crc = byte
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
        table.append(crc)
    return table


CRC16_LAME = _crc16_lame_table()


def crc16_lame(data):
    crc = 0
    for byte in data:
        crc = (crc >> 8) ^ CRC16_LAME[(crc ^ byte) & 0xff]
    return crc


def syncsafe(value):
    return bytes([(value >> 21) & 0x7f, (value >> 14) & 0x7f, (value >> 7) & 0x7f, value & 0x7f])


def id3_frame(name, payload, version=3, flags=0, size=None):
    length = len(payload) if size is None else size
    if version == 2:
        return name[:3] + length.to_bytes(3, 'big') + payload
    encoded = syncsafe(length) if version == 4 else struct.pack('>I', length & 0xffffffff)
    return name + encoded + struct.pack('>H', flags) + payload


def text(value, encoding=0):
    if encoding in (1, 2):
        return bytes([encoding]) + utf16(value, encoding) + b'\0\0'
    return bytes([encoding]) + value + b'\0'


def utf16(value, encoding):
    if encoding == 1:
        return b'\xff\xfe' + value.decode('latin-1').encode('utf-16-le')
    return value.decode('latin-1').encode('utf-16-be')


def comment(description, value, language=b'eng', encoding=0):
    if encoding in (1, 2):
        return bytes([encoding]) + language + utf16(description, encoding) + b'\0\0' + utf16(value, encoding)
    return bytes([encoding]) + language + description + b'\0' + value


def tag(frames, version=3, flags=0, padding=0, extended=None):
    body = b''
    if extended is not None:
        body += extended
    body += b''.join(frames) + bytes(padding)
    out = b'ID3' + bytes([version, 0, flags]) + syncsafe(len(body)) + body
    if version == 4 and flags & 0x10:
        out += b'3DI' + bytes([version, 0, flags]) + syncsafe(len(body))
    return out


def unsynchronise(data):
    out = bytearray()
    for index, byte in enumerate(data):
        out.append(byte)
        if byte == 0xff and (index + 1 == len(data) or data[index + 1] == 0 or data[index + 1] & 0xe0 == 0xe0):
            out.append(0)
    return bytes(out)


def smpb(priming, remainder, samples, style='apple'):
    if style == 'apple':
        words = [0, priming, remainder, samples] + [0] * 8
        return b' ' + b' '.join(b'%08X' % word if index != 3 else b'%016X' % word for index, word in enumerate(words))
    return style % (priming, remainder, samples)


def id3v1(title=b'Title'):
    return b'TAG' + title.ljust(30, b'\0') + b'Artist'.ljust(30, b'\0') + b'Album'.ljust(30, b'\0') + b'2026' + bytes(28) + b'\0\x01\x0c'


def ape_tag():
    item = struct.pack('<II', 5, 0) + b'Title\0' + b'Hello'
    footer = b'APETAGEX' + struct.pack('<IIII', 2000, 32 + len(item), 1, 0x80000000) + bytes(8)
    header = b'APETAGEX' + struct.pack('<IIII', 2000, 32 + len(item), 1, 0xA0000000) + bytes(8)
    return header + item + footer
