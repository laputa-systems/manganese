import hashlib
import os
import pathlib
import random
from compare import ask, peer, request_batch, request_decode, request_encode, request_label

os.chdir(pathlib.Path(__file__).resolve().parents[3])
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
scratch = pathlib.Path('.work/tmp/encoding-store')
scratch.mkdir(parents=True, exist_ok=True)
oracle = peer('.work/tmp/encoding/oracle')
labels = ask(oracle, b'\x03').splitlines()
if not labels:
    raise SystemExit('oracle returned no labels')
rows = []
seen_inputs = set()
pending = []


def save(data):
    digest = hashlib.sha256(data).hexdigest()
    target = store / digest
    if target.exists():
        if target.read_bytes() != data:
            raise RuntimeError(f'corrupt stored blob {digest}')
        return digest, len(data)
    temporary = scratch / f'{digest}.{os.getpid()}'
    temporary.write_bytes(data)
    try:
        try:
            os.link(temporary, target)
        except FileExistsError:
            if target.read_bytes() != data:
                raise RuntimeError(f'corrupt stored blob {digest}')
    finally:
        temporary.unlink()
    return digest, len(data)


def add(lane, name, data):
    digest = hashlib.sha256(data).digest()
    if digest in seen_inputs:
        return
    seen_inputs.add(digest)
    pending.append(data)
    if len(pending) == 64:
        flush(lane)


def flush(lane):
    if not pending:
        return
    data = request_batch(pending)
    expected = ask(oracle, data)
    a, b = save(data), save(expected)
    rows.append(f'encoding {lane} {lane}-{len(rows)} {a[0]} {a[1]} {b[0]} {b[1]}')
    pending.clear()


canonical = {}
for label in labels:
    name = ask(oracle, request_label(label))
    if name:
        canonical.setdefault(name, label)
for lane in ('seen', 'holdout'):
    for i, label in enumerate(labels):
        value = label.lower() if lane == 'seen' else label.upper()
        add(lane, f'label-{i}', request_label(value))
        add(lane, f'web-label-{i}', request_label(value, web=True))
        add(lane, f'whitespace-label-{i}', request_label(b'\t' + value + (b'\n' if lane == 'seen' else b'\r')))
    for invalid in (b'', b'not-an-encoding', b'x' * 64, b'utf\x008'):
        value = invalid if lane == 'seen' else b' ' + invalid + b'\t'
        add(lane, 'invalid-label', request_label(value))
    for i, label in enumerate(canonical.values()):
        rng = random.Random(90100 + i + (0 if lane == 'seen' else 10000))
        inputs = [b'', b'ASCII\x00', b'\xff', b'\x81\x30\x81\x30', b'\x1b$B$\x22\x1b(B', b'\x82\xa0', b'\xe2\x82\xac', b'\xef\xbb\xbfX', b'\xff\xfeA\x00', b'\xfe\xff\x00A', rng.randbytes(8)]
        for j, data in enumerate(inputs):
            for cut in range(len(data) + 1):
                chunks = [data[:cut], data[cut:]]
                if lane == 'holdout':
                    chunks.insert(0, b'')
                for mode in range(3):
                    for fatal in range(2):
                        add(lane, f'decode-{i}-{j}-{cut}-{mode}-{fatal}', request_decode(label, mode, fatal, chunks))
        text = ('ASCII € 中 あ 😀' + str(i) + lane).encode('utf-16-be')
        add(lane, f'encode-{i}', request_encode(label, text))
    flush(lane)
    for i, label in enumerate(canonical.values()):
        malformed_utf16 = b'\xd8\x00\x00A\xdc\x00' + lane.encode('utf-16-be')
        add(lane, f'encode-surrogates-{i}', request_encode(label, malformed_utf16))
    flush(lane)
    for i, label in enumerate(canonical.values()):
        malformed_utf16 = b'\x30\x42\xd8\x00\x30\x42' + lane.encode('utf-16-be')
        add(lane, f'encode-state-surrogate-{i}', request_encode(label, malformed_utf16))
    flush(lane)
    data = bytes.fromhex('feff8fc67d8c2dcbbc64856d2a')
    for cut in range(len(data) + 1):
        chunks = [data[:cut], data[cut:]]
        if lane == 'holdout':
            chunks.insert(0, b'')
        for mode in range(3):
            for fatal in range(2):
                add(lane, f'euc-jp-regression-{cut}-{mode}-{fatal}', request_decode(b'euc-jp', mode, fatal, chunks))
    flush(lane)
    for value in ('8862', '8864', '88a3', '88a5'):
        data = bytes.fromhex(value)
        for cut in range(len(data) + 1):
            chunks = [data[:cut], data[cut:]]
            if lane == 'holdout':
                chunks.insert(0, b'')
            for mode in range(3):
                for fatal in range(2):
                    add(lane, f'big5-composite-{value}-{cut}-{mode}-{fatal}', request_decode(b'big5', mode, fatal, chunks))
    flush(lane)
oracle.stdin.close()
if oracle.wait():
    raise SystemExit('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('encoding ')]
temporary = scratch / f'goldens.lock.{os.getpid()}'
try:
    temporary.write_text('\n'.join(existing + rows) + '\n')
    os.replace(temporary, lock)
finally:
    temporary.unlink(missing_ok=True)
print('recorded', len(rows), 'cases from', len(labels), 'labels and', len(canonical), 'codecs')
