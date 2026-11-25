import struct
import subprocess

oracle = '.work/tmp/zlib/oracle'
candidate = '.work/tmp/target/x86_64-unknown-linux-musl/debug/zlib'

def peer(path):
    process = subprocess.Popen([path], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    process.stdin.write(b'MGB1')
    process.stdin.flush()
    assert process.stdout.read(4) == b'MGB1'
    return process

def ask(process, data):
    process.stdin.write(struct.pack('>I', len(data)) + data)
    process.stdin.flush()
    length = struct.unpack('>I', process.stdout.read(4))[0]
    result = process.stdout.read(length)
    assert len(result) == length
    return result

def request(op, level, window, mem, strategy, chunks):
    result = bytes((op, level & 255, window & 255, mem, strategy)) + struct.pack('>H', len(chunks))
    for data, flush in chunks:
        result += struct.pack('>IB', len(data), flush) + data
    return result

if __name__ == '__main__':
    a, b = peer(oracle), peer(candidate)
    inputs = [b'', b'a', b'hello world' * 100, bytes(range(256)) * 20, b'a' * 5000, (b'abracadabra' * 1000 + b'xyz' * 100)]
    for level in range(10):
        matches = 0
        for data in inputs:
            item = request(0, level, 15, 8, 0, [(data, 4)])
            matches += ask(a, item) == ask(b, item)
        print('level', level, 'matches', matches, '/', len(inputs), flush=True)
    for p in (a, b):
        p.stdin.close()
        assert p.wait() == 0
