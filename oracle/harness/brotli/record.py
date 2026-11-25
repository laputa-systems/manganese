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
rng = random.Random(10023)
inputs = {
    'seen': [b'', b'a', b'hello world\n' * 1000, bytes(range(256)) * 40, rng.randbytes(8192), b'window-boundary-' * 65536],
    'holdout': [b'b', b'another document\n' * 4000, bytes(range(255, -1, -1)) * 41, rng.randbytes(8193)],
}
rows = []
process = peer('.work/tmp/brotli/oracle')


def blob(data):
    digest = hashlib.sha256(data).hexdigest()
    path = store / digest
    if path.exists() and path.read_bytes() != data:
        raise RuntimeError('digest collision')
    if not path.exists():
        path.write_bytes(data)
    return digest, len(data)


def add(lane, name, value):
    result = ask(process, value)
    a, b = blob(value), blob(result)
    rows.append(f'brotli {lane} {lane}-{name} {a[0]} {a[1]} {b[0]} {b[1]}')


def encode(data, quality, window, dictionary=b''):
    args = ['.work/tmp/brotli/encode', str(quality), str(window)]
    if dictionary:
        dictionary_path = pathlib.Path('.work/tmp/brotli/fixture.dict')
        dictionary_path.write_bytes(dictionary)
        args.append(str(dictionary_path))
    return subprocess.run(args, input=data, stdout=subprocess.PIPE, check=True).stdout


def chunks(data, width):
    width = min(width, 32768)
    return [data[i:i + width] for i in range(0, len(data), width)] or [b'']


for lane, cases in inputs.items():
    for index, data in enumerate(cases):
        for quality in (0, 1, 4, 6, 9, 11):
            for window in ((10, 16, 20, 22, 24, 25) if lane == 'seen' else (10, 16, 20, 22, 24)):
                encoded = encode(data, quality, window)
                for chunk_size, output_size in ((len(encoded) + 1, 16384), (17, 4096)):
                    value = request(0, output_size, b'', chunks(encoded, chunk_size))
                    add(lane, f'decode-{index}-{quality}-{window}-{chunk_size}-{output_size}', value)
                if lane == 'seen' and quality == 6 and window == 22:
                    negatives = [('truncated', encoded[:-1]), ('trailing', encoded + b'garbage')]
                    if lane == 'seen' and index == 0:
                        negatives.append(('empty', b''))
                    for name, bad in negatives:
                        add(lane, f'{name}-{index}', request(0, 1024, b'', [bad]))
                    if encoded:
                        bad = bytearray(encoded)
                        bad[len(bad) // 2] ^= 0x40
                        add(lane, f'corrupt-{index}', request(0, 1024, b'', chunks(bytes(bad), 7)))
                if quality in (0, 6, 11) and window in (10, 22, 24):
                    for mode, name, size in ((1, 'one', len(data)), (2, 'resource', len(data)), (3, 'patch', 32768)):
                        add(lane, f'{name}-{index}-{quality}-{window}', request(mode, size, b'', [encoded]))
                    if lane == 'seen':
                        for size in (max(0, len(data) - 1), len(data) + 1):
                            add(lane, f'one-size-{index}-{quality}-{window}-{size}', request(1, size, b'', [encoded]))
                        add(lane, f'patch-trailing-{index}-{quality}-{window}', request(3, 32768, b'', [encoded + b'x']))
    dictionary = (b'shared dictionary ' + lane.encode() + b' documents and resources ') * 300
    data = dictionary[-4096:] + b' and decoded suffix ' + lane.encode()
    for quality in (4, 6, 9, 11):
        encoded = encode(data, quality, 22, dictionary)
        with_dict = request(0, 4096, dictionary, chunks(encoded, 17))
        without_dict = request(0, 4096, b'', chunks(encoded, 17))
        if ask(process, with_dict) == ask(process, without_dict):
            raise RuntimeError('reference encoder did not use the dictionary')
        add(lane, f'dictionary-{quality}', with_dict)
        if lane == 'seen':
            add(lane, f'missing-dictionary-{quality}', without_dict)
process.stdin.close()
if process.wait():
    raise SystemExit('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('brotli ')]
lock.write_text('\n'.join(existing + rows) + '\n')
print('recorded', len(rows))
