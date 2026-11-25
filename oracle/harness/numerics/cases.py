import random
import struct


def generate_base(rng: random.Random) -> bytes:
    op = rng.choice((0, 1, 2, 3, 4, 5, 6, 7, 8, 9))
    if op < 5:
        bits = rng.choice((rng.getrandbits(64), rng.getrandbits(64), rng.choice((0, 1, 0x8000000000000000, 0x7ff0000000000000, 0xfff0000000000000, 0x7ff8000000000000, 0x7fefffffffffffff, 0x0010000000000000))))
        digits = rng.randrange(101) if op == 1 else rng.randrange(1, 121) if op == 3 else rng.randrange(0, 122) if op == 4 else 1
        return struct.pack('>BQH', op, bits, digits + 1 if op in (1, 3) else digits)
    if op == 6:
        units = []
        for _ in range(rng.randrange(64)):
            if rng.randrange(5) == 0:
                units.extend((rng.randrange(0xd800, 0xdc00), rng.randrange(0xdc00, 0xe000)))
            else:
                units.append(rng.randrange(65536))
        return b'\x06' + b''.join(struct.pack('>H', unit) for unit in units)
    if op == 9:
        if rng.randrange(2):
            return b'\x09' + bytes(rng.randrange(256) for _ in range(rng.randrange(80)))
        return b'\x09' + ''.join(chr(rng.choice((rng.randrange(0xd800), rng.randrange(0xe000, 0x110000)))) for _ in range(rng.randrange(25))).encode()
    alphabet = b'0123456789abcdefABCDEF+-xX.eE \t\n!'
    if rng.randrange(2):
        value = str(rng.randrange(-(1 << 70), 1 << 70)).encode()
        data = rng.choice((b'', b' ', b'+', b'-')) + value + rng.choice((b'', b'', b'.12', b'e-4', b'x'))
    else:
        data = bytes(rng.choice(alphabet) for _ in range(rng.randrange(45)))
    return bytes((op, rng.randrange(8))) + data if op == 7 else bytes((op,)) + data


def generate_blink(rng: random.Random) -> bytes:
    op = rng.choice((10, 11))
    bits = rng.getrandbits(64)
    digits = rng.randrange(1, 21) if op == 10 else rng.randrange(21)
    return struct.pack('>BQH', op, bits, digits)


def generate_json(rng: random.Random) -> bytes:
    number = str(rng.randrange(-(1 << 80), 1 << 80)).encode()
    if rng.randrange(3) == 0: number += b'.' + str(rng.randrange(10**18)).encode()
    if rng.randrange(3) == 0: number += b'e' + str(rng.randrange(-400, 401)).encode()
    if rng.randrange(3) == 0:
        alphabet = b'0123456789+-.eE x\t\n'
        number = bytes(rng.choice(alphabet) for _ in range(rng.randrange(60)))
    return b'\x0c' + rng.choice((b'', b' ', b'\t')) + number + rng.choice((b'', b'', b' ', b'!'))


def generate_json_writer(rng: random.Random) -> bytes:
    bits = rng.choice((rng.getrandbits(64), rng.choice((0, 1, 0x8000000000000000, 0x7ff0000000000000, 0xfff0000000000000, 0x7ff8000000000000, 0x7fefffffffffffff))))
    return struct.pack('>BQBB', 13, bits, rng.randrange(2), 0)


def generate_blink_wide(rng: random.Random) -> bytes:
    alphabet = b'0123456789+-.eE x\t\n'
    if rng.randrange(2):
        units = list(str(rng.randrange(-(1 << 60), 1 << 60)).encode())
        units += [rng.randrange(128, 65536)] if rng.randrange(2) else [ord('e'), ord('-'), ord('4')]
    else:
        units = [rng.choice(alphabet) if rng.randrange(4) else rng.randrange(128, 65536) for _ in range(rng.randrange(130))]
    return b'\x0e' + b''.join(struct.pack('>H', unit) for unit in units)


def generate_format(rng: random.Random) -> bytes:
    bits = rng.choice((rng.getrandbits(64), rng.choice((0, 1, 0x8000000000000000, 0x7ff0000000000000, 0xfff0000000000000, 0xfff8000000000000))))
    if rng.randrange(2): return struct.pack('>BQBB', 15, bits, 0, 0)
    return struct.pack('>BQBB', 16, bits, rng.randrange(4), rng.randrange(21))


def generate_base_utf(rng: random.Random) -> bytes:
    if rng.randrange(2):
        if rng.randrange(2): data = rng.randbytes(rng.randrange(100))
        else: data = ''.join(chr(rng.choice((rng.randrange(0xd800), rng.randrange(0xe000, 0x110000)))) for _ in range(rng.randrange(30))).encode()
        return b'\x11' + data
    units = [rng.randrange(65536) for _ in range(rng.randrange(100))]
    return b'\x12' + b''.join(struct.pack('>H', unit) for unit in units)


def generate_wide(rng: random.Random) -> bytes:
    op = rng.randrange(19, 23)
    if op == 19: data = rng.randbytes(rng.randrange(70))
    elif op in (20, 21):
        values = [rng.choice((rng.randrange(0x110000), rng.getrandbits(32), 0xd800, 0x10ffff, 0x110000)) for _ in range(rng.randrange(40))]
        data = b''.join(struct.pack('>I', value) for value in values)
    else: data = b''.join(struct.pack('>H', rng.randrange(65536)) for _ in range(rng.randrange(70)))
    return bytes([op]) + data


def generate_int_format(rng: random.Random) -> bytes:
    return struct.pack('>BBQ', rng.randrange(23, 26), rng.randrange(4), rng.getrandbits(64))


def generate_wide_number(rng: random.Random) -> bytes:
    alphabet = [*b'0123456789abcdefABCDEFxX+-.eE !\t', 0x85, 0xa0, 0x1680, 0x180e, 0x2000, 0x2028, 0x205f, 0x3000, 0xfeff, 0xd800]
    if rng.randrange(2): units = list(str(rng.randrange(-(1 << 70), 1 << 70)).encode()) + [rng.choice(alphabet) for _ in range(rng.randrange(5))]
    else: units = [rng.choice(alphabet) for _ in range(rng.randrange(35))]
    data = b''.join(struct.pack('>H', unit) for unit in units)
    return b'\x1a' + data if rng.randrange(2) else b'\x1b' + bytes([rng.randrange(8)]) + data


def generate(rng: random.Random) -> bytes:
    selected = rng.randrange(14)
    if selected == 0: return generate_blink(rng)
    if selected == 1: return generate_json(rng)
    if selected == 2: return generate_json_writer(rng)
    if selected == 3: return generate_blink_wide(rng)
    if selected == 4: return generate_format(rng)
    if selected == 5: return generate_base_utf(rng)
    if selected == 6: return generate_wide(rng)
    if selected == 7: return generate_int_format(rng)
    if selected == 8: return generate_wide_number(rng)
    return generate_base(rng)
