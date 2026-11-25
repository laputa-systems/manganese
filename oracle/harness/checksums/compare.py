import struct
import subprocess

def peer(path):
    process = subprocess.Popen([path], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    process.stdin.write(b'MGB1')
    process.stdin.flush()
    if process.stdout.read(4) != b'MGB1': raise ValueError('invalid peer header')
    return process

def ask(process, data):
    process.stdin.write(struct.pack('>I', len(data)) + data)
    process.stdin.flush()
    header = process.stdout.read(4)
    if len(header) != 4: raise ValueError('short peer response')
    length = struct.unpack('>I', header)[0]
    result = process.stdout.read(length)
    if len(result) != length: raise ValueError('short peer response')
    return result

def request(op, offset, seed, chunks):
    return struct.pack('>BBHI', op, offset, len(chunks), seed) + b''.join(struct.pack('>I', len(data)) + data for data in chunks)
