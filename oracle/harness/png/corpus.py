import random
import struct
import zlib

SIGNATURE = b'\x89PNG\r\n\x1a\n'
PASSES = ((0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2))
CHANNELS = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}


def chunk(kind, data):
    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))


def paeth(a, b, c):
    p = a + b - c
    distances = (abs(p - a), abs(p - b), abs(p - c))
    return (a, b, c)[distances.index(min(distances))]


def filtered(row, previous, bpp, kind):
    result = bytearray()
    for i, value in enumerate(row):
        left = row[i - bpp] if i >= bpp else 0
        up = previous[i] if previous else 0
        upper_left = previous[i - bpp] if previous and i >= bpp else 0
        predictor = (0, left, up, (left + up) // 2, paeth(left, up, upper_left))[kind]
        result.append((value - predictor) & 255)
    return bytes([kind]) + bytes(result)


def sample_row(width, x0, y, dx, channels, depth, seed):
    samples = []
    for x in range(x0, width, dx):
        for channel in range(channels):
            samples.append((x * 37 + y * 53 + channel * 67 + seed * 19) % (1 << depth))
    if depth == 16:
        return b''.join(struct.pack('>H', value) for value in samples)
    if depth == 8:
        return bytes(samples)
    packed = bytearray((len(samples) * depth + 7) // 8)
    for index, value in enumerate(samples):
        packed[index * depth // 8] |= value << (8 - depth - (index * depth % 8))
    return bytes(packed)


def image_data(width, height, depth, color, interlaced, filter_kind, seed):
    channels = CHANNELS[color]
    bpp = max(1, (channels * depth + 7) // 8)
    passes = PASSES if interlaced else ((0, 0, 1, 1),)
    data = bytearray()
    for x0, y0, dx, dy in passes:
        if x0 >= width or y0 >= height:
            continue
        previous = None
        for y in range(y0, height, dy):
            row = sample_row(width, x0, y, dx, channels, depth, seed)
            data.extend(filtered(row, previous, bpp, filter_kind))
            previous = row
    return zlib.compress(bytes(data), 6)


def make_png(width, height, depth, color, interlaced, filter_kind, seed, color_chunk=None, animated=False):
    parts = [SIGNATURE, chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, depth, color, 0, 0, int(interlaced)))]
    if color_chunk == 'gamma':
        parts.append(chunk(b'gAMA', struct.pack('>I', 45455)))
    elif color_chunk == 'srgb':
        parts.append(chunk(b'sRGB', b'\x00'))
    elif color_chunk == 'chromaticity':
        parts.append(chunk(b'cHRM', struct.pack('>8I', 31270, 32900, 64000, 33000, 30000, 60000, 15000, 6000)))
    elif color_chunk == 'icc':
        parts.append(chunk(b'iCCP', b'profile\x00\x00' + zlib.compress(bytes(128))))
    if color == 3:
        count = 1 << depth
        parts.append(chunk(b'PLTE', bytes(channel for i in range(count) for channel in ((i * 47) & 255, (i * 83) & 255, (i * 131) & 255))))
        parts.append(chunk(b'tRNS', bytes((i * 97) & 255 for i in range(count))))
    elif color == 0:
        parts.append(chunk(b'tRNS', struct.pack('>H', seed % (1 << depth))))
    elif color == 2:
        parts.append(chunk(b'tRNS', struct.pack('>HHH', 1, 2, 3)))
    if animated:
        parts.append(chunk(b'acTL', struct.pack('>II', 7, 3)))
        parts.append(chunk(b'fcTL', struct.pack('>IIIIIHHBB', 0, width, height, 0, 0, 1, 10, 0, 0)))
    parts.append(chunk(b'IDAT', image_data(width, height, depth, color, interlaced, filter_kind, seed)))
    if animated:
        sequence = 1
        for dispose in range(3):
            for blend in range(2):
                parts.append(chunk(b'fcTL', struct.pack('>IIIIIHHBB', sequence, width, height, 0, 0, dispose + 2, 10, dispose, blend)))
                sequence += 1
                data = image_data(width, height, depth, color, interlaced, (filter_kind + sequence) % 5, seed + sequence)
                parts.append(chunk(b'fdAT', struct.pack('>I', sequence) + data))
                sequence += 1
    parts.append(chunk(b'IEND', b''))
    return b''.join(parts)


def decode_request(data, cuts):
    chunks = []
    start = 0
    for end in cuts:
        end = min(max(start, end), len(data))
        chunks.append(data[start:end])
        start = end
    chunks.append(data[start:])
    return b'\x00' + struct.pack('>I', len(chunks)) + b''.join(struct.pack('>I', len(part)) + part for part in chunks)


def encode_request(width, height, mode, seed, opaque):
    rng = random.Random(seed)
    pixels = bytearray()
    for _ in range(width * height):
        alpha = 255 if opaque else rng.randrange(256)
        pixels.extend((rng.randrange(alpha + 1), rng.randrange(alpha + 1), rng.randrange(alpha + 1), alpha))
    return b'\x01' + struct.pack('>III', width, height, mode + (3 if opaque else 0)) + pixels
