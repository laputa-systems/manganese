import os
import struct
import subprocess

OPS = {
    'file': 0x01, 'open': 0x02, 'close': 0x03, 'execute': 0x04, 'prepare': 0x05, 'bind': 0x06,
    'step': 0x07, 'reset': 0x08, 'finalize': 0x09, 'begin': 0x0a, 'commit': 0x0b, 'rollback': 0x0c,
    'end_transaction': 0x0d, 'meta_init': 0x0e, 'meta_set_version': 0x0f, 'meta_get_version': 0x10,
    'meta_set_compatible': 0x11, 'meta_get_compatible': 0x12, 'meta_set_value': 0x13,
    'meta_get_value': 0x14, 'meta_delete_key': 0x15, 'meta_raze_if_incompatible': 0x16, 'raze': 0x17,
    'raze_and_poison': 0x18, 'poison': 0x19, 'vacuum': 0x1a, 'checkpoint': 0x1b, 'exists': 0x1c,
    'changes': 0x1d, 'dump': 0x1e, 'artifacts': 0x1f, 'digest': 0x20, 'residue': 0x21, 'crash': 0x22,
    'export': 0x23, 'sql_valid': 0x24,
}

WAL = 0x01
SHARED_LOCKING = 0x02
READ_ONLY = 0x04
NO_MMAP = 0x08
TRIGGERS = 0x10
VIEWS = 0x20
NO_SYNC = 0x40
MMAP_ALT_STATUS = 0x80

DATABASE, JOURNAL, WAL_FILE = 0, 1, 2
GETTERS = {'auto': 0, 'int64': 1, 'double': 2, 'string': 3, 'blob': 4, 'int': 5, 'bool': 6}


def text(value):
    data = value.encode() if isinstance(value, str) else bytes(value)
    return struct.pack('>I', len(data)) + data


def value(item):
    if item is None:
        return b'\x00'
    if isinstance(item, bool):
        return b'\x01' + struct.pack('>q', int(item))
    if isinstance(item, int):
        return b'\x01' + struct.pack('>q', item)
    if isinstance(item, float):
        return b'\x02' + struct.pack('>d', item)
    if isinstance(item, str):
        return b'\x03' + text(item)
    return b'\x04' + text(item)


