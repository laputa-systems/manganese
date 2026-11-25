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
        if index % 3 == 0:
            data = bytes(rng.randrange(256) for _ in range(rng.randrange(80)))
        elif index % 3 == 1:
            data = ''.join(chr(rng.choice((rng.randrange(0xD800), rng.randrange(0xE000, 0x110000)))) for _ in range(rng.randrange(20))).encode()
        else:
            data = rng.choice((b'\x80', b'\xc0\x80', b'\xed\xa0\x80', b'\xf4\x90\x80\x80', b'\xef\xb7\x90', b'\xef\xbf\xbe', b'\xf0\x90\x80'))
        request = b'\x11' + data
        expected = ask(a, request)
        actual = ask(b, request)
        if expected != actual:
            raise SystemExit(f'divergence index={index} input={data.hex()} oracle={expected!r} candidate={actual!r}')
    print('Base UTF-8 conversion matched 20000 inputs')
finally:
    for proc in (a, b):
        proc.stdin.close()
        proc.wait()
