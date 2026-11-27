import random
import struct


def make_palette(count: int, seed: int) -> bytes:
    out = bytearray()
    for index in range(count):
        out += bytes(((index * 37 + seed * 11) & 255, (index * 61 + seed * 7) & 255,
                      (index * 97 + seed * 3) & 255))
    return bytes(out)


def lzw(indices: bytes, minimum: int) -> bytes:
    clear = 1 << minimum
    end = clear + 1
    width = minimum + 1
    bits = []
    code = 0
    bit_count = 0

    def emit(value: int) -> None:
        nonlocal bits, code, bit_count
        for offset in range(width):
            if (value >> offset) & 1:
                code |= 1 << bit_count
            bit_count += 1
            if bit_count == 8:
                bits.append(code)
                code = 0
                bit_count = 0

    emit(clear)
    save = clear + 1
    for index in indices:
        emit(index)
        save += 1
        if width < 12 and (save >> width) & 1:
            width += 1
    emit(end)
    if bit_count:
        bits.append(code)
    return bytes(bits)


def sub_blocks(payload: bytes, block_size: int = 255) -> bytes:
    out = bytearray()
    for start in range(0, len(payload), block_size):
        chunk = payload[start:start + block_size]
        out.append(len(chunk))
        out += chunk
    out.append(0)
    return bytes(out)


def interlace_rows(height: int):
    row = 0
    for start, step in ((0, 8), (4, 8), (2, 4), (1, 2)):
        while start < height:
            yield start
            start += step


def frame_pixels(width: int, height: int, palette_size: int, seed: int, interlace: bool) -> bytes:
    rows = list(interlace_rows(height)) if interlace else range(height)
    out = bytearray()
    for y in rows:
        for x in range(width):
            out.append((x * 7 + y * 13 + seed * 29 + (x * y) // 3) % palette_size)
    return bytes(out)


def make_gif(width: int, height: int, colors: int, seed: int, frames: int = 1, *,
             interlace: bool = False, loop: int | None = None, transparency: bool = False,
             local_palette: bool = False, disposal: int = 0, delays: tuple[int, ...] = (10,),
             offsets: tuple[tuple[int, int], ...] = ((0, 0),), sizes: tuple[tuple[int, int], ...] = (),
             block_size: int = 255, terminate: bool = True) -> bytes:
    bits = max(1, (colors - 1).bit_length())
    count = 1 << bits
    out = bytearray(b'GIF89a')
    out += struct.pack('<HHBBB', width, height, 0x80 | (bits - 1), 0, 0)
    out += make_palette(count, seed)
    if loop is not None:
        out += b'\x21\xff\x0bNETSCAPE2.0\x03\x01' + struct.pack('<H', loop) + b'\x00'
    for index in range(frames):
        delay = delays[min(index, len(delays) - 1)]
        transparent = transparency and index % 2 == 0
        packed = (disposal & 7) << 2 | (1 if transparent else 0)
        out += b'\x21\xf9\x04' + bytes((packed,)) + struct.pack('<H', delay)
        out += bytes((index % count if transparent else 0, 0))
        x, y = offsets[min(index, len(offsets) - 1)]
        fw, fh = sizes[index] if sizes else (width, height)
        flags = (0x80 | ((bits - 1) & 7) if local_palette else 0) | (0x40 if interlace else 0)
        out += b'\x2c' + struct.pack('<HHHHB', x, y, fw, fh, flags)
        if local_palette:
            out += make_palette(count, seed + 53 + index)
        paint_width = min(x + fw, width) - x
        paint_height = min(y + fh, height) - y
        if index == 0:
            paint_width = fw
            paint_height = fh
        minimum = max(1, (count - 1).bit_length())
        out += bytes((minimum,))
        out += sub_blocks(lzw(frame_pixels(paint_width, paint_height, count, seed + index, interlace), minimum), block_size)
    if terminate:
        out += b'\x3b'
    return bytes(out)


def decode_request(data: bytes, cuts: list[int] | tuple[int, ...] = ()) -> bytes:
    pieces = []
    last = 0
    for cut in sorted(cuts):
        cut = max(last, min(len(data), int(cut)))
        pieces.append(data[last:cut])
        last = cut
    pieces.append(data[last:])
    pieces = [piece for piece in pieces if piece]
    if not pieces:
        pieces = [b'']
    return b'\x00' + struct.pack('>I', len(pieces)) + b''.join(struct.pack('>I', len(piece)) + piece for piece in pieces)


def mutate(data: bytes, rng: random.Random) -> bytes:
    if len(data) < 14 or data[10] & 0x80 == 0:
        return data
    count = 1 << ((data[10] & 7) + 1)
    result = bytearray(data)
    for _ in range(rng.randrange(1, 4)):
        position = 13 + rng.randrange(3 * count)
        result[position] ^= 1 << rng.randrange(8)
    return bytes(result)


def random_gif(rng: random.Random) -> bytes:
    colors = rng.choice((2, 4, 8, 16, 32, 256))
    width, height = rng.randrange(1, 28), rng.randrange(1, 24)
    frames = 1
    return make_gif(width, height, colors, rng.randrange(1 << 20), frames,
                    interlace=bool(rng.randrange(2)),
                    loop=rng.choice((None, 0, 1, 2, 7)),
                    transparency=bool(rng.randrange(2)),
                    local_palette=bool(rng.randrange(2)),
                    disposal=rng.randrange(5),
                    delays=tuple(rng.choice((0, 1, 10, 100, 65535)) for _ in range(frames)),
                    offsets=tuple((rng.randrange(0, 4), rng.randrange(0, 4)) for _ in range(frames)),
                    block_size=rng.choice((1, 2, 7, 255)))
