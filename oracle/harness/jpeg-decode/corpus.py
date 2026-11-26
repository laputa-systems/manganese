import pathlib
import random
import struct
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[3]
ENCODER = ROOT / '.work/tmp/jpeg-decode/encoder'
FIXTURES = ROOT / '.work/tmp/jpeg-decode/fixtures'

UNKNOWN, GRAYSCALE, RGB, YCBCR, CMYK, YCCK = range(6)
ISLOW, IFAST, FLOAT = range(3)
COMPONENTS = {GRAYSCALE: 1, RGB: 3, YCBCR: 3, CMYK: 4, YCCK: 4}


def u32(*values):
    return b''.join(struct.pack('>I', value & 0xffffffff) for value in values)


def segment(marker, payload):
    return bytes((0xFF, marker)) + struct.pack('>H', len(payload) + 2) + payload


def pixels(width, height, components, seed, precision=8, kind=None):
    rng = random.Random(seed)
    kind = rng.randrange(4) if kind is None else kind
    top = (1 << precision) - 1
    noise = [rng.randrange(top + 1) for _ in range(257)]
    phases = [rng.randrange(64) for _ in range(components)]
    data = bytearray()
    for y in range(height):
        for x in range(width):
            for c in range(components):
                if kind == 0:
                    value = ((x + phases[c]) * top // max(1, width - 1 + phases[c]) + (y * 3 + c * 17)) % (top + 1)
                elif kind == 1:
                    value = noise[(x * 7 + y * 13 + c * 29 + seed) % 257]
                elif kind == 2:
                    value = top if ((x // (2 + c)) + (y // 3)) % 2 else noise[(x + c) % 257] // 4
                else:
                    value = (x * x + y * 5 + c * 50 + phases[c] * 3) % (top + 1) if (x + y) % 5 else noise[(x * y + c) % 257]
                if precision > 8:
                    data += struct.pack('>H', value)
                else:
                    data.append(value)
    return bytes(data)


def encode(width, height, pixels_data, *, components=3, in_space=RGB, jpeg_space=YCBCR, precision=8,
           quality=75, optimize=False, progressive=False, smoothing=0, restart=0, restart_rows=0,
           dct=ISLOW, jfif=None, adobe=None, predictor=0, point_transform=0, sampling=None,
           quant=None, scans=(), markers=(), icc=b''):
    count = COMPONENTS.get(jpeg_space, components)
    if sampling is None:
        sampling = [(2, 2) if jpeg_space in (YCBCR, YCCK) and index in (0, 3) else (1, 1) for index in range(count)]
        if jpeg_space not in (YCBCR, YCCK):
            sampling = [(1, 1)] * count
    if quant is None:
        quant = [0 if index in (0, 3) or jpeg_space not in (YCBCR, YCCK) else 1 for index in range(count)]
    if jfif is None:
        jfif = jpeg_space in (GRAYSCALE, YCBCR)
    if adobe is None:
        adobe = jpeg_space in (RGB, CMYK, YCCK)
    body = u32(width, height, components, in_space, jpeg_space, precision, quality, int(optimize), int(progressive),
               smoothing, restart, restart_rows, dct, int(jfif), int(adobe), predictor, point_transform)
    for (h, v), table in zip(sampling, quant):
        body += u32(h, v, table)
    body += u32(len(scans))
    for scan in scans:
        indexes, ss, se, ah, al = scan
        body += u32(len(indexes), *(list(indexes) + [0] * (4 - len(indexes))), ss, se, ah, al)
    body += u32(len(markers))
    for code, payload in markers:
        body += u32(code, len(payload)) + payload
    body += u32(len(icc)) + icc + pixels_data
    result = subprocess.run([str(ENCODER)], input=b'JENC' + body, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise RuntimeError('encoder failed: ' + result.stderr.decode(errors='replace'))
    return result.stdout


def image(width, height, seed, jpeg_space=YCBCR, precision=8, kind=None, **params):
    if jpeg_space == UNKNOWN:
        components = params.pop('components')
        in_space = UNKNOWN
    else:
        components = COMPONENTS[jpeg_space]
        in_space = {GRAYSCALE: GRAYSCALE, RGB: RGB, YCBCR: RGB, CMYK: CMYK, YCCK: CMYK}[jpeg_space]
        params.pop('components', None)
    data = pixels(width, height, components, seed, precision, kind)
    return encode(width, height, data, components=components, in_space=in_space, jpeg_space=jpeg_space,
                  precision=precision, **params)


def tiff_entry(tag, kind, count, value, little):
    order = '<' if little else '>'
    if kind == 3 and count == 1:
        field = struct.pack(order + 'HH', value, 0)
    else:
        field = struct.pack(order + 'I', value)
    return struct.pack(order + 'HHI', tag, kind, count) + field


def exif(orientation=None, little=False, resolution=None, unit=None, dimensions=None, raw_entries=()):
    order = '<' if little else '>'
    entries = []
    extra = b''
    ifd0 = []
    if orientation is not None:
        ifd0.append((0x0112, 3, 1, orientation))
    if unit is not None:
        ifd0.append((0x0128, 3, 1, unit))
    rationals = []
    if resolution is not None:
        for tag, (numerator, denominator) in zip((0x011A, 0x011B), resolution):
            rationals.append((tag, numerator, denominator))
    ifd0.extend(raw_entries)
    count = len(ifd0) + len(rationals) + (1 if dimensions else 0)
    base = 8 + 2 + 12 * count + 4
    offset = base
    for tag, numerator, denominator in rationals:
        ifd0.append((tag, 5, 1, offset))
        extra += struct.pack(order + 'II', numerator, denominator)
        offset += 8
    sub = b''
    if dimensions:
        ifd0.append((0x8769, 4, 1, offset))
        sub_entries = [(0xA002, 4, 1, dimensions[0]), (0xA003, 4, 1, dimensions[1])]
        sub = struct.pack(order + 'H', len(sub_entries)) + b''.join(tiff_entry(*e, little) for e in sub_entries) + struct.pack(order + 'I', 0)
    ifd0.sort(key=lambda entry: entry[0])
    entries = b''.join(tiff_entry(*entry, little) for entry in ifd0)
    tiff = (b'II*\0' if little else b'MM\0*') + struct.pack(order + 'I', 8) + struct.pack(order + 'H', len(ifd0)) + entries + struct.pack(order + 'I', 0) + extra + sub
    return b'Exif\0\0' + tiff


def icc_profile(size, seed):
    rng = random.Random(seed)
    body = bytes(rng.randrange(256) for _ in range(max(0, size - 40)))
    return struct.pack('>I', size) + b'lcms' + b'\x04\x30\0\0' + b'mntrRGB XYZ ' + bytes(12) + b'acsp' + b'APPL' + body[:max(0, size - 40)]


def icc_segments(profile, parts, order=None, count=None, extra=()):
    size = -(-len(profile) // parts) if parts else 0
    pieces = [profile[i * size:(i + 1) * size] for i in range(parts)]
    indexes = list(range(1, parts + 1)) if order is None else order
    total = parts if count is None else count
    return [(0xE2, b'ICC_PROFILE\0' + bytes((index, total)) + pieces[(index - 1) % parts]) for index in indexes] + list(extra)


def insert_after_soi(data, segments):
    return data[:2] + b''.join(segment(marker, payload) for marker, payload in segments) + data[2:]


def markers(data):
    found = []
    position = 2
    while position + 4 <= len(data) and data[position] == 0xFF:
        marker = data[position + 1]
        if marker == 0xDA or marker == 0xD9:
            found.append((marker, position, len(data) - position))
            break
        length = struct.unpack('>H', data[position + 2:position + 4])[0]
        found.append((marker, position, length + 2))
        position += length + 2
    return found


def request(data, cuts=()):
    bounds = [0] + sorted(cuts) + [len(data)]
    chunks = [data[start:end] for start, end in zip(bounds, bounds[1:])]
    return b'\0' + u32(len(chunks)) + b''.join(u32(len(chunk)) + chunk for chunk in chunks)


def request_data(record):
    count = struct.unpack_from('>I', record, 1)[0]
    offset = 5
    data = b''
    for _ in range(count):
        size = struct.unpack_from('>I', record, offset)[0]
        data += record[offset + 4:offset + 4 + size]
        offset += 4 + size
    return data


def mutate(data, rng):
    data = bytearray(data)
    for _ in range(rng.randrange(1, 5)):
        if not data:
            data.append(rng.randrange(256))
        choice = rng.randrange(10)
        position = rng.randrange(len(data))
        if choice < 3:
            data[position] ^= 1 << rng.randrange(8)
        elif choice == 3:
            data[position] = rng.choice((0x00, 0xFF, 0xD9, 0xD8, 0xDA, 0xC0, 0xC2, 0xC4, 0xDB, 0xDD, 0xD0 + rng.randrange(8), rng.randrange(256)))
        elif choice == 4:
            data[position:position] = bytes(rng.randrange(256) for _ in range(rng.randrange(1, 9)))
        elif choice == 5:
            del data[position:position + rng.randrange(1, 17)]
        elif choice == 6:
            del data[rng.randrange(2, len(data) + 1) if len(data) > 2 else len(data):]
        elif choice == 7:
            data[position:position] = bytes((0xFF, rng.choice((0xD0 + rng.randrange(8), 0xD9, 0x00, 0xFF, 0xE1, 0xFE, 0xDC))))
        elif choice == 8:
            found = markers(bytes(data))
            if found:
                marker, start, size = rng.choice(found)
                if size >= 4 and marker not in (0xDA, 0xD9):
                    offset = start + 2 + rng.randrange(2)
                    data[offset] = (data[offset] + rng.choice((-2, -1, 1, 2, 0x80))) & 255
        else:
            source = rng.randrange(len(data))
            length = rng.randrange(1, 33)
            data[position:position] = data[source:source + length]
    return bytes(data)


def random_cuts(length, rng, most=5):
    return sorted(rng.randrange(length + 1) for _ in range(rng.randrange(0, most + 1)))


def oversized(data, limit=1 << 22):
    position = 0
    while True:
        position = data.find(b'\xff', position)
        if position < 0 or position + 9 > len(data):
            return False
        marker = data[position + 1]
        if 0xC0 <= marker <= 0xCF and marker not in (0xC4, 0xC8, 0xCC):
            height, width = struct.unpack('>HH', data[position + 5:position + 9])
            if width * height > limit:
                return True
        position += 1


def progressive_script(rng, components):
    scans = [(tuple(range(components)), 0, 0, 0, 1)]
    for c in range(components):
        cut = rng.randrange(1, 63)
        scans.append(((c,), 1, cut, 0, 2))
        scans.append(((c,), cut + 1, 63, 0, 2))
    scans.append((tuple(range(components)), 0, 0, 1, 0))
    for c in range(components):
        scans.append(((c,), 1, 63, 2, 1))
        scans.append(((c,), 1, 63, 1, 0))
    return scans


def sequential_script(components):
    return [((c,), 0, 63, 0, 0) for c in range(components)]


def random_params(rng):
    space = rng.choice((YCBCR, YCBCR, YCBCR, GRAYSCALE, RGB, CMYK, YCCK))
    count = COMPONENTS[space]
    params = {'jpeg_space': space, 'quality': rng.choice((1, 10, 30, 50, 75, 90, 95, 100)), 'optimize': rng.random() < 0.3,
              'dct': rng.choice((ISLOW, IFAST, FLOAT)), 'smoothing': rng.choice((0, 0, 0, 20))}
    if rng.random() < 0.6:
        params['sampling'] = [(rng.randrange(1, 5), rng.randrange(1, 5)) for _ in range(count)]
        if sum(h * v for h, v in params['sampling']) > 10:
            params['sampling'] = [(rng.choice((1, 2)), rng.choice((1, 2)))] + [(1, 1)] * (count - 1)
    kind = rng.randrange(6)
    if kind == 0:
        params['progressive'] = True
    elif kind == 1:
        params['scans'] = progressive_script(rng, count)
    elif kind == 2:
        params['scans'] = sequential_script(count)
    elif kind == 3 and space in (RGB, GRAYSCALE, CMYK):
        params['predictor'] = rng.randrange(1, 8)
        params['point_transform'] = rng.randrange(0, 3)
        params['sampling'] = [(1, 1)] * count
    if rng.random() < 0.3:
        params['restart'] = rng.choice((1, 2, 3, 7))
    elif rng.random() < 0.1:
        params['restart_rows'] = 1
    if rng.random() < 0.2:
        params['markers'] = [(0xE1, exif(rng.randrange(0, 10), rng.random() < 0.5))]
    if rng.random() < 0.1:
        params['icc'] = icc_profile(rng.randrange(200, 3000), rng.randrange(1 << 30))
    if rng.random() < 0.05 and 'predictor' not in params:
        params['precision'] = 12
    return params


def random_jpeg(rng, max_side=48):
    while True:
        side = rng.choice((max_side, max_side, 160, 400))
        params = random_params(rng)
        width, height = rng.randrange(1, side), rng.randrange(1, side)
        if params.get('predictor') and params.get('restart'):
            params['restart'] = 0
        try:
            return image(width, height, rng.randrange(1 << 30), **params)
        except RuntimeError:
            continue


SAMPLINGS = [(1, 1), (2, 1), (1, 2), (2, 2), (4, 1), (1, 4), (4, 2), (2, 4), (3, 1), (1, 3), (3, 3), (4, 4), (3, 2)]


def replace_marker(data, marker, payload):
    for code, start, size in markers(data):
        if code == marker:
            return data[:start] + segment(marker, payload) + data[start + size:]
    raise RuntimeError('marker not found')


def marker_payload(data, marker):
    for code, start, size in markers(data):
        if code == marker:
            return data[start + 4:start + size]
    raise RuntimeError('marker not found')


def strip_markers(data, codes):
    kept = data[:2]
    for code, start, size in markers(data):
        if code not in codes:
            kept += data[start:start + size]
    return kept


def patch_sof(data, code=None, components=None):
    for marker, start, size in markers(data):
        if 0xC0 <= marker <= 0xCF and marker not in (0xC4, 0xC8, 0xCC):
            patched = bytearray(data)
            if code is not None:
                patched[start + 1] = code
            if components:
                for index, (cid, sampling, table) in components.items():
                    base = start + 10 + 3 * index
                    if cid is not None:
                        patched[base] = cid
                    if sampling is not None:
                        patched[base + 1] = sampling
                    if table is not None:
                        patched[base + 2] = table
            return bytes(patched)
    raise RuntimeError('no SOF')


def rst_positions(data):
    return [i for i in range(len(data) - 1) if data[i] == 0xFF and 0xD0 <= data[i + 1] <= 0xD7]


def generated(lane, seed):
    rng = random.Random(seed)
    size = 1 if lane == 'seen' else 3
    items = []

    def add(name, data, cuts=()):
        items.append((name, request(data, cuts)))

    def dims():
        return rng.choice(((1, 1), (2, 3), (7, 9), (8, 8), (16, 16), (17, 13), (31, 33), (40, 24), (63, 65))[size - 1:])

    for space in (YCBCR, GRAYSCALE, RGB, CMYK, YCCK):
        for mode in ('baseline', 'progressive', 'script', 'multiscan', 'restart', 'optimize', 'quality1', 'quality100'):
            width, height = dims()
            params = {'jpeg_space': space}
            if mode == 'progressive':
                params['progressive'] = True
            elif mode == 'script':
                params['scans'] = progressive_script(rng, COMPONENTS[space])
            elif mode == 'multiscan':
                params['scans'] = sequential_script(COMPONENTS[space])
            elif mode == 'restart':
                params['restart'] = rng.choice((1, 2, 5))
                params['progressive'] = rng.random() < 0.5
            elif mode == 'optimize':
                params['optimize'] = True
            elif mode == 'quality1':
                params['quality'] = 1
            elif mode == 'quality100':
                params['quality'] = 100
            data = image(width, height, rng.randrange(1 << 30), **params)
            add(f'{space}-{mode}', data, random_cuts(len(data), rng, 3))
    for index, (h, v) in enumerate(SAMPLINGS):
        for variant, (space, other) in enumerate(((YCBCR, (1, 1)), (YCCK, (1, 1)), (YCBCR, (2, 2) if (h, v) == (1, 1) else (h, v)))):
            count = COMPONENTS[space]
            sampling = [(h, v)] + [other] * (count - 1)
            if space == YCCK:
                sampling[3] = (h, v)
            width, height = rng.randrange(1, 70), rng.randrange(1, 70)
            for progressive in (False, True):
                try:
                    data = image(width, height, rng.randrange(1 << 30), jpeg_space=space, sampling=sampling, progressive=progressive, restart=rng.choice((0, 0, 3)))
                except RuntimeError:
                    continue
                add(f'sampling-{variant}-{h}x{v}-{other[0]}x{other[1]}-{int(progressive)}-{index}', data, random_cuts(len(data), rng, 2))
    for components in ([(2, 1), (1, 2), (1, 1)], [(1, 2), (2, 1), (2, 2)], [(3, 1), (1, 1), (1, 1)], [(4, 1), (2, 1), (1, 1)]):
        data = image(rng.randrange(9, 60), rng.randrange(9, 60), rng.randrange(1 << 30), sampling=components)
        add(f'mixed-{components[0][0]}{components[0][1]}{components[1][0]}{components[1][1]}', data)
    base = image(24, 16, seed, sampling=[(2, 1), (1, 1), (1, 1)])
    add('fractional-sampling', patch_sof(base, components={0: (None, 0x31, None), 1: (None, 0x21, None)}))
    add('zero-sampling', patch_sof(base, components={1: (None, 0x01, None)}))
    add('sampling-5', patch_sof(base, components={0: (None, 0x51, None)}))
    add('blocks-in-mcu-overflow', patch_sof(image(32, 32, seed, sampling=[(2, 2), (1, 1), (1, 1)]), components={0: (None, 0x44, None)}))
    for predictor in range(1, 8):
        for transform in (0, 1):
            space = (GRAYSCALE, RGB, CMYK)[(predictor + transform + seed) % 3]
            data = image(rng.randrange(1, 40), rng.randrange(1, 40), rng.randrange(1 << 30), jpeg_space=space, predictor=predictor, point_transform=transform, sampling=[(1, 1)] * COMPONENTS[space])
            add(f'lossless-{space}-{predictor}-{transform}', data, random_cuts(len(data), rng, 2))
    for precision in (2, 5, 12, 16):
        try:
            data = image(rng.randrange(4, 30), rng.randrange(4, 30), rng.randrange(1 << 30), jpeg_space=RGB, predictor=1 + seed % 7, precision=precision, sampling=[(1, 1)] * 3)
            add(f'lossless-precision-{precision}', data, random_cuts(len(data), rng, 1))
        except RuntimeError:
            pass
    data = image(20, 20, seed, jpeg_space=YCBCR, predictor=1, sampling=[(1, 1)] * 3)
    add('lossless-ycbcr', data)
    for progressive in (False, True):
        data = image(rng.randrange(8, 40), rng.randrange(8, 40), rng.randrange(1 << 30), precision=12, progressive=progressive)
        add(f'precision12-{int(progressive)}', data, random_cuts(len(data), rng, 2))
    base = image(40, 30, seed + 1)
    for code in (0xC1, 0xC3, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF):
        add(f'sof-{code:x}', patch_sof(base, code=code))
    add('rgb-component-ids', image(30, 20, seed + 2, jpeg_space=RGB, adobe=False))
    add('cmyk-without-adobe', image(30, 20, seed + 3, jpeg_space=CMYK, adobe=False))
    add('ycbcr-without-jfif', image(30, 20, seed + 4, jfif=False))
    adobe = image(30, 20, seed + 5, jpeg_space=YCCK)
    payload = bytearray(marker_payload(adobe, 0xEE))
    payload[11] = 1
    add('adobe-transform-1-four-components', replace_marker(adobe, 0xEE, bytes(payload)))
    payload[11] = 7
    add('adobe-transform-7-four-components', replace_marker(adobe, 0xEE, bytes(payload)))
    rgb = image(30, 20, seed + 6, jpeg_space=RGB)
    payload = bytearray(marker_payload(rgb, 0xEE))
    payload[11] = 2
    add('adobe-transform-2-three-components', replace_marker(rgb, 0xEE, bytes(payload)))
    add('odd-component-ids', patch_sof(image(30, 20, seed + 7, jfif=False), components={0: (5, None, None), 1: (9, None, None), 2: (11, None, None)}))
    add('two-components', encode(20, 10, pixels(20, 10, 2, seed), components=2, in_space=UNKNOWN, jpeg_space=UNKNOWN, sampling=[(1, 1), (1, 1)], quant=[0, 1]))
    add('five-components', encode(12, 10, pixels(12, 10, 5, seed), components=5, in_space=UNKNOWN, jpeg_space=UNKNOWN, sampling=[(1, 1)] * 5, quant=[0, 1, 1, 0, 1], scans=sequential_script(5)))
    jfif = bytearray(marker_payload(base, 0xE0))
    jfif[5] = 2
    add('jfif-version-2', replace_marker(base, 0xE0, bytes(jfif)))
    add('std-huffman-tables', strip_markers(image(48, 32, seed + 8), {0xC4}))
    add('progressive-without-tables', strip_markers(image(48, 32, seed + 9, progressive=True), {0xC4}))
    add('no-quant-tables', strip_markers(base, {0xDB}))
    add('tables-only', base[:2] + b''.join(base[start:start + size] for code, start, size in markers(base) if code in (0xDB, 0xC4)) + b'\xff\xd9')
    add('sof-then-eoi', base[:2] + b''.join(base[start:start + size] for code, start, size in markers(base) if code != 0xDA) + b'\xff\xd9')
    add('sos-before-sof', base[:2] + b''.join(base[start:start + size] for code, start, size in markers(base) if code != 0xC0))
    add('duplicate-soi', base[:2] + base)
    add('duplicate-sof', base[:2] + b''.join(base[start:start + size] for code, start, size in markers(base) if 0xC0 == code) + base[2:])
    add('com-and-appn', insert_after_soi(base, [(0xFE, b'comment'), (0xE3, b'x' * 40), (0xEF, b'APP15'), (0xEE, b'NotAdobe' + bytes(8)), (0xE0, b'JFXX\0\x10' + bytes(20))]))
    add('garbage-between-markers', base[:2] + b'\x00\x13garbage\xff\xff\xff' + base[2:])
    add('stuffed-zero-before-marker', base[:2] + b'\xff\x00' + base[2:])
    add('reserved-marker', insert_after_soi(base, [(0xF3, b'reserved')]))
    add('bad-dqt-length', replace_marker(base, 0xDB, marker_payload(base, 0xDB) + b'\x01'))
    add('bad-dri-length', insert_after_soi(base, [(0xDD, b'\x00\x04\x00')]))
    add('dqt-16-bit', image(32, 24, seed + 10, quality=3))
    add('bad-dht-index', insert_after_soi(base, [(0xC4, bytes([0x15]) + bytes(16))]))
    add('bad-dht-count', insert_after_soi(base, [(0xC4, bytes([0x00]) + bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255]))]))
    add('overfull-huffman', insert_after_soi(base, [(0xC4, bytes([0x00, 3]) + bytes(15) + b'\x00\x01\x02')]))
    add('dc-symbol-16', insert_after_soi(strip_markers(base, {0xC4}), [(0xC4, bytes([0x00, 0, 1]) + bytes(14) + bytes([16]))]))
    multiscan = image(64, 48, seed + 11, scans=sequential_script(3), restart=2)
    first_scan = [start for code, start, size in markers(multiscan) if code == 0xDA][0]
    sos = multiscan.find(b'\xff\xda', first_scan + 2)
    add('dnl-between-scans', multiscan[:sos] + segment(0xDC, b'\x00\x30') + multiscan[sos:])
    add('dri-between-scans', multiscan[:sos] + segment(0xDD, b'\x00\x01') + multiscan[sos:])
    add('dqt-between-scans', multiscan[:sos] + segment(0xDB, b'\x00' + bytes(range(1, 65))) + multiscan[sos:])
    restarts = image(64, 48, seed + 12, restart=1)
    positions = rst_positions(restarts)
    for name, changes in (('rst-skip', {positions[2]: 0xD5}), ('rst-prior', {positions[3]: 0xD1}), ('rst-far', {positions[4]: 0xD0 + (5 + 4) % 8}), ('rst-not-rst', {positions[2]: 0xC0}), ('rst-low-marker', {positions[2]: 0x05})):
        patched = bytearray(restarts)
        for position, value in changes.items():
            patched[position + 1] = value
        add(name, bytes(patched))
    removed = bytearray(restarts)
    del removed[positions[1]:positions[1] + 2]
    add('rst-missing', bytes(removed))
    add('missing-eoi', image(40, 30, seed + 13)[:-2])
    add('trailing-data', image(40, 30, seed + 14) + b'trailing\xff\xd8\xff')
    add('extraneous-before-eoi', image(40, 30, seed + 15)[:-2] + b'\x12\x34\xff\xd9')
    for width, height in ((5000, 4000), (40000, 20000), (65500, 1), (65501, 1), (1, 65535)):
        header = image(16, 16, seed + width % 97)
        for marker, start, length in markers(header):
            if marker == 0xC0:
                header = header[:start + 5] + struct.pack('>HH', height, width) + header[start + 9:]
        add(f'large-{width}x{height}', header)
    for orientation in range(10):
        data = insert_after_soi(image(13 + orientation, 7, seed + orientation), [(0xE1, exif(orientation, little=(orientation + seed) % 2 == 1))])
        add(f'exif-orientation-{orientation}', data)
    for name, payload in (('exif-density-match', exif(1, resolution=((144, 1), (144, 1)), unit=2, dimensions=(10, 4))),
                          ('exif-density-mismatch', exif(6, resolution=((144, 1), (72, 1)), unit=2, dimensions=(10, 4))),
                          ('exif-density-cm', exif(1, resolution=((144, 1), (144, 1)), unit=3, dimensions=(10, 4))),
                          ('exif-density-zero-denominator', exif(1, resolution=((144, 0), (144, 1)), unit=2, dimensions=(10, 4))),
                          ('exif-truncated', exif(3)[:14]),
                          ('exif-bad-endian', b'Exif\0\0XX\0*' + exif(3)[10:]),
                          ('exif-wrong-type', exif(None, raw_entries=[(0x0112, 4, 1, 6)])),
                          ('exif-count-2', exif(None, raw_entries=[(0x0112, 3, 2, 6)])),
                          ('exif-sub-ifd-loop', exif(1, dimensions=(20, 8), resolution=((144, 1), (144, 1)), unit=2)),
                          ('exif-short-signature', b'Exif\0')):
        add(name, insert_after_soi(image(20, 8, seed + len(name)), [(0xE1, payload)]))
    add('exif-after-xmp', insert_after_soi(image(16, 8, seed), [(0xE1, b'http://ns.adobe.com/xap/1.0/\0<x/>'), (0xE1, exif(8))]))
    add('exif-twice', insert_after_soi(image(16, 8, seed + 1), [(0xE1, exif(5)), (0xE1, exif(7))]))
    profile = icc_profile(5000 + seed * 13, seed)
    for name, segments_list in (('icc-single', icc_segments(profile, 1)), ('icc-three', icc_segments(profile, 3)), ('icc-reversed', icc_segments(profile, 3, order=[3, 2, 1])),
                                ('icc-duplicate', icc_segments(profile, 3, order=[1, 2, 2, 3])), ('icc-missing', icc_segments(profile, 3, order=[1, 3])),
                                ('icc-count-zero', icc_segments(profile, 2, count=0)), ('icc-index-over', icc_segments(profile, 2, order=[1, 3])),
                                ('icc-conflicting', icc_segments(profile, 2) + [(0xE2, b'ICC_PROFILE\0\x02\x03xyz')]),
                                ('icc-empty-part', [(0xE2, b'ICC_PROFILE\0\x01\x01')] + icc_segments(profile, 1)),
                                ('icc-with-mpf', [(0xE2, b'MPF\0' + bytes(20))] + icc_segments(profile, 2))):
        add(name, insert_after_soi(image(20, 12, seed + len(name)), segments_list))
    add('icc-large-encoder', image(24, 16, seed, icc=icc_profile(150000, seed)))
    for name in ('baseline', 'progressive'):
        data = image(16, 16, seed + 20, progressive=name == 'progressive')
        add(f'byte-by-byte-{name}', data, list(range(1, len(data))))
    big = image(400 + seed % 7, 300, seed + 30, quality=95)
    add('large-baseline-chunks', big, [4000, 4063, 4064, 4065, 8136, 8200, 20000])
    big_progressive = image(320, 240 + seed % 5, seed + 31, progressive=True, sampling=[(1, 1)] * 3)
    add('large-progressive-chunks', big_progressive, sorted(rng.randrange(len(big_progressive)) for _ in range(6)))
    for index in range(12):
        data = random_jpeg(rng)
        cut = rng.randrange(len(data))
        add(f'truncated-{index}', data[:cut], random_cuts(cut, rng, 2))
    for index in range(40 * size):
        data = random_jpeg(rng)
        if index % 2:
            data = mutate(data, rng)
        add(f'random-{index}', data, random_cuts(len(data), rng, 3))
    return items


def fixture_items(lane):
    items = []
    files = sorted(FIXTURES.rglob('*.*'))
    rng = random.Random(4242)
    for index, path in enumerate(files):
        data = path.read_bytes()
        if (index % 3 == 0) != (lane == 'holdout'):
            continue
        name = f'{path.parent.name}-' + path.stem.replace('.', '-').replace('_', '-').lower()
        items.append((f'fixture-{index}-{name}', request(data)))
        if len(data) <= 200000:
            items.append((f'fixture-cuts-{index}-{name}', request(data, random_cuts(len(data), rng, 3))))
            cut = rng.randrange(len(data) + 1)
            items.append((f'fixture-truncated-{index}-{name}', request(data[:cut])))
            items.append((f'fixture-mutated-{index}-{name}', request(mutate(data, rng))))
    return items


def lane_items(lane):
    return generated(lane, 1 if lane == 'seen' else 2) + fixture_items(lane)
