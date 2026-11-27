import hashlib
import os
import pathlib
import random
import struct
import subprocess
import tempfile

import corpus

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
scratch = pathlib.Path('.work/tmp/zip')
scratch.mkdir(parents=True, exist_ok=True)


def open_oracle():
    process = subprocess.Popen(['.work/tmp/zip/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    try:
        process.stdin.write(b'MGB1')
        process.stdin.flush()
        if process.stdout.read(4) != b'MGB1':
            raise RuntimeError('oracle protocol header')
    except BaseException:
        stop_oracle(process)
        raise
    return process


def stop_oracle(process):
    if process.poll() is None:
        process.kill()
    process.wait()


def ask(process, data):
    process.stdin.write(struct.pack('>I', len(data)) + data)
    process.stdin.flush()
    size = process.stdout.read(4)
    if len(size) != 4:
        raise RuntimeError('oracle terminated')
    count = struct.unpack('>I', size)[0]
    result = process.stdout.read(count)
    if len(result) != count:
        raise RuntimeError('short oracle response')
    return result


def stage(data, directory, prefix):
    with tempfile.NamedTemporaryFile(dir=directory, prefix=prefix, delete=False) as temporary:
        path = pathlib.Path(temporary.name)
        try:
            temporary.write(data)
            temporary.flush()
            os.fsync(temporary.fileno())
        except BaseException:
            path.unlink()
            raise
    return path


def blob(data):
    digest = hashlib.sha256(data).hexdigest()
    path = store / digest
    if path.exists():
        if path.read_bytes() != data:
            raise RuntimeError('artifact digest collision')
        return digest, len(data)
    temporary = stage(data, store, 'blob-')
    try:
        try:
            os.link(temporary, path)
        except FileExistsError:
            if path.read_bytes() != data:
                raise RuntimeError('artifact digest collision')
    finally:
        temporary.unlink()
    return digest, len(data)


def replace_lock(rows):
    lock = pathlib.Path('goldens.lock')
    lines = lock.read_text().splitlines()
    existing = [line for line in lines if not line.startswith('zip ')]
    if len(existing) != len(lines):
        raise RuntimeError('zip goldens already exist')
    temporary = stage(('\n'.join(existing + rows) + '\n').encode(), scratch, 'lock-')
    try:
        os.replace(temporary, lock)
    finally:
        temporary.unlink(missing_ok=True)


def main():
    oracle = open_oracle()
    try:
        rows = []

        def add(lane, name, data):
            request = corpus.encode_request(data)
            source = blob(request)
            result = blob(ask(oracle, request))
            rows.append(f'zip {lane} {lane}-{name} {source[0]} {source[1]} {result[0]} {result[1]}')

        base = corpus.cases()
        for lane in ('seen', 'holdout'):
            if lane == 'seen':
                for name, data in base:
                    add(lane, name, data)
            first = 21 if lane == 'seen' else 91
            for seed in range(first, first + 4):
                rng = random.Random(seed)
                for index in range(100):
                    add(lane, f'r{seed}-{index}', corpus.random_case(rng))
        oracle.stdin.close()
        if oracle.wait():
            raise RuntimeError('oracle failed')
    finally:
        stop_oracle(oracle)
    replace_lock(rows)
    print('recorded', len(rows), 'zip goldens:', sum(1 for row in rows if ' seen ' in row),
          'seen', sum(1 for row in rows if ' holdout ' in row), 'holdout')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
