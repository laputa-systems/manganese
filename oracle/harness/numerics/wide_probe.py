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
        op = 19 + index % 4
        if op == 19:
            data = rng.randbytes(rng.randrange(70))
        elif op in (20, 21):
            values = [rng.choice((rng.randrange(0x110000), rng.getrandbits(32), 0xd800, 0x10ffff, 0x110000)) for _ in range(rng.randrange(40))]
            data = b''.join(struct.pack('>I', value) for value in values)
        else:
            data = b''.join(struct.pack('>H', rng.randrange(65536)) for _ in range(rng.randrange(70)))
        request = bytes([op]) + data
        expected = ask(a, request)
        actual = ask(b, request)
        if expected != actual:
            raise SystemExit(f'divergence index={index} input={request.hex()} oracle={expected!r} candidate={actual!r}')
    print('wide conversion matched 20000 inputs')
finally:
    for proc in (a, b):
        proc.stdin.close()
        proc.wait()
