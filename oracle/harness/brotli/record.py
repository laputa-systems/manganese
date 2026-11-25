import hashlib
import os
import pathlib
import random
import subprocess
from compare import ask, peer, request

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
scratch = pathlib.Path('.work/tmp/brotli')
STREAM, EXACT, BUFFER, PATCH, SHARED, SHARED_DICT = range(6)
UPSTREAM_READ = 32768

words = {}


def vocabulary(rng, key):
    if key not in words:
        letters = 'abcdefghijklmnopqrstuvwxyz'
        words[key] = [''.join(rng.choice(letters) for _ in range(rng.randrange(2, 11))) for _ in range(700)]
    return words[key]


def text(rng, size, key):
    vocab = vocabulary(rng, key)
    out = []
    length = 0
    while length < size:
        word = vocab[min(int(rng.expovariate(0.012)), len(vocab) - 1)]
        out.append(word)
        length += len(word) + 1
    return ' '.join(out).encode()[:size]


def markup(rng, size, key):
    vocab = vocabulary(rng, key)
    out = []
    length = 0
    while length < size:
        tag = rng.choice(['div', 'span', 'p', 'a', 'li', 'td'])
        piece = f'<{tag} class="{rng.choice(vocab)}" id="{rng.choice(vocab)}{rng.randrange(1000)}">{" ".join(rng.choice(vocab) for _ in range(rng.randrange(1, 9)))}</{tag}>\n'
        out.append(piece)
        length += len(piece)
    return ''.join(out).encode()[:size]


def style(rng, size, key):
    vocab = vocabulary(rng, key)
    out = []
    length = 0
    while length < size:
        piece = f'.{rng.choice(vocab)}-{rng.choice(vocab)} {{ margin: {rng.randrange(40)}px; color: #{rng.randrange(1 << 24):06x}; font-family: {rng.choice(vocab)}; }}\n'
        out.append(piece)
        length += len(piece)
    return ''.join(out).encode()[:size]


def table(rng, size):
    out = bytearray()
    value = 0
    while len(out) < size:
        value = (value + rng.randrange(-40, 80)) & 0xFFFF
        out += value.to_bytes(2, 'big') + bytes([rng.randrange(8), 0, rng.randrange(256), 0])
    return bytes(out[:size])


def documents(lane):
    rng = random.Random(10023 if lane == 'seen' else 20046)
    if lane == 'seen':
        return [
            b'', b'a', text(rng, 300, 'a'), text(rng, 33000, 'a'), markup(rng, 70000, 'a'), table(rng, 40000), rng.randbytes(8192),
            b'window-boundary-' * 65536, bytes(300000), pathlib.Path('docs/oracle.md').read_bytes(), style(rng, 12000, 'a'),
            bytes(range(256)) * 40,
        ]
    return [
        b'b', text(rng, 5000, 'b'), markup(rng, 100000, 'b'), style(rng, 30000, 'b'), table(rng, 65537), rng.randbytes(8193),
        b'holdout-window-boundary-' * 50000, b'\x00\x01' * 100000, text(rng, 262145, 'b'), bytes(range(255, -1, -1)) * 41,
    ]


cache = {}


def encode(data, quality, window, mode=0, dictionary=b''):
    key = (hashlib.sha256(data).digest(), quality, window, mode, hashlib.sha256(dictionary).digest())
    if key not in cache:
        args = [str(scratch / 'encode'), str(quality), str(window), str(mode)]
        if dictionary:
            path = scratch / 'fixture.dict'
            path.write_bytes(dictionary)
            args.append(str(path))
        cache[key] = subprocess.run(args, input=data, stdout=subprocess.PIPE, check=True).stdout
    return cache[key]