class Script:
    def __init__(self):
        self.steps = []

    def add(self, op, payload=b''):
        self.steps.append(bytes([OPS[op]]) + payload)
        return self

    def file(self, kind, data):
        return self.add('file', bytes([kind]) + text(data))

    def open(self, flags=0, page_size=4096, cache_size=0):
        return self.add('open', bytes([flags]) + struct.pack('>Ii', page_size, cache_size))

    def close(self):
        return self.add('close')

    def execute(self, sql):
        return self.add('execute', text(sql))

    def prepare(self, slot, sql):
        return self.add('prepare', bytes([slot]) + text(sql))

    def bind(self, slot, index, item):
        return self.add('bind', bytes([slot]) + struct.pack('>H', index) + value(item))

    def step(self, slot, max_rows=65535, getters=()):
        codes = bytes(GETTERS[name] for name in getters)
        return self.add('step', bytes([slot]) + struct.pack('>HB', max_rows, len(codes)) + codes)

    def reset(self, slot, clear_bindings=True):
        return self.add('reset', bytes([slot, int(clear_bindings)]))

    def finalize(self, slot):
        return self.add('finalize', bytes([slot]))

    def begin(self, slot=0):
        return self.add('begin', bytes([slot]))

    def commit(self, slot=0):
        return self.add('commit', bytes([slot]))

    def rollback(self, slot=0):
        return self.add('rollback', bytes([slot]))

    def end_transaction(self, slot=0):
        return self.add('end_transaction', bytes([slot]))

    def meta_init(self, version, compatible):
        return self.add('meta_init', struct.pack('>ii', version, compatible))

    def meta_set_version(self, version):
        return self.add('meta_set_version', struct.pack('>i', version))

    def meta_get_version(self):
        return self.add('meta_get_version')

    def meta_set_compatible(self, version):
        return self.add('meta_set_compatible', struct.pack('>i', version))

    def meta_get_compatible(self):
        return self.add('meta_get_compatible')

    def meta_set_value(self, key, item):
        if isinstance(item, int):
            return self.add('meta_set_value', text(key) + b'\x01' + struct.pack('>q', item))
        return self.add('meta_set_value', text(key) + b'\x03' + text(item))

    def meta_get_value(self, key, kind):
        return self.add('meta_get_value', text(key) + (b'\x01' if kind == 'int' else b'\x03'))

    def meta_delete_key(self, key):
        return self.add('meta_delete_key', text(key))

    def meta_raze_if_incompatible(self, lowest, current):
        return self.add('meta_raze_if_incompatible', struct.pack('>ii', lowest, current))

    def raze(self):
        return self.add('raze')

    def raze_and_poison(self):
        return self.add('raze_and_poison')

    def poison(self):
        return self.add('poison')

    def vacuum(self):
        return self.add('vacuum')

    def checkpoint(self, restart=False):
        return self.add('checkpoint', bytes([int(restart)]))

    def exists(self, kind, name='', column=None):
        kinds = {'table': 0, 'index': 1, 'view': 2, 'column': 3, 'meta': 4}
        payload = bytes([kinds[kind]]) + text(name)
        if kind == 'column':
            payload += text(column)
        return self.add('exists', payload)

    def changes(self):
        return self.add('changes')

    def dump(self):
        return self.add('dump')

    def artifacts(self):
        return self.add('artifacts')

    def digest(self):
        return self.add('digest')

    def residue(self, pattern):
        return self.add('residue', text(pattern))

    def crash(self, drop_sidecars=False):
        return self.add('crash', bytes([int(drop_sidecars)]))

    def export(self):
        return self.add('export')

    def sql_valid(self, sql):
        return self.add('sql_valid', text(sql))

    def encode(self):
        if len(self.steps) > 0xffff:
            raise ValueError('too many steps')
        return struct.pack('>H', len(self.steps)) + b''.join(self.steps)


def frame(data):
    return struct.pack('>I', len(data)) + data


def run(program, requests, scratch, timeout=300):
    os.makedirs(scratch, exist_ok=True)
    payload = b'MGB1' + b''.join(frame(request) for request in requests)
    result = subprocess.run([program], input=payload, capture_output=True, timeout=timeout,
                            env=dict(os.environ, GOLDENS_SCRATCH=scratch))
    if result.returncode != 0:
        raise RuntimeError(f'{program} exited {result.returncode}: {result.stderr.decode(errors="replace")}')
    out = result.stdout
    if out[:4] != b'MGB1':
        raise RuntimeError('missing MGB1 header')
    responses, offset = [], 4
    while offset < len(out):
        (length,) = struct.unpack_from('>I', out, offset)
        responses.append(out[offset + 4:offset + 4 + length])
        offset += 4 + length
    if len(responses) != len(requests):
        raise RuntimeError('response count mismatch')
    return responses


NAMES = {code: name for name, code in OPS.items()}


class Cursor:
    def __init__(self, data):
        self.data, self.pos = data, 0

    def take(self, count):
        if self.pos + count > len(self.data):
            raise ValueError('truncated')
        chunk = self.data[self.pos:self.pos + count]
        self.pos += count
        return chunk

    def u8(self):
        return self.take(1)[0]

    def u16(self):
        return struct.unpack('>H', self.take(2))[0]

    def u32(self):
        return struct.unpack('>I', self.take(4))[0]

    def i32(self):
        return struct.unpack('>i', self.take(4))[0]

    def i64(self):
        return struct.unpack('>q', self.take(8))[0]

    def blob(self):
        return self.take(self.u32())


def read_value(cursor):
    tag = cursor.u8()
    if tag == 0:
        return None
    if tag == 1:
        return cursor.i64()
    if tag == 2:
        return struct.unpack('>d', cursor.take(8))[0]
    if tag == 3:
        return cursor.blob().decode(errors='backslashreplace')
    return cursor.blob()


