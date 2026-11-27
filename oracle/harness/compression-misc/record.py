import hashlib
import os
import pathlib
import subprocess
import tempfile

import compare
import corpus

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)


def open_oracle():
    process = subprocess.Popen(['.work/tmp/compression-misc/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    try:
        process.stdin.write(b'MGB1')
        process.stdin.flush()
        if process.stdout.read(4) != b'MGB1':
            raise RuntimeError('oracle protocol header')
    except BaseException:
        process.kill()
        process.wait()
        raise
    return process


def compress(oracle, data):
    status, output = compare.unwrap(compare.ask(oracle, corpus.compress_request(data)))
    if status != 0:
        raise RuntimeError('oracle compression failed')
    return output


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
    existing = [line for line in lines if not line.startswith('compression-misc ')]
    if len(existing) != len(lines):
        raise RuntimeError('compression-misc goldens already exist')
    temporary = stage(('\n'.join(existing + rows) + '\n').encode(), store, 'lock-')
    try:
        os.replace(temporary, lock)
    finally:
        temporary.unlink(missing_ok=True)


def main():
    oracle = open_oracle()
    rows = []
    seen_inputs = set()
    seen_outputs = set()
    try:
        for lane, seed in (('seen', 7211), ('holdout', 8719)):
            used = set()
            skipped = 0
            for name, request in corpus.cases(lane, seed, lambda data: compress(oracle, data)):
                digest = hashlib.sha256(request).digest()
                if digest in used or (lane == 'holdout' and (digest in seen_inputs or digest in seen_outputs)):
                    skipped += 1
                    continue
                used.add(digest)
                source = save(request)
                status, output = compare.unwrap(compare.ask(oracle, request))
                if status != 0 and name.endswith(('roundtrip', 'limit-plus')):
                    raise RuntimeError(f'oracle rejected {lane}-{name}')
                response = bytes([status]) + len(output).to_bytes(4, 'big') + output
                result = save(response)
                if lane == 'seen':
                    seen_inputs.add(digest)
                    seen_outputs.add(hashlib.sha256(response).digest())
                rows.append(f'compression-misc {lane} {lane}-{name} {source[0]} {source[1]} {result[0]} {result[1]}')
            print(f'recorded {lane} skipped {skipped}', flush=True)
        oracle.stdin.close()
        if oracle.wait():
            raise RuntimeError('oracle failed')
    finally:
        if oracle.poll() is None:
            oracle.kill()
            oracle.wait()
    replace_lock(rows)
    print('recorded', len(rows), 'compression-misc goldens')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
