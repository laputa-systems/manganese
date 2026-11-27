import hashlib
import os
import pathlib
import subprocess
import tempfile

import corpus

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)


def open_oracle():
    process = subprocess.Popen(['.work/tmp/structured-data/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
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
    process.stdin.write(corpus.u32(len(data)) + data)
    process.stdin.flush()
    size = process.stdout.read(4)
    if len(size) != 4:
        raise RuntimeError('oracle terminated')
    length = int.from_bytes(size, 'big')
    result = process.stdout.read(length)
    if len(result) != length:
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


def save(data):
    digest = hashlib.sha256(data).hexdigest()
    target = store / digest
    if target.exists():
        if target.read_bytes() != data:
            raise RuntimeError(f'corrupt stored blob {digest}')
        return digest, len(data)
    temporary = stage(data, store, 'blob-')
    try:
        try:
            os.link(temporary, target)
        except FileExistsError:
            if target.read_bytes() != data:
                raise RuntimeError(f'corrupt stored blob {digest}')
    finally:
        temporary.unlink()
    return digest, len(data)


def replace_lock(rows):
    lock = pathlib.Path('goldens.lock')
    lines = lock.read_text().splitlines()
    existing = [line for line in lines if not line.startswith('structured-data ')]
    if len(existing) != len(lines):
        raise RuntimeError('structured-data goldens already exist')
    temporary = stage(('\n'.join(existing + rows) + '\n').encode(), store, 'lock-')
    try:
        os.replace(temporary, lock)
    finally:
        temporary.unlink(missing_ok=True)


def main():
    oracle = open_oracle()
    try:
        rows = []
        seen_inputs = set()
        seen_outputs = set()
        for lane, seed in (('seen', 4103), ('holdout', 8719)):
            used = set()
            skipped = 0
            for name, data in corpus.cases(lane, seed):
                digest = hashlib.sha256(data).digest()
                if digest in used or (lane == 'holdout' and (digest in seen_inputs or digest in seen_outputs)):
                    skipped += 1
                    continue
                used.add(digest)
                source = save(data)
                result = save(ask(oracle, data))
                if lane == 'seen':
                    seen_inputs.add(digest)
                    seen_outputs.add(bytes.fromhex(result[0]))
                rows.append(f'structured-data {lane} {lane}-{name} {source[0]} {source[1]} {result[0]} {result[1]}')
            print(f'recorded {lane} skipped {skipped}', flush=True)
        oracle.stdin.close()
        if oracle.wait():
            raise RuntimeError('oracle failed')
    finally:
        stop_oracle(oracle)
    replace_lock(rows)
    print('recorded', len(rows), 'structured-data goldens')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
