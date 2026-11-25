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
        units = []
        for _ in range(rng.randrange(80)):
            choice = rng.randrange(6)
            if choice == 0:
                units.extend((rng.randrange(0xd800, 0xdc00), rng.randrange(0xdc00, 0xe000)))
            elif choice == 1:
                units.append(rng.randrange(0xd800, 0xe000))
            else:
                units.append(rng.randrange(0xd800))
        data = b''.join(struct.pack('>H', unit) for unit in units)
        request = b'\x12' + data
        expected = ask(a, request)
        actual = ask(b, request)
        if expected != actual:
            raise SystemExit(f'divergence index={index} input={data.hex()} oracle={expected!r} candidate={actual!r}')
    print('Base UTF-16 conversion matched 20000 inputs')
finally:
    for proc in (a, b):
        proc.stdin.close()
        proc.wait()