def read_rows(cursor, count):
    rows = []
    for _ in range(count):
        rows.append([read_value(cursor) for _ in range(cursor.u16())])
    return rows


def read_query(cursor):
    ok = cursor.u8()
    count = cursor.u32()
    return ok, read_rows(Cursor(cursor.blob()), count)


def decode(request, response):
    req, res = Cursor(request), Cursor(response)
    out = []
    for _ in range(req.u16()):
        op = req.u8()
        name = NAMES[op]
        if res.u8() != op:
            raise ValueError(f'response op mismatch at {name}')
        if name == 'file':
            req.u8(); req.blob(); item = res.u8()
        elif name == 'open':
            req.take(9); item = (res.u8(), res.i32())
        elif name in ('close', 'finalize', 'end_transaction', 'poison'):
            req.take({'close': 0, 'finalize': 1, 'end_transaction': 1, 'poison': 0}[name]); item = res.u8()
        elif name == 'execute':
            item = (req.blob()[:60], res.u8(), res.i32())
        elif name == 'prepare':
            req.u8(); item = (req.blob()[:60], res.u8(), res.i32())
        elif name == 'bind':
            req.u8(); req.u16(); read_value(req); item = res.u8()
        elif name == 'step':
            req.u8(); req.u16(); req.take(req.u8())
            count = res.u32()
            item = (read_rows(Cursor(res.blob()), count), res.u8(), res.u8(), res.i32())
        elif name == 'reset':
            req.take(2); item = res.u8()
        elif name in ('begin', 'commit'):
            req.u8(); item = (res.u8(), res.i32())
        elif name == 'rollback':
            req.u8(); item = res.u8()
        elif name == 'meta_init':
            req.take(8); item = (res.u8(), res.i32())
        elif name in ('meta_set_version', 'meta_set_compatible'):
            req.take(4); item = (res.u8(), res.i32())
        elif name in ('meta_get_version', 'meta_get_compatible'):
            item = (res.u8(), res.i32())
        elif name == 'meta_set_value':
            req.blob(); tag = req.u8(); req.take(8) if tag == 1 else req.blob(); item = (res.u8(), res.i32())
        elif name == 'meta_get_value':
            req.blob(); tag = req.u8(); item = (res.u8(), res.i64() if tag == 1 else res.blob())
        elif name == 'meta_delete_key':
            req.blob(); item = (res.u8(), res.i32())
        elif name == 'meta_raze_if_incompatible':
            req.take(8); item = (res.u8(), res.u8())
        elif name in ('raze', 'vacuum'):
            item = (res.u8(), res.i32())
        elif name == 'raze_and_poison':
            item = res.u8()
        elif name == 'checkpoint':
            req.u8(); item = res.u8()
        elif name == 'exists':
            kind = req.u8()
            req.blob()
            if kind == 3:
                req.blob()
            item = res.u8()
        elif name == 'changes':
            item = (res.u8(), res.i64(), res.i64())
        elif name == 'dump':
            res.u8()
            item = {'page_size': read_query(res), 'user_version': read_query(res), 'application_id': read_query(res),
                    'schema': read_query(res), 'tables': {}}
            for _ in range(res.u32()):
                table = res.blob().decode(errors='backslashreplace')
                item['tables'][table] = read_query(res)
        elif name == 'artifacts':
            res.u8(); item = [(res.u8(), res.i64()) for _ in range(3)]
        elif name == 'digest':
            item = (res.u8(), res.blob().hex()[:16])
        elif name == 'residue':
            req.blob(); item = (res.u8(), res.u8())
        elif name == 'crash':
            req.u8(); item = res.u8()
        elif name == 'export':
            res.u8(); item = [(res.u8(), res.blob()) for _ in range(3)]
        elif name == 'sql_valid':
            req.blob(); item = res.u8()
        out.append((name, item))
    if res.pos != len(response):
        raise ValueError('trailing response bytes')
    return out


def compare(request, expected, actual):
    left, right = decode(request, expected), decode(request, actual)
    for index, (a, b) in enumerate(zip(left, right)):
        if a != b:
            return index, a, b
    return None
