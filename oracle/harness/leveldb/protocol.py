import struct


def blob(value):
    return struct.pack('>I', len(value)) + value


def request(flags, files, operations):
    return (bytes([flags]) + struct.pack('>H', len(files)) +
            b''.join(blob(name.encode()) + blob(contents) for name, contents in files) +
            struct.pack('>H', len(operations)) + b''.join(operations))


def put(key, value, sync=False):
    return bytes([1, sync]) + blob(key) + blob(value)


def delete(key, sync=False):
    return bytes([2, sync]) + blob(key)


def batch(entries, sync=False):
    return bytes([3, sync]) + struct.pack('>H', len(entries)) + b''.join(
        bytes([op]) + blob(key) + (blob(value) if op == 1 else b'') for op, key, value in entries)


def get(key, snapshot=255):
    return bytes([4, snapshot]) + blob(key)


def snapshot(identifier):
    return bytes([5, identifier])


def release(identifier):
    return bytes([6, identifier])


def iterate(direction=0, key=b'', snapshot=255):
    return bytes([7, snapshot, direction]) + blob(key)


def compact(first=b'', last=b'\xff' * 16):
    return bytes([8]) + blob(first) + blob(last)


def truncate(name, length):
    return bytes([12]) + blob(name.encode()) + struct.pack('>Q', length)


OPEN = bytes([0])
CLOSE = bytes([9])
REPAIR = bytes([10])
DESTROY = bytes([11])
