import struct
import subprocess


def blob(data):
    return struct.pack('>I', len(data)) + data


def request_label(label, web=False):
    return bytes((5 if web else 0,)) + blob(label)


def request_decode(label, mode, fatal, chunks):
    return b'\x01' + blob(label) + bytes((mode, fatal)) + struct.pack('>I', len(chunks)) + b''.join(blob(chunk) for chunk in chunks)


def request_encode(label, units):
    return b'\x02' + blob(label) + blob(units)


def request_batch(items):
    return b'\x04' + struct.pack('>I', len(items)) + b''.join(blob(item) for item in items)


def peer(path):
    process = subprocess.Popen([path], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    process.stdin.write(b'MGB1')
    process.stdin.flush()
    if process.stdout.read(4) != b'MGB1':
        raise ValueError('invalid peer header')
    return process


def ask(process, data):
    process.stdin.write(blob(data))
    process.stdin.flush()
    header = process.stdout.read(4)
    if len(header) != 4:
        raise ValueError('short peer response')
    length = struct.unpack('>I', header)[0]
    result = process.stdout.read(length)
    if len(result) != length:
        raise ValueError('short peer response')
    return result
