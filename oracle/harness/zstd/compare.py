import struct
import subprocess

def peer(path):
    process = subprocess.Popen([path], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    process.stdin.write(b'MGB1')
    process.stdin.flush()
    assert process.stdout.read(4) == b'MGB1'
    return process

def ask(process, data):
    process.stdin.write(struct.pack('>I', len(data)) + data)
    process.stdin.flush()
    head = process.stdout.read(4)
    if len(head) != 4:
        raise RuntimeError('adapter exited')
    length = struct.unpack('>I', head)[0]
    result = process.stdout.read(length)
    if len(result) != length:
        raise RuntimeError('adapter returned a truncated response')
    return result

def request(chunks, dictionary=b'', output_size=32768):
    return (struct.pack('>III', output_size, len(dictionary), len(chunks)) + dictionary +
            b''.join(struct.pack('>I', len(chunk)) + chunk for chunk in chunks))
