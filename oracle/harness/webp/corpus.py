import hashlib
import pathlib
import random
import subprocess

root = pathlib.Path(__file__).resolve().parents[3]
fixtures = root / '.work/tmp/webp/fixtures'
encoder = root / '.work/tmp/webp/encoder'
sizes = [(w, h) for w in (1, 2, 3, 7, 8, 9, 15, 16, 17, 31, 32, 33, 64, 65) for h in (1, 3, 7, 8, 15, 16, 17, 31, 32, 33, 64) if w * h <= 4096]
qualities = (1, 25, 50, 75, 80, 90, 92, 95, 99, 100)


def request(data):
    return b'\x00' + data


def request_data(record):
    return record[1:]


def dimensions(data):
    if len(data) < 12 or data[:4] != b'RIFF' or data[8:12] != b'WEBP':
        return None
    position = 12
    while position + 8 <= len(data):
        chunk = data[position:position + 4]
        length = int.from_bytes(data[position + 4:position + 8], 'little')
        payload = data[position + 8:position + 8 + length]
        if chunk == b'VP8X' and len(payload) >= 10:
            return 1 + int.from_bytes(payload[4:7], 'little'), 1 + int.from_bytes(payload[7:10], 'little')
        if chunk == b'VP8 ' and len(payload) >= 10:
            return int.from_bytes(payload[6:8], 'little') & 0x3fff, int.from_bytes(payload[8:10], 'little') & 0x3fff
        if chunk == b'VP8L' and len(payload) >= 5:
            bits = int.from_bytes(payload[1:5], 'little')
            return (bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1
        position += 8 + length + (length & 1)
    return None


def encode(seed, width, height, lossless, quality):
    return subprocess.run([str(encoder), str(seed), str(width), str(height), str(lossless), str(quality)],
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True).stdout


def random_webp(rng):
    width, height = rng.choice(sizes)
    lossless = int(rng.random() < 0.4)
    return encode(rng.randrange(1, 1 << 62), width, height, lossless, rng.choice(qualities))


def mutate(data, rng):
    return data + bytes(rng.randrange(256) for _ in range(rng.randrange(1, 16)))


def base_items():
    items = []
    for path in sorted(fixtures.rglob('*.webp')):
        if path.name == 'skbug3429.webp':
            continue
        data = path.read_bytes()
        if len(data) > 131072:
            continue
        dims = dimensions(data)
        if dims is None or dims[0] * dims[1] > 65536:
            continue
        items.append((path.stem, data))
    return items


def digest(data):
    return hashlib.sha256(data).digest()


def _generated(offset, count):
    for index in range(count):
        seed = offset + index
        rng = random.Random(seed)
        width, height = rng.choice(sizes)
        lossless = int(rng.random() < 0.4)
        quality = rng.choice(qualities)
        yield f'generated-{seed}', encode(seed, width, height, lossless, quality)


def _mutated(seed, count, pool):
    rng = random.Random(seed)
    for index in range(count):
        yield f'mutated-{index}', mutate(rng.choice(pool), rng)


_all = None


def all_items():
    global _all
    if _all is not None:
        return _all
    fixtures_by_lane = {'seen': [], 'holdout': []}
    for index, (name, data) in enumerate(base_items()):
        fixtures_by_lane['holdout' if index % 3 == 0 else 'seen'].append((f'fixture-{name.lower()}', data))
    generated_by_lane = {
        'seen': list(_generated(100000, 400)),
        'holdout': list(_generated(900000, 200)),
    }
    emitted = set()
    result = {'seen': [], 'holdout': []}
    seen_hashes = set()
    for name, data in fixtures_by_lane['seen'] + generated_by_lane['seen']:
        key = digest(data)
        if key in emitted:
            continue
        emitted.add(key)
        seen_hashes.add(key)
        result['seen'].append((name, request(data)))
    for name, data in fixtures_by_lane['holdout'] + generated_by_lane['holdout']:
        key = digest(data)
        if key in emitted or key in seen_hashes:
            continue
        emitted.add(key)
        result['holdout'].append((name, request(data)))
    _all = result
    return result


def lane_items(lane):
    return all_items()[lane]
