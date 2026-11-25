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
        bits = rng.getrandbits(64)
        request = struct.pack('>BQH', 10 if index % 2 else 11, bits, 1 + rng.randrange(20))
        expected = ask(a, request)
        actual = ask(b, request)
        if expected != actual:
            raise SystemExit(f'divergence index={index} bits={bits:016x} oracle={expected!r} candidate={actual!r}')
    print('Blink wrapper matched 20000 random values')
finally:
    for proc in (a, b):
        proc.stdin.close()
        proc.wait()
