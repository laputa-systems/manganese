import hashlib
import os
import pathlib
import struct
import subprocess
import tempfile

import corpus

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
scratch = pathlib.Path('.work/tmp/regex')
scratch.mkdir(parents=True, exist_ok=True)


def open_oracle():
    process = subprocess.Popen(['.work/tmp/regex/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
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
    header = process.stdout.read(4)
    if len(header) != 4:
        raise RuntimeError('oracle terminated')
    length = struct.unpack('>I', header)[0]
    result = process.stdout.read(length)
    if len(result) != length:
        raise RuntimeError('short oracle response')
    return result


def blob(data):
    digest = hashlib.sha256(data).hexdigest()
    path = store / digest
    if path.exists():
        if path.read_bytes() != data:
            raise RuntimeError('artifact digest collision')
        return digest, len(data)
    with tempfile.NamedTemporaryFile(dir=scratch, prefix='blob-', delete=False) as temporary:
        temporary_path = pathlib.Path(temporary.name)
        try:
            temporary.write(data)
            temporary.flush()
            os.fsync(temporary.fileno())
        except BaseException:
            temporary_path.unlink()
            raise
    try:
        try:
            os.link(temporary_path, path)
        except FileExistsError:
            if path.read_bytes() != data:
                raise RuntimeError('artifact digest collision')
    finally:
        temporary_path.unlink()
    return digest, len(data)


def replace_lock(rows):
    lock = pathlib.Path('goldens.lock')
    lines = lock.read_text().splitlines()
    existing = [line for line in lines if not line.startswith('regex ')]
    if len(existing) != len(lines):
        raise RuntimeError('regex goldens already exist')
    content = '\n'.join(existing + rows) + '\n'
    with tempfile.NamedTemporaryFile(dir=scratch, prefix='lock-', delete=False) as temporary:
        temporary_path = pathlib.Path(temporary.name)
        try:
            temporary.write(content.encode())
            temporary.flush()
            os.fsync(temporary.fileno())
        except BaseException:
            temporary_path.unlink()
            raise
    try:
        os.replace(temporary_path, lock)
    finally:
        temporary_path.unlink(missing_ok=True)


def main():
    oracle = open_oracle()
    try:
        rows = []
        seen_inputs = set()
        for lane, count in (('seen', 1200), ('holdout', 640)):
            for request in corpus.cases(lane, count):
                if lane == 'holdout' and request in seen_inputs:
                    continue
                source = blob(request)
                result = blob(ask(oracle, request))
                if lane == 'seen':
                    seen_inputs.add(request)
                rows.append('regex {} regex-{}-{} {} {} {} {}'.format(
                    lane, lane, len(rows), source[0], source[1], result[0], result[1]))
        oracle.stdin.close()
        if oracle.wait():
            raise RuntimeError('oracle failed')
    finally:
        stop_oracle(oracle)
    replace_lock(rows)
    print('recorded', len(rows), 'regex goldens')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
