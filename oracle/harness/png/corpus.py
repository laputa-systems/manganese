import random
import struct
import zlib

SIGNATURE = b'\x89PNG\r\n\x1a\n'
PASSES = ((0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2))
CHANNELS = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}


def chunk(kind: bytes, data: bytes) -> bytes:
    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))


def paeth(a: int, b: int, c: int) -> int:
    p = a + b - c
    distances = (abs(p - a), abs(p - b), abs(p - c))
    return (a, b, c)[distances.index(min(distances))]


def filtered(row: bytes, previous: bytes | None, bpp: int, kind: int) -> bytes:
    result = bytearray()
    for i, value in enumerate(row):
        left = row[i - bpp] if i >= bpp else 0
        up = previous[i] if previous else 0
        upper_left = previous[i - bpp] if previous and i >= bpp else 0
        predictor = (0, left, up, (left + up) // 2, paeth(left, up, upper_left))[kind]
        result.append((value - predictor) & 255)
    return bytes([kind]) + bytes(result)


def sample_row(width: int, x0: int, y: int, dx: int, channels: int, depth: int, seed: int) -> bytes:
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


def image_data(width: int, height: int, depth: int, color: int, interlaced: bool | int, filter_kind: int, seed: int) -> bytes:
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
            data.extend(filtered(row, previous, bpp, filter_kind if filter_kind >= 0 else random.Random(seed * 7919 + y * 31 + x0).randrange(5)))
            previous = row
    return zlib.compress(bytes(data), 6)


def split_pieces(payload: bytes, rng: random.Random) -> list[bytes]:
    if rng is None or len(payload) < 2 or rng.randrange(3):
        return [payload]
    cuts = sorted(rng.sample(range(1, len(payload)), min(len(payload) - 1, rng.randrange(1, 4))))
    return [payload[a:b] for a, b in zip([0] + cuts, cuts + [len(payload)])]


def random_png(rng: random.Random) -> bytes:
    layouts = [(0, 1), (0, 2), (0, 4), (0, 8), (0, 16), (2, 8), (2, 16),
               (3, 1), (3, 2), (3, 4), (3, 8), (4, 8), (4, 16), (6, 8), (6, 16)]
    color, depth = rng.choice(layouts)
    width, height = rng.randrange(1, 28), rng.randrange(1, 24)
    interlaced = rng.randrange(2)
    seed = rng.randrange(1 << 20)
    parts = [SIGNATURE, chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, depth, color, 0, 0, interlaced))]
    for kind in rng.sample(('gAMA', 'sRGB', 'cHRM', 'iCCP', 'pHYs', 'sBIT', 'tEXt', 'bKGD'), rng.randrange(0, 4)):
        if kind == 'gAMA':
            parts.append(chunk(b'gAMA', struct.pack('>I', rng.choice((0, 45455, 100000, 22727)))))
        elif kind == 'sRGB':
            parts.append(chunk(b'sRGB', b'\x00'))
        elif kind == 'cHRM':
            parts.append(chunk(b'cHRM', struct.pack('>8I', 31270, 32900, 64000, 33000, 30000, 60000, 15000, 6000)))
        elif kind == 'iCCP':
            parts.append(chunk(b'iCCP', b'profile\x00\x00' + zlib.compress(bytes(rng.randrange(0, 200)))))
        elif kind == 'pHYs':
            parts.append(chunk(b'pHYs', struct.pack('>IIB', 2835, 2835, 1)))
        elif kind == 'sBIT':
            if color == 2 and depth == 8:
                parts.append(chunk(b'sBIT', bytes(rng.choice(((5, 6, 5), (8, 8, 8), (5, 5, 5))))))
            elif color == 4 and depth == 8:
                parts.append(chunk(b'sBIT', bytes(rng.choice(((1, 8), (8, 8), (4, 8))))))
            elif color == 6:
                parts.append(chunk(b'sBIT', bytes((depth,) * 4)))
        elif kind == 'tEXt':
            parts.append(chunk(b'tEXt', b'Comment\x00hello'))
        elif kind == 'bKGD':
            pass
    if color == 3:
        count = rng.choice((1 << depth, 1 << depth, max(1, (1 << depth) // 2), rng.randrange(1, (1 << depth) + 1)))
        parts.append(chunk(b'PLTE', bytes(rng.randrange(256) for _ in range(3 * count))))
        if rng.randrange(2):
            parts.append(chunk(b'tRNS', bytes(rng.randrange(256) for _ in range(rng.randrange(0, count + 3)))))
    elif color == 0 and rng.randrange(2):
        parts.append(chunk(b'tRNS', struct.pack('>H', rng.randrange(1 << depth))))
    elif color == 2 and rng.randrange(2):
        parts.append(chunk(b'tRNS', struct.pack('>HHH', *(rng.randrange(1 << depth) for _ in range(3)))))
    animated = rng.randrange(2)
    default_in_animation = rng.randrange(3) != 0
    frame_count = rng.randrange(1, 5) if animated else 1
    if animated:
        parts.append(chunk(b'acTL', struct.pack('>II', frame_count if default_in_animation else frame_count, rng.choice((0, 1, 2, 5)))))
    sequence = 0

    def control(w: int, h: int, x: int, y: int) -> bytes:
        nonlocal sequence
        data = struct.pack('>IIIIIHHBB', sequence, w, h, x, y, rng.randrange(0, 50), rng.choice((0, 10, 100, 1000)), rng.randrange(3), rng.randrange(2))
        sequence += 1
        return chunk(b'fcTL', data)

    filter_kind = rng.choice((-1, -1, 0, 1, 2, 3, 4))
    if animated and default_in_animation:
        parts.append(control(width, height, 0, 0))
    parts.extend(chunk(b'IDAT', piece) for piece in split_pieces(image_data(width, height, depth, color, interlaced, filter_kind, seed), rng))
    for _ in range(frame_count - 1 if default_in_animation else frame_count if animated else 0):
        fw, fh = rng.randrange(1, width + 1), rng.randrange(1, height + 1)
        x, y = rng.randrange(0, width - fw + 1), rng.randrange(0, height - fh + 1)
        if rng.randrange(3) == 0:
            fw, fh, x, y = width, height, 0, 0
        parts.append(control(fw, fh, x, y))
        data = image_data(fw, fh, depth, color, interlaced, filter_kind, seed + sequence)
        for piece in split_pieces(data, rng):
            parts.append(chunk(b'fdAT', struct.pack('>I', sequence) + piece))
            sequence += 1
    parts.append(chunk(b'IEND', b''))
    return b''.join(parts)


def make_png(width: int, height: int, depth: int, color: int, interlaced: bool | int, filter_kind: int, seed: int, color_chunk: str | None = None, animated: bool = False) -> bytes:
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


def mutate(data: bytes, rng: random.Random) -> bytes:
    data = bytearray(data)
    for _ in range(rng.choice((0, 0, 1, 1, 2, 3))):
        kind = rng.randrange(7)
        if kind == 0:
            del data[rng.randrange(len(data) + 1):]
        elif kind == 1 and data:
            data[rng.randrange(len(data))] ^= 1 << rng.randrange(8)
        elif kind == 2 and data:
            position = rng.randrange(len(data))
            del data[position:position + rng.randrange(1, 9)]
        elif kind == 3:
            position = rng.randrange(len(data) + 1)
            data[position:position] = rng.randbytes(rng.randrange(1, 9))
        elif kind == 4 and len(data) > 8:
            position = rng.randrange(8, len(data))
            data[position] = rng.randrange(256)
        elif kind == 5 and len(data) > 40:
            position = rng.randrange(8, len(data) - 12)
            span = rng.randrange(1, 40)
            data[position:position + span] = data[position + 12 + span:position + 12 + 2 * span]
        elif kind == 6 and len(data) > 16:
            position = rng.randrange(8, min(33, len(data)))
            data[position] = rng.choice((0, 1, 2, 3, 4, 5, 6, 8, 16, 255))
    return bytes(data)


def decode_request(data: bytes, cuts: list[int]) -> bytes:
    chunks = []
    start = 0
    for end in cuts:
        end = min(max(start, end), len(data))
        chunks.append(data[start:end])
        start = end
    chunks.append(data[start:])
    return b'\x00' + struct.pack('>I', len(chunks)) + b''.join(struct.pack('>I', len(part)) + part for part in chunks)


def encode_request(width: int, height: int, operation: int, seed: int, alpha_kind: int) -> bytes:
    rng = random.Random(seed)
    pixels = bytearray()
    for _ in range(width * height):
        alpha = 255 if alpha_kind == 1 else rng.randrange(256)
        limit = alpha if alpha_kind == 0 else 255
        pixels.extend((rng.randrange(limit + 1), rng.randrange(limit + 1), rng.randrange(limit + 1), alpha))
    return b'\x01' + struct.pack('>III', width, height, operation * 3 + alpha_kind) + pixels
