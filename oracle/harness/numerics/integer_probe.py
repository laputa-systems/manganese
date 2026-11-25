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
        alphabet = b'0123456789abcdefABCDEF+-xX junk\t\n'
        data = bytes(rng.choice(alphabet) for _ in range(rng.randrange(0, 35)))
        if index % 2:
            value = rng.randrange(-(1 << 70), 1 << 70)
            data = str(value).encode() if index % 4 else format(value, 'x').encode()
            data += rng.choice((b'', b'', b'!', b' '))
        request = b'\x07' + bytes([index % 8]) + data
        expected = ask(a, request)
        actual = ask(b, request)
        if expected != actual:
            raise SystemExit(f'divergence index={index} input={request!r} oracle={expected!r} candidate={actual!r}')
    print('integer matched 20000 inputs')
finally:
    for proc in (a, b):
        proc.stdin.close()
        proc.wait()
