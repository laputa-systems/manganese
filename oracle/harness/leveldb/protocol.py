import struct

PARANOID, REUSE_LOGS, SNAPPY, BLOOM, CREATE, EXCLUSIVE = 1, 2, 4, 8, 16, 32
VERIFY, FILL = 1, 2
LOG, TABLE, MANIFEST, CURRENT, TEMP = 0, 1, 2, 3, 4
DATABASE, REBUILD = 0, 1
NO_SNAPSHOT = 255
FIRST, LAST, NEXT, PREV, DRAIN, DRAIN_BACK = 0, 1, 3, 4, 5, 6
DEFAULT_WRITE_BUFFER = 4 << 20
DEFAULT_OPEN_FILES = 1000


def blob(value):
    return struct.pack('>I', len(value)) + value


def request(flags, operations, files=(), write_buffer=DEFAULT_WRITE_BUFFER, open_files=DEFAULT_OPEN_FILES):
    return (bytes([flags]) + struct.pack('>II', write_buffer, open_files) + struct.pack('>H', len(files)) +
            b''.join(bytes([directory]) + blob(name.encode()) + blob(contents) for directory, name, contents in files) +
            struct.pack('>H', len(operations)) + b''.join(operations))


def put(key, value, sync=False):
    return bytes([1, sync]) + blob(key) + blob(value)


def delete(key, sync=False):
    return bytes([2, sync]) + blob(key)


def batch(entries, sync=False):
    return bytes([3, sync]) + struct.pack('>H', len(entries)) + b''.join(
        bytes([1]) + blob(key) + blob(value) if value is not None else bytes([2]) + blob(key) for key, value in entries)


def get(key, snapshot=NO_SNAPSHOT, read=FILL):
    return bytes([4, read, snapshot]) + blob(key)


def snapshot(identifier):
    return bytes([5, identifier])


def release(identifier):
    return bytes([6, identifier])


def seek(key):
    return bytes([2]) + blob(key)


def iterate(steps=(bytes([FIRST]), bytes([DRAIN])), snapshot=NO_SNAPSHOT, read=FILL):
    steps = [bytes([step]) if isinstance(step, int) else step for step in steps]
    return bytes([7, read, snapshot, len(steps)]) + b''.join(steps)


def compact(begin=None, end=None):
    return (bytes([8, (begin is not None) | (end is not None) << 1]) + (blob(begin) if begin is not None else b'') +
            (blob(end) if end is not None else b''))


def truncate(kind, rank, keep):
    return bytes([12, kind, rank]) + struct.pack('>Q', keep)


def corrupt(kind, rank, offset, mask):
    return bytes([13, kind, rank]) + struct.pack('>Q', offset) + blob(mask)


def remove(kind, rank):
    return bytes([14, kind, rank])


OPEN = bytes([0])
CLOSE = bytes([9])
REPAIR = bytes([10])
DESTROY = bytes([11])
REWRITE = bytes([15])