def chunks(data, width):
    width = min(max(width, -(-len(data) // 60000)), UPSTREAM_READ)
    return [data[i:i + width] for i in range(0, len(data), width)]


def blob(data):
    digest = hashlib.sha256(data).hexdigest()
    path = store / digest
    if not path.exists():
        path.write_bytes(data)
    return digest, len(data)


process = peer(str(scratch / 'oracle'))
rows = []
seen_digests = set()


def add(lane, name, value):
    result = ask(process, value)
    a, b = blob(value), blob(result)
    if lane == 'holdout' and (a[0] in seen_digests or b[0] in seen_digests):
        return
    if lane == 'seen':
        seen_digests.update((a[0], b[0]))
    rows.append(f'brotli {lane} {lane}-{name} {a[0]} {a[1]} {b[0]} {b[1]}')


def case(lane, name, mode, output_size, data, dictionary=b''):
    add(lane, name, request(mode, output_size, dictionary, [data]))


def dictionary_cases(lane, dictionary_source, target_source):
    dictionary = dictionary_source[-6000:]
    data = target_source[:2500] + dictionary[1000:3000] + target_source[2500:3500] + dictionary[:500]
    used = 0
    for quality in (2, 4, 6, 9, 11):
        encoded = encode(data, quality, 22, 0, dictionary)
        with_dictionary = request(STREAM, 4096, dictionary, chunks(encoded, 1000))
        without_dictionary = request(STREAM, 4096, b'', chunks(encoded, 1000))
        if ask(process, with_dictionary) == ask(process, without_dictionary):
            continue
        used += 1
        add(lane, f'dictionary-{quality}', with_dictionary)
        add(lane, f'dictionary-small-{quality}', request(STREAM, 61, dictionary, chunks(encoded, 17)))
        add(lane, f'dictionary-shared-{quality}', request(SHARED_DICT, len(data), dictionary, [encoded]))
        add(lane, f'dictionary-shared-short-{quality}', request(SHARED_DICT, len(data) - 1, dictionary, [encoded]))
        add(lane, f'dictionary-shared-long-{quality}', request(SHARED_DICT, len(data) + 50, dictionary, [encoded]))
        if lane == 'seen':
            add(lane, f'missing-dictionary-{quality}', without_dictionary)
            add(lane, f'missing-dictionary-shared-{quality}', request(SHARED, len(data), b'', [encoded]))
            add(lane, f'wrong-dictionary-{quality}', request(STREAM, 4096, dictionary[::-1], chunks(encoded, 1000)))
            add(lane, f'empty-dictionary-shared-{quality}', request(SHARED_DICT, len(data), b'', [encoded]))
            add(lane, f'dictionary-oneshot-{quality}', request(EXACT, len(data), b'', [encoded]))
    if used < 3:
        raise RuntimeError('reference encoder rarely used the dictionary')


def stream_cases(lane, index, data, encoded, tag, small):
    for width, output_size in ((UPSTREAM_READ, 16384), (17, 4096), (1000, 65536)):
        add(lane, f'stream-{index}-{tag}-{width}-{output_size}', request(STREAM, output_size, b'', chunks(encoded, width)))
    if small:
        add(lane, f'stream-{index}-{tag}-tiny', request(STREAM, 1, b'', chunks(encoded, 1)))
        add(lane, f'stream-{index}-{tag}-tiny-output', request(STREAM, 3, b'', chunks(encoded, 4093)))


def caller_cases(lane, index, data, encoded, tag):
    add(lane, f'woff2-{index}-{tag}', request(EXACT, len(data), b'', [encoded]))
    add(lane, f'resource-{index}-{tag}', request(BUFFER, len(data), b'', [encoded]))
    add(lane, f'patch-{index}-{tag}', request(PATCH, 32768, b'', [encoded]))
    add(lane, f'shared-{index}-{tag}', request(SHARED, len(data), b'', [encoded]))
    add(lane, f'shared-slack-{index}-{tag}', request(SHARED, len(data) + 1000, b'', [encoded]))
    add(lane, f'resource-slack-{index}-{tag}', request(BUFFER, len(data) + 9, b'', [encoded]))
    if len(data):
        add(lane, f'woff2-short-{index}-{tag}', request(EXACT, len(data) - 1, b'', [encoded]))
        add(lane, f'resource-short-{index}-{tag}', request(BUFFER, len(data) - 1, b'', [encoded]))
        add(lane, f'shared-short-{index}-{tag}', request(SHARED, len(data) - 1, b'', [encoded]))
    add(lane, f'woff2-long-{index}-{tag}', request(EXACT, len(data) + 1, b'', [encoded]))


def damaged_cases(lane, index, data, encoded):
    if len(encoded) < 4:
        return
    cuts = sorted({1, len(encoded) // 4, len(encoded) // 2, len(encoded) - 2, len(encoded) - 1})
    for cut in cuts:
        short = encoded[:cut]
        add(lane, f'truncated-{index}-{cut}', request(STREAM, 4096, b'', chunks(short, 1000)))
        add(lane, f'truncated-oneshot-{index}-{cut}', request(BUFFER, len(data), b'', [short]))
        add(lane, f'truncated-patch-{index}-{cut}', request(PATCH, 32768, b'', [short]))
        add(lane, f'truncated-shared-{index}-{cut}', request(SHARED, len(data), b'', [short]))
    for suffix in (b'x', b'garbage' * 40, bytes(70000)):
        trailing = encoded + suffix
        add(lane, f'trailing-{index}-{len(suffix)}', request(STREAM, 4096, b'', chunks(trailing, 20000)))
        add(lane, f'trailing-oneshot-{index}-{len(suffix)}', request(BUFFER, len(data), b'', [trailing]))
        add(lane, f'trailing-exact-{index}-{len(suffix)}', request(EXACT, len(data), b'', [trailing]))
        add(lane, f'trailing-patch-{index}-{len(suffix)}', request(PATCH, 32768, b'', [trailing]))
        add(lane, f'trailing-shared-{index}-{len(suffix)}', request(SHARED, len(data), b'', [trailing]))
    rng = random.Random(index + len(encoded))
    for flip in range(6):
        bad = bytearray(encoded)
        position = len(bad) - 1 - rng.randrange(max(1, len(bad) // 2)) if flip % 2 else rng.randrange(len(bad))
        bad[position] ^= 1 << rng.randrange(8)
        value = bytes(bad)
        add(lane, f'corrupt-{index}-{flip}', request(STREAM, 4096, b'', chunks(value, 7 + flip * 301)))
        add(lane, f'corrupt-oneshot-{index}-{flip}', request(BUFFER, len(data), b'', [value]))
        add(lane, f'corrupt-patch-{index}-{flip}', request(PATCH, 32768, b'', [value]))
        add(lane, f'corrupt-shared-{index}-{flip}', request(SHARED, len(data), b'', [value]))


def corpus(lane):
    docs = documents(lane)
    qualities = (0, 1, 2, 4, 5, 6, 9, 11)
    windows = (10, 12, 16, 18, 20, 22, 24)
    for index, data in enumerate(docs):
        wide = index % 3 == 0 or len(data) < 1000
        for quality in qualities if wide else (1, 6, 11):
            if quality == 11 and len(data) > 400000:
                continue
            for window in windows if wide else (16, 22, 24):
                encoded = encode(data, quality, window)
                tag = f'{quality}-{window}'
                stream_cases(lane, index, data, encoded, tag, len(encoded) < 600)
                if (quality, window) in ((0, 10), (1, 16), (6, 22), (11, 24), (9, 20), (4, 18)) or not wide:
                    caller_cases(lane, index, data, encoded, tag)
        for mode_name, mode in (('text', 1), ('font', 2)):
            encoded = encode(data, 9, 22, mode)
            stream_cases(lane, index, data, encoded, mode_name, False)
            caller_cases(lane, index, data, encoded, mode_name)
        damaged_cases(lane, index, data, encode(data, 6, 22))
        if lane == 'seen':
            for window in (25, 26, 28, 30):
                encoded = encode(data, 6, window)
                add(lane, f'large-window-{index}-{window}', request(STREAM, 4096, b'', chunks(encoded, 1000)))
                add(lane, f'large-window-oneshot-{index}-{window}', request(EXACT, len(data), b'', [encoded]))
    donor, target = docs[3 if lane == 'seen' else 2], docs[4 if lane == 'seen' else 3]
    dictionary_cases(lane, donor, target)


corpus('seen')
corpus('holdout')
if lane_rows := [row for row in rows if ' seen ' in row]:
    holdout = len(rows) - len(lane_rows)
    if holdout * 100 < len(rows) * 30:
        raise RuntimeError(f'holdout share too small: {holdout} of {len(rows)}')
process.stdin.close()
if process.wait():
    raise SystemExit('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('brotli ')]
lock.write_text('\n'.join(existing + rows) + '\n')
print('recorded', len(rows), 'holdout', holdout)
