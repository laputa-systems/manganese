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
scratch = pathlib.Path('.work/tmp/webp')
scratch.mkdir(parents=True, exist_ok=True)


def open_oracle():
    process = subprocess.Popen(['.work/tmp/webp/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
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


def main():
    oracle = open_oracle()
    try:
        rows = []
        names = set()
        for lane in ('seen', 'holdout'):
            for name, data in corpus.lane_items(lane):
                item = f'{lane}-{name}'
                if item in names:
                    raise RuntimeError(f'duplicate corpus item {item}')
                names.add(item)
                source, result = blob(data), blob(ask(oracle, data))
                rows.append(f'webp {lane} {item} {source[0]} {source[1]} {result[0]} {result[1]}')
        oracle.stdin.close()
        if oracle.wait():
            raise RuntimeError('oracle failed')
    finally:
        stop_oracle(oracle)
    lock = pathlib.Path('goldens.lock')
    existing = [line for line in lock.read_text().splitlines() if not line.startswith('webp ')]
    with tempfile.NamedTemporaryFile(dir=scratch, prefix='lock-', delete=False) as temporary:
        temporary_path = pathlib.Path(temporary.name)
        try:
            temporary.write(('\n'.join(existing + rows) + '\n').encode())
            temporary.flush()
            os.fsync(temporary.fileno())
        except BaseException:
            temporary_path.unlink()
            raise
    try:
        os.replace(temporary_path, lock)
    finally:
        temporary_path.unlink(missing_ok=True)
    print('recorded', len(rows), 'seen', sum(' seen ' in row for row in rows), 'holdout', sum(' holdout ' in row for row in rows))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
