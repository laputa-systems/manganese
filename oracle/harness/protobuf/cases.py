import random


def varint(value):
    data = bytearray()
    while value > 127:
        data.append((value & 127) | 128)
        value >>= 7
    data.append(value)
    return bytes(data)


def field(tag, wire, value):
    return varint(tag * 8 + wire) + value


def packed(tag, value):
    return field(tag, 2, varint(len(value)) + value)


def unknown(rng, depth=0):
    tag = rng.choice([7, 9, 17, 50, 127, 128, 4096])
    wire = rng.choice([0, 1, 2, 3, 5] if depth < 3 else [0, 1, 2, 5])
    if wire == 0:
        value = varint(rng.choice([0, 1, 127, 128, 2**32 - 1, 2**64 - 1, rng.getrandbits(64)]))
    elif wire == 1:
        value = rng.randbytes(8)
    elif wire == 2:
        value = rng.randbytes(rng.randrange(20))
        value = varint(len(value)) + value
    elif wire == 3:
        value = b''.join(unknown(rng, depth + 1) for _ in range(rng.randrange(3))) + varint(tag * 8 + 4)
    else:
        value = rng.randbytes(4)
    return field(tag, wire, value)


def origin(rng):
    parts = []
    for _ in range(rng.randrange(8)):
        tag = rng.choice([1, 2, 3, 7])
        if tag in (1, 2):
            parts.append(packed(tag, rng.randbytes(rng.randrange(24))))
        elif tag == 3:
            parts.append(field(tag, 0, varint(rng.getrandbits(40))))
        else:
            parts.append(unknown(rng))
    return b''.join(parts)


def token(rng):
    parts = []
    for _ in range(rng.randrange(12)):
        tag = rng.choice([1, 2, 3, 4, 5, 6, 9])
        if tag in (1, 4, 5):
            parts.append(packed(tag, rng.randbytes(rng.randrange(30))))
        elif tag in (2, 3, 6):
            parts.append(field(tag, 0, varint(rng.getrandbits(rng.choice([1, 7, 32, 64])))))
        else:
            parts.append(unknown(rng))
    return b''.join(parts)


def registration(rng: random.Random) -> bytes:
    parts = []
    if rng.randrange(4):
        parts = [packed(2, rng.randbytes(rng.randrange(12))), field(7, 0, varint(rng.getrandbits(32))), field(8, 0, varint(rng.getrandbits(64)))]
    for _ in range(rng.randrange(6)):
        tag = rng.choice([2, 7, 8, 9, 10, 11])
        if tag == 2:
            parts.append(packed(tag, rng.randbytes(rng.randrange(16))))
        elif tag == 9:
            options = field(1, 0, varint(rng.getrandbits(64))) + (unknown(rng) if rng.randrange(2) else b'')
            parts.append(packed(tag, options))
        elif tag == 11:
            parts.append(unknown(rng))
        else:
            parts.append(field(tag, 0, varint(rng.getrandbits(rng.choice([1, 32, 64])))))
    rng.shuffle(parts)
    return b''.join(parts)


def payment_image(rng: random.Random) -> bytes:
    parts = []
    for _ in range(rng.randrange(7)):
        tag = rng.choice([1, 2, 3, 9])
        if tag in (1, 2):
            parts.append(packed(tag, rng.randbytes(rng.randrange(20))))
        elif tag == 3:
            size = field(rng.choice([1, 2]), 0, varint(rng.getrandbits(64))) + (unknown(rng) if rng.randrange(2) else b'')
            parts.append(packed(tag, size))
        else:
            parts.append(unknown(rng))
    return b''.join(parts)


def generate(rng):
    kind = rng.randrange(4)
    parts = []
    if kind == 2 and rng.randrange(4):
        parts.append(packed(3, rng.randbytes(rng.randrange(16))))
    for _ in range(rng.randrange(12)):
        if kind == 0:
            tag = rng.choice([1, 3, 4, 8])
            if tag == 8:
                parts.append(unknown(rng))
            else:
                value = rng.choice([0, 1, 4, 5, 127, 128, 2**32 - 1, 2**64 - 1, rng.getrandbits(64)])
                parts.append(field(tag, 0, varint(value)))
        elif kind == 1:
            tag = rng.choice([1, 1, 2, 2, 9])
            parts.append(packed(tag, origin(rng) if tag == 1 else token(rng)) if tag != 9 else unknown(rng))
        elif kind == 2:
            tag = rng.choice([1, 1, 3, 9])
            parts.append(packed(tag, registration(rng) if tag == 1 else rng.randbytes(rng.randrange(16))) if tag != 9 else unknown(rng))
        else:
            tag = rng.choice([1, 2, 3, 4, 5, 6, 7, 8, 8, 9])
            if tag == 1:
                parts.append(field(tag, 0, varint(rng.getrandbits(64))))
            elif tag == 6:
                parts.append(packed(tag, payment_image(rng)))
            elif tag == 8:
                values = [varint(rng.getrandbits(rng.choice([1, 32, 64]))) for _ in range(rng.randrange(5))]
                parts.append(packed(tag, b''.join(values)) if rng.randrange(2) else b''.join(field(tag, 0, value) for value in values))
            elif tag == 9:
                parts.append(unknown(rng))
            else:
                parts.append(packed(tag, rng.randbytes(rng.randrange(20))))
    data = bytes([kind]) + b''.join(parts)
    mode = rng.randrange(10)
    if mode == 0 and len(data) > 1:
        data = data[:rng.randrange(1, len(data))]
    elif mode == 1:
        data += rng.choice([b'\x80', b'\x00', b'\xff' * 11, b'\x0a\xff\xff\xff\xff\x0f', b'\x0c'])
    elif mode == 2 and len(data) > 1:
        pos = rng.randrange(1, len(data))
        data = data[:pos] + bytes([data[pos] ^ (1 << rng.randrange(8))]) + data[pos + 1:]
    elif mode == 3:
        data += field(rng.choice([1, 3, 4]), 5, rng.randbytes(4))
    elif mode == 4:
        data += field(rng.choice([1, 3, 4, 11]), 0, b'\x80\x00')
    return data
