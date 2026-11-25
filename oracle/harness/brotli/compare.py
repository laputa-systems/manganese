import struct
import subprocess


def peer(path):
    process = subprocess.Popen([path], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    process.stdin.write(b'MGB1')
    process.stdin.flush()
    if process.stdout.read(4) != b'MGB1':
        raise RuntimeError('invalid peer handshake')
    return process


def ask(process, data):
    process.stdin.write(struct.pack('>I', len(data)) + data)
    process.stdin.flush()
    head = process.stdout.read(4)
    if len(head) != 4:
        raise RuntimeError('peer exited before response')
    length = struct.unpack('>I', head)[0]
    result = process.stdout.read(length)
    if len(result) != length:
        raise RuntimeError('short peer response')
    return result


def request(mode, output_size, dictionary, chunks):
    result = struct.pack('>BHII', mode, len(chunks), output_size, len(dictionary)) + dictionary
    return result + b''.join(struct.pack('>I', len(chunk)) + chunk for chunk in chunks)
