import random


def u32(value):
    return value.to_bytes(4, 'big')


def compress_request(data):
    return b'\x00' + u32(len(data)) + data


def decode_request(stream, limit):
    return b'\x01' + u32(limit) + u32(len(stream)) + stream


LENGTHS = (1, 2, 7, 15, 16, 17, 31, 60, 61, 64, 65, 127, 255, 256, 257,
           1023, 1024, 1025, 4095, 4096, 4097, 16384, 65535, 65536, 65537, 131072)
KINDS = ('zeros', 'repeat', 'random', 'text', 'binary')


def generate_data(rng, lane, index, length, kind):
    tag = ord('S') if lane == 'seen' else ord('H')
    if kind == 'zeros':
        data = bytearray(length)
        data[0] = tag
    elif kind == 'repeat':
        pattern = bytes([tag]) + bytes(rng.randrange(1, 256) for _ in range(rng.randrange(2, 20)))
        data = bytearray((pattern * (length // len(pattern) + 1))[:length])
    elif kind == 'random':
        data = bytearray(rng.randbytes(length))
        data[0] = tag
    elif kind == 'text':
        pattern = ('text-' + lane + '-' + str(index) + '-').encode()
        data = bytearray((pattern * (length // len(pattern) + 1))[:length])
    else:
        data = bytearray((bytes(range(256)) * (length // 256 + 1))[:length])
        data[0] = tag
    return bytes(data)


def invalid_streams(lane):
    tag = 0x53 if lane == 'seen' else 0x48
    return [
        bytes([tag]) + b'\x80\x80\x80\x80',
        b'\x0a',
        b'\x04\x10' + bytes([tag]),
        b'\x08\x01\x00',
        b'\x01\x01\x00',
        b'\x03' + bytes([tag]) + b'\x41\x42\x43',
        b'\x00' + bytes([tag]),
        b'\xff\xff\xff\xff\x0f\x00',
        b'\x80\x80\x80\x80\x07\x00',
        b'\x02\x01' + bytes([tag]) + b'\x00',
    ]


def cases(lane, seed, compress):
    rng = random.Random(seed)
    if lane == 'seen':
        yield 'empty', compress_request(b'')
        stream = compress(b'')
        yield 'empty-roundtrip', decode_request(stream, 0)
        yield 'empty-limit-plus', decode_request(stream, 1)
    index = 0
    for length in LENGTHS:
        for kind in KINDS:
            data = generate_data(rng, lane, index, length, kind)
            label = f'{kind}-{length}-{index}'
            index += 1
            yield label, compress_request(data)
            stream = compress(data)
            yield label + '-roundtrip', decode_request(stream, length)
            yield label + '-limit-plus', decode_request(stream, length + 1)
            yield label + '-limit-minus', decode_request(stream, length - 1)
    for number, stream in enumerate(invalid_streams(lane)):
        yield f'invalid-{number}', decode_request(stream, 1 << 16)
    mutated = bytearray(compress(generate_data(rng, lane, 9999, 512, 'repeat')))
    for number in range(12):
        candidate_stream = bytearray(mutated)
        offset = rng.randrange(len(candidate_stream))
        candidate_stream[offset] ^= 1 << rng.randrange(8)
        yield f'mutated-{number}', decode_request(bytes(candidate_stream), 1 << 16)
