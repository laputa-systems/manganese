import random
import struct

SIZES = [(1, 1), (2, 3), (8, 8), (9, 7), (16, 16), (17, 13),
         (64, 63), (65, 17), (128, 128), (129, 97)]
QUALITIES = [1, 25, 50, 75, 80, 90, 92, 95, 99, 100, 0, 101]
PATTERNS = ['flat', 'ramp-h', 'ramp-v', 'checker', 'edges', 'noise',
            'alpha-gradient', 'zero-alpha']


def pixels(width, height, seed, pattern):
    pattern_seed = PATTERNS.index(pattern) if pattern in PATTERNS else 0
    rng = random.Random(seed * 2654435761 + pattern_seed * 104729)
    result = bytearray()
    for y in range(height):
        for x in range(width):
            if pattern == 'flat':
                r, g, b, a = seed & 255, (seed >> 8) & 255, (seed >> 16) & 255, 255
            elif pattern == 'ramp-h':
                r, g, b, a = x * 255 // max(1, width - 1), (x * 7 + seed) & 255, (x * 3 + 11) & 255, 255
            elif pattern == 'ramp-v':
                r, g, b, a = (y * 13 + seed) & 255, y * 255 // max(1, height - 1), (y * 29 + 3) & 255, 255
            elif pattern == 'checker':
                v = 255 if ((x // 4 + y // 4) & 1) else 0
                r, g, b, a = v, 255 - v, (x * y + seed) & 255, 255
            elif pattern == 'edges':
                r, g, b, a = (255 if x in (0, width - 1) else 16), (255 if y in (0, height - 1) else 32), seed & 255, 255
            elif pattern == 'noise':
                r, g, b, a = (rng.randrange(256) for _ in range(4))
            elif pattern == 'alpha-gradient':
                a = y * 255 // max(1, height - 1)
                r, g, b = (x * 255 // max(1, width - 1)), (a * 3 // 4), (255 - a + seed) & 255
            else:
                r, g, b, a = rng.randrange(256), rng.randrange(256), rng.randrange(256), 0
            result.extend((b & 255, g & 255, r & 255, a & 255))
    return bytes(result)


def encode_request(width, height, quality, seed, pattern='noise'):
    return (b'\x01' + struct.pack('>III', width, height, quality) +
            pixels(width, height, seed, pattern))


def mutate(data, rng):
    if len(data) < 13:
        return data
    result = bytearray(data)
    kind = rng.randrange(6)
    if kind == 0:
        struct.pack_into('>I', result, 9, rng.choice(QUALITIES + [rng.randrange(256)]))
    elif kind == 1:
        struct.pack_into('>I', result, 1, rng.choice((1, 2, 3, 8, 9, 16, 17, 31, 64, 65)))
    elif kind == 2:
        struct.pack_into('>I', result, 5, rng.choice((1, 2, 3, 8, 9, 16, 17, 31, 64, 65)))
    elif kind == 3:
        for _ in range(rng.randrange(1, 12)):
            result[13 + rng.randrange(len(result) - 13)] ^= 1 << rng.randrange(8)
    elif kind == 4:
        start = 13 + rng.randrange(max(1, len(result) - 13 - 16))
        span = rng.randrange(1, min(32, len(result) - start) + 1)
        chunk = bytes(result[start:start + span])
        result[start:start + span] = bytes(result[start + 8:start + 8 + span]) if start + 8 + span <= len(result) else chunk[::-1]
    else:
        for _ in range(rng.randrange(1, 8)):
            index = 13 + rng.randrange(len(result) - 13)
            result[index] = rng.randrange(256)
    # Keep the record self-consistent: resize the pixel payload to width*height*4.
    width, height, _ = struct.unpack_from('>III', result, 1)
    need = 13 + width * height * 4
    if width == 0 or height == 0 or width > 256 or height > 256:
        width = max(1, min(256, width))
        height = max(1, min(256, height))
        struct.pack_into('>II', result, 1, width, height)
        need = 13 + width * height * 4
    if len(result) < need:
        result.extend(bytes(need - len(result)))
    elif len(result) > need:
        del result[need:]
    return bytes(result)


def random_request(rng):
    width = rng.choice((1, 2, 3, 7, 8, 9, 15, 16, 17, 31, 64, 65, 128))
    height = rng.choice((1, 2, 3, 7, 8, 9, 15, 16, 17, 31, 64, 65, 128))
    quality = rng.choice(QUALITIES)
    pattern = rng.choice(PATTERNS)
    return encode_request(width, height, quality, rng.randrange(1 << 30), pattern)
