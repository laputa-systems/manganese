import struct
import subprocess

oracle = '.work/tmp/compression-misc/oracle'
candidate = '.work/tmp/target/x86_64-unknown-linux-musl/debug/compression-misc'


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


def unwrap(response):
    assert response[0] in (0, 1)
    size = struct.unpack('>I', response[1:5])[0]
    assert len(response) == 5 + size
    return response[0], response[5:]


def u32(value):
    return struct.pack('>I', value)


def compress_request(data):
    return b'\x00' + u32(len(data)) + data


def decode_request(stream, limit):
    return b'\x01' + u32(limit) + u32(len(stream)) + stream
