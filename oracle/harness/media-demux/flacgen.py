import math
import struct


def _crc8_table():
    table = []
    for byte in range(256):
        crc = byte
        for _ in range(8):
            crc = ((crc << 1) ^ 0x07) & 0xff if crc & 0x80 else (crc << 1) & 0xff
        table.append(crc)
    return table


def _crc16_table():
    table = []
    for byte in range(256):
        crc = byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x8005) & 0xffff if crc & 0x8000 else (crc << 1) & 0xffff
        table.append(crc)
    return table


CRC8 = _crc8_table()
CRC16 = _crc16_table()


def crc8(data):
    crc = 0
    for byte in data:
        crc = CRC8[crc ^ byte]
    return crc


def crc16(data):
    crc = 0
    for byte in data:
        crc = ((crc << 8) & 0xffff) ^ CRC16[(crc >> 8) ^ byte]
    return crc


class Bits:
    def __init__(self):
        self.out = bytearray()
        self.value = 0
        self.count = 0

    def put(self, bits, value):
        self.value = (self.value << bits) | (value & ((1 << bits) - 1))
        self.count += bits
        while self.count >= 8:
            self.count -= 8
            self.out.append((self.value >> self.count) & 0xff)
        self.value &= (1 << self.count) - 1

    def unary(self, zeros):
        while zeros >= 32:
            self.put(32, 0)
            zeros -= 32
        self.put(zeros + 1, 1)

    def align(self):
        if self.count % 8:
            self.put(8 - self.count % 8, 0)

    def data(self):
        self.align()
        return bytes(self.out)


def utf8_number(value):
    if value < 0x80:
        return bytes([value])
    for length, lead in ((2, 0xC0), (3, 0xE0), (4, 0xF0), (5, 0xF8), (6, 0xFC), (7, 0xFE)):
        if value < 1 << (5 * length + 1 + (length == 7) * 5):
            break
    out = []
    for _ in range(length - 1):
        out.append(0x80 | (value & 0x3F))
        value >>= 6
    out.append(lead | value)
    return bytes(reversed(out))


BLOCKSIZE_CODES = {192: 1, 576: 2, 1152: 3, 2304: 4, 4608: 5, 256: 8, 512: 9, 1024: 10, 2048: 11, 4096: 12, 8192: 13, 16384: 14, 32768: 15}
RATE_CODES = {88200: 1, 176400: 2, 192000: 3, 8000: 4, 16000: 5, 22050: 6, 24000: 7, 32000: 8, 44100: 9, 48000: 10, 96000: 11}
BPS_CODES = {8: 1, 12: 2, 16: 4, 20: 5, 24: 6, 32: 7}


