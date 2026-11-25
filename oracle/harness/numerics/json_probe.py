import random
import struct
import subprocess


def peer(path: str) -> subprocess.Popen:
    proc = subprocess.Popen([path], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    proc.stdin.write(b'MGB1')
    proc.stdin.flush()
    if proc.stdout.read(4) != b'MGB1':
        raise RuntimeError('invalid peer header')
    return proc


def ask(proc: subprocess.Popen, data: bytes) -> bytes:
    proc.stdin.write(struct.pack('>I', len(data)) + data)
    proc.stdin.flush()
    header = proc.stdout.read(4)
    if len(header) != 4:
        raise RuntimeError('short peer response')
    result = proc.stdout.read(struct.unpack('>I', header)[0])
    if len(result) != struct.unpack('>I', header)[0]:
        raise RuntimeError('short peer response')
    return result


a = peer('.work/tmp/numerics/oracle')
b = peer('.work/tmp/target/x86_64-unknown-linux-musl/debug/numerics')
rng = random.Random(42)
try:
    for index in range(20000):
        alphabet = b'0123456789+-.eE x\t\n'
        data = bytes(rng.choice(alphabet) for _ in range(rng.randrange(0, 80)))
        if index % 2:
            data = str(rng.randrange(-10**30, 10**30)).encode() + b'.' + str(rng.randrange(10**18)).encode() + b'e' + str(rng.randrange(-400, 401)).encode() + data[:rng.randrange(5)]
        request = b'\x0c' + data
        expected = ask(a, request)
        actual = ask(b, request)
        if expected != actual:
            raise SystemExit(f'divergence index={index} input={data!r} oracle={expected!r} candidate={actual!r}')
    print('JSON reader matched 20000 inputs')
finally:
    for proc in (a, b):
        proc.stdin.close()
        proc.wait()
