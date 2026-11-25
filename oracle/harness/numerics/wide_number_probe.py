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
        alphabet = [*b'0123456789abcdefABCDEFxX+-.eE !\t', 0x85, 0xa0, 0x1680, 0x180e, 0x2000, 0x2028, 0x205f, 0x3000, 0xfeff, 0xd800]
        if index % 2:
            units = list(str(rng.randrange(-(1 << 70), 1 << 70)).encode())
            units += [rng.choice(alphabet) for _ in range(rng.randrange(5))]
        else:
            units = [rng.choice(alphabet) for _ in range(rng.randrange(35))]
        data = b''.join(struct.pack('>H', unit) for unit in units)
        request = b'\x1a' + data if index % 4 < 2 else b'\x1b' + bytes([index % 8]) + data
        expected = ask(a, request)
        actual = ask(b, request)
        if expected != actual:
            raise SystemExit(f'divergence index={index} input={request.hex()} oracle={expected!r} candidate={actual!r}')
    print('wide numeric parse matched 20000 inputs')
finally:
    for proc in (a, b):
        proc.stdin.close()
        proc.wait()