def frame_header(blocksize, rate, rate_code, channel_code, bps_code, number, variable):
    head = Bits()
    head.put(15, 0x7FFC)
    head.put(1, 1 if variable else 0)
    extra = b''
    if blocksize in BLOCKSIZE_CODES:
        bs_code = BLOCKSIZE_CODES[blocksize]
    elif blocksize <= 256:
        bs_code = 6
        extra += bytes([blocksize - 1])
    else:
        bs_code = 7
        extra += struct.pack('>H', blocksize - 1)
    if rate_code is None:
        if rate in RATE_CODES:
            rate_code = RATE_CODES[rate]
        elif rate % 1000 == 0 and rate // 1000 < 256:
            rate_code = 12
        elif rate < 65536:
            rate_code = 13
        else:
            rate_code = 14
    if rate_code == 12:
        extra += bytes([rate // 1000])
    elif rate_code == 13:
        extra += struct.pack('>H', rate)
    elif rate_code == 14:
        extra += struct.pack('>H', rate // 10)
    head.put(4, bs_code)
    head.put(4, rate_code)
    head.put(4, channel_code)
    head.put(3, bps_code)
    head.put(1, 0)
    header = head.data() + utf8_number(number) + extra
    return header + bytes([crc8(header)])


def zigzag(value):
    return (value << 1) if value >= 0 else ((-value) << 1) - 1


def rice(bits, residuals):
    mean = sum(zigzag(v) for v in residuals) / max(1, len(residuals))
    k = max(0, int(math.log2(mean + 1))) if mean > 0 else 0
    bits.put(2, 0)
    bits.put(4, 0)
    if k > 14:
        width = max(max(abs(v) for v in residuals).bit_length() + 1, 1)
        bits.put(4, 15)
        bits.put(5, width)
        for value in residuals:
            bits.put(width, value)
        return
    bits.put(4, k)
    for value in residuals:
        u = zigzag(value)
        bits.unary(u >> k)
        if k:
            bits.put(k, u & ((1 << k) - 1))


def subframe(bits, samples, bps, kind):
    if kind == 'constant' or all(s == samples[0] for s in samples):
        bits.put(1, 0)
        bits.put(6, 0)
        bits.put(1, 0)
        bits.put(bps, samples[0])
        return
    if kind == 'verbatim' or bps > 24:
        bits.put(1, 0)
        bits.put(6, 1)
        bits.put(1, 0)
        for value in samples:
            bits.put(bps, value)
        return
    order = 2 if kind == 'fixed2' else 1
    bits.put(1, 0)
    bits.put(6, 8 | order)
    bits.put(1, 0)
    for value in samples[:order]:
        bits.put(bps, value)
    if order == 1:
        residuals = [samples[i] - samples[i - 1] for i in range(1, len(samples))]
    else:
        residuals = [samples[i] - 2 * samples[i - 1] + samples[i - 2] for i in range(2, len(samples))]
    rice(bits, residuals)


def decorrelate(channels, mode):
    if mode < 8:
        return [(channel, 0) for channel in channels]
    left, right = channels
    if mode == 8:
        return [(left, 0), ([l - r for l, r in zip(left, right)], 1)]
    if mode == 9:
        return [([l - r for l, r in zip(left, right)], 1), (right, 0)]
    if mode == 10:
        return [([(l + r) >> 1 for l, r in zip(left, right)], 0), ([l - r for l, r in zip(left, right)], 1)]
    return [(channel, 0) for channel in channels]


def frame(channels, bps, rate, number, variable=False, mode=None, kind='fixed1', rate_code=None, bps_code=None, corrupt_crc=False):
    blocksize = len(channels[0])
    channel_code = mode if mode is not None else len(channels) - 1
    data = frame_header(blocksize, rate, rate_code, channel_code, BPS_CODES[bps] if bps_code is None else bps_code, number, variable)
    bits = Bits()
    for samples, extra in decorrelate(channels, channel_code):
        subframe(bits, samples, bps + extra, kind)
    data += bits.data()
    crc = crc16(data) ^ (0x5a5a if corrupt_crc else 0)
    return data + struct.pack('>H', crc)


def signal(rng, frames, channels, bps, shape='sine'):
    peak = (1 << (bps - 1)) - 1
    out = []
    for c in range(channels):
        phase = rng.random() * 6.28
        step = 0.003 + rng.random() * 0.05 * (c + 1)
        if shape == 'silence':
            out.append([0] * frames)
        elif shape == 'noise':
            out.append([rng.randint(-peak, peak) for _ in range(frames)])
        else:
            out.append([max(-peak - 1, min(peak, int(math.sin(phase + i * step) * peak * 0.6 + rng.uniform(-0.01, 0.01) * peak))) for i in range(frames)])
    return out


def block(kind, body, last=False):
    return bytes([(0x80 if last else 0) | kind]) + len(body).to_bytes(3, 'big') + body


def streaminfo(min_block, max_block, min_frame, max_frame, rate, channels, bps, samples, md5=bytes(16)):
    bits = Bits()
    bits.put(16, min_block)
    bits.put(16, max_block)
    bits.put(24, min_frame)
    bits.put(24, max_frame)
    bits.put(20, rate)
    bits.put(3, channels - 1)
    bits.put(5, bps - 1)
    bits.put(36, samples)
    return bits.data() + md5


def vorbis_comment(comments, vendor=b'manganese corpus'):
    body = struct.pack('<I', len(vendor)) + vendor + struct.pack('<I', len(comments))
    for comment in comments:
        body += struct.pack('<I', len(comment)) + comment
    return body


def picture(mime=b'image/png', data=None, kind=3, width=16, height=16, description=b'cover', length=None):
    if data is None:
        data = b'\x89PNG\r\n\x1a\n' + bytes(40)
    body = struct.pack('>I', kind) + struct.pack('>I', len(mime)) + mime + struct.pack('>I', len(description)) + description
    body += struct.pack('>IIII', width, height, 24, 0) + struct.pack('>I', len(data) if length is None else length) + data
    return body


def seektable(points):
    return b''.join(struct.pack('>QQH', sample, offset, count) for sample, offset, count in points)


def cuesheet(tracks=2, index_points=1):
    body = bytearray(b'0000000000000'.ljust(128, b'\0')) + struct.pack('>Q', 0) + bytes([0x80]) + bytes(258)
    body += bytes([tracks + 1])
    for track in range(tracks + 1):
        body += struct.pack('>Q', track * 44100) + bytes([track + 1 if track < tracks else 170]) + b'ISRC00000000' + bytes(14)
        points = index_points if track < tracks else 0
        body += bytes([points]) + b''.join(struct.pack('>Q', 0) + bytes([1]) + bytes(3) for _ in range(points))
    return bytes(body)


def stream(rng, channels=2, bps=16, rate=44100, blocksize=4096, frames=12, variable=False, mode=None, kind='fixed1', shape='sine',
           sizes=None, numbers=None, rate_code=None, bps_code=None, corrupt=()):
    blocks = sizes or [blocksize] * frames
    total = sum(blocks)
    pcm = signal(rng, total, channels, bps, shape)
    out = []
    start = 0
    for index, size in enumerate(blocks):
        chunk = [channel[start:start + size] for channel in pcm]
        number = numbers[index] if numbers else (start if variable else index)
        out.append(frame(chunk, bps, rate, number, variable, mode, kind, rate_code, bps_code, corrupt_crc=index in corrupt))
        start += size
    return out, total, blocks


def flac_file(frames, rate, channels, bps, total, blocks, metadata=(), info=None, marker=b'fLaC', info_last=None):
    if info is None:
        sizes = [len(f) for f in frames] or [0]
        info = streaminfo(min(blocks or [16]), max(blocks or [16]), min(sizes), max(sizes), rate, channels, bps, total)
    head = marker
    blocks_out = [(0, info)] + list(metadata)
    for index, (kind, body) in enumerate(blocks_out):
        last = index == len(blocks_out) - 1 if info_last is None else (info_last and index == 0)
        head += block(kind, body, last)
    return head + b''.join(frames)
