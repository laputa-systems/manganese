"""Deterministic corpus scenarios for the leveldb box.

Each scenario yields (name, request). Scenarios that read a directory another run wrote ask the
oracle for it through EXPORT, so fixture bytes always come from Chromium's LevelDB.
"""

import random

from protocol import (BLOOM, CLOSE, CREATE, CURRENT, DATABASE, DESTROY, DRAIN, DRAIN_BACK, EXCLUSIVE, EXPORT,
                      FILL, FIRST, LAST, LOG, MANIFEST, NEXT, NO_SNAPSHOT, OPEN, PARANOID, PREV, REBUILD, REPAIR,
                      REUSE_LOGS, REWRITE, SNAPPY, TABLE, VERIFY, batch, compact, corrupt, delete, exported, get,
                      iterate, put, release, remove, request, seek, snapshot, truncate)

DEFAULT = REUSE_LOGS | SNAPPY
CALLERS = {
    'dom-storage': (DEFAULT | CREATE, 64 << 10, 0),
    'indexed-db': (DEFAULT | CREATE | PARANOID | BLOOM, 4 << 20, 80),
    'sync': (DEFAULT | CREATE | PARANOID, 512 << 10, 1000),
    'gcm': (DEFAULT | CREATE | PARANOID, 128 << 10, 1000),
    'value-store': (DEFAULT | CREATE | PARANOID, 4 << 20, 1000),
    'proto': (DEFAULT | CREATE, 4 << 20, 1000),
    'file-system': (DEFAULT | CREATE, 4 << 20, 0),
    'repair': (SNAPPY | CREATE, 4 << 20, 0),
    'repair-paranoid': (SNAPPY | CREATE | PARANOID, 4 << 20, 1000),
    'plain': (CREATE, 4 << 20, 1000),
}
READ_ALL = iterate()
READ_BACK = iterate([LAST, DRAIN_BACK])
CHECKED = iterate(read=VERIFY | FILL)


def caller(name, extra=0, drop=0):
    flags, write_buffer, open_files = CALLERS[name]
    return (flags | extra) & ~drop, write_buffer, open_files


def make(options, operations, files=()):
    flags, write_buffer, open_files = options
    return request(flags, operations, files, write_buffer, open_files)


def key_set(rng, count, sizes=(1, 4, 8, 16, 40)):
    keys = set()
    while len(keys) < count:
        keys.add(rng.randbytes(rng.choice(sizes)))
    return sorted(keys)


def value(rng, size):
    if size == 0:
        return b''
    kind = rng.randrange(3)
    if kind == 0:
        return rng.randbytes(size)
    if kind == 1:
        return bytes([rng.randrange(256)]) * size
    words = [b'alpha', b'beta', b'local-storage', b'indexed', b'\x00\x01', b'record']
    return b''.join(rng.choice(words) for _ in range(size // 4 + 1))[:size]


def random_operations(rng, keys, count, snapshots=True, large=False):
    ops, held = [], set()
    sizes = (0, 1, 8, 100, 1000, 5000) + ((40000, 120000) if large else ())
    for _ in range(count):
        choice = rng.randrange(100)
        key = rng.choice(keys)
        if choice < 30:
            ops.append(put(key, value(rng, rng.choice(sizes)), rng.randrange(6) == 0))
        elif choice < 40:
            ops.append(delete(key, rng.randrange(6) == 0))
        elif choice < 50:
            entries = [(rng.choice(keys), None if rng.randrange(3) == 0 else value(rng, rng.choice(sizes[:4])))
                       for _ in range(rng.randrange(8))]
            ops.append(batch(entries, rng.randrange(6) == 0))
        elif choice < 62:
            ops.append(get(key, rng.choice(sorted(held)) if held and rng.randrange(2) else NO_SNAPSHOT,
                           rng.choice((FILL, VERIFY | FILL, VERIFY, 0))))
        elif choice < 68 and snapshots:
            identifier = rng.randrange(4)
            held.add(identifier)
            ops.append(snapshot(identifier))
        elif choice < 71 and snapshots:
            identifier = rng.randrange(4)
            held.discard(identifier)
            ops.append(release(identifier))
        elif choice < 85:
            ops.append(iterate(movements(rng, keys), rng.choice(sorted(held)) if held and rng.randrange(2) else NO_SNAPSHOT,
                               rng.choice((FILL, VERIFY | FILL))))
        elif choice < 90:
            bounds = [rng.choice(keys) for _ in range(2)]
            ops.append(compact(*[(bound if rng.randrange(3) else None) for bound in sorted(bounds)]))
        elif choice < 97:
            ops.extend((CLOSE, OPEN))
            held.clear()
        else:
            ops.append(REWRITE)
            held.clear()
    return ops


def movements(rng, keys):
    steps = [rng.choice((bytes([FIRST]), bytes([LAST]), seek(rng.choice(keys)), seek(rng.randbytes(rng.randrange(6)))))]
    for _ in range(rng.randrange(1, 10)):
        steps.append(rng.choice((bytes([NEXT]), bytes([NEXT]), bytes([PREV]), seek(rng.choice(keys)), bytes([FIRST]),
                                 bytes([LAST]))))
    if rng.randrange(3) == 0:
        steps.append(bytes([rng.choice((DRAIN, DRAIN_BACK))]))
    return steps


def sequences(rng, count):
    for index in range(count):
        name = rng.choice(sorted(CALLERS))
        keys = key_set(rng, rng.randrange(2, 30))
        ops = [OPEN] + random_operations(rng, keys, rng.randrange(5, 60), large=rng.randrange(4) == 0)
        yield f'sequence-{name}-{index}', make(caller(name), ops + [READ_ALL, READ_BACK, CLOSE, OPEN, CHECKED, CLOSE])


def sizes(rng):
    key_sizes = (0, 1, 8, 255, 4096, 65536)
    value_sizes = (0, 1, 100, 4096, 32760, 32768, 100000, 1 << 20)
    for key_size in key_sizes:
        for value_size in value_sizes:
            if key_size * value_size > (64 << 20):
                continue
            key = rng.randbytes(key_size)
            data = value(rng, value_size)
            name = rng.choice(('dom-storage', 'indexed-db', 'plain', 'sync'))
            ops = [OPEN, put(key, data, rng.randrange(2) == 1), get(key), CLOSE, OPEN, get(key), compact(), get(key),
                   READ_ALL, put(key + b'\x00', data[:7]), READ_BACK, CLOSE, OPEN, CHECKED, CLOSE]
            yield f'size-key-{key_size}-value-{value_size}', make(caller(name), ops)


def batches(rng):
    keys = key_set(rng, 12)
    yield 'batch-empty', make(caller('plain'), [OPEN, batch([]), batch([], True), READ_ALL, CLOSE, OPEN, READ_ALL])
    for index in range(6):
        entries = [(rng.choice(keys), None if rng.randrange(3) == 0 else value(rng, rng.randrange(200)))
                   for _ in range(rng.randrange(1, 300))]
        ops = [OPEN, put(keys[0], b'before'), batch(entries, index % 2 == 0), READ_ALL, get(keys[0]), CLOSE, OPEN,
               READ_BACK, CLOSE]
        yield f'batch-mixed-{index}', make(caller(('dom-storage', 'gcm', 'indexed-db')[index % 3]), ops)
    large = [(b'large-%05d' % i, value(rng, 2000)) for i in range(3000)]
    for name in ('dom-storage', 'sync', 'plain'):
        ops = [OPEN, batch(large[:1500]), batch(large[1500:], True), get(b'large-00007'), get(b'large-02999'),
               iterate([seek(b'large-01'), NEXT, NEXT, PREV, PREV, PREV]), CLOSE, OPEN, get(b'large-01500'), CHECKED,
               CLOSE]
        yield f'batch-large-{name}', make(caller(name), ops)


def iteration(rng):
    for index in range(16):
        keys = key_set(rng, rng.randrange(5, 60), sizes=(0, 1, 2, 3, 8))
        name = rng.choice(('dom-storage', 'indexed-db', 'plain'))
        ops = [OPEN]
        for key in keys:
            ops.append(put(key, value(rng, rng.randrange(50))))
        layout = index % 4
        if layout >= 1:
            ops.append(compact())
        for key in rng.sample(keys, len(keys) // 3):
            ops.append(delete(key) if rng.randrange(2) else put(key, value(rng, 9)))
        if layout == 2:
            ops.extend((CLOSE, OPEN))
        if layout == 3:
            ops.append(compact(keys[0], keys[len(keys) // 2]))
        ops.append(snapshot(1))
        for key in rng.sample(keys, len(keys) // 4):
            ops.append(delete(key) if rng.randrange(2) else put(key, value(rng, 5)))
        for _ in range(8):
            ops.append(iterate(movements(rng, keys)))
            ops.append(iterate(movements(rng, keys), 1))
        ops.extend((READ_ALL, READ_BACK, iterate([seek(keys[-1] + b'\xff'), PREV, PREV, NEXT]),
                    iterate([LAST, NEXT, PREV, FIRST, PREV, NEXT]), CLOSE))
        yield f'iterate-{index}', make(caller(name), ops)


def snapshots(rng):
    for index in range(8):
        keys = key_set(rng, 10)
        name = rng.choice(('indexed-db', 'dom-storage', 'plain'))
        ops = [OPEN] + [put(key, b'v1-' + key) for key in keys] + [snapshot(0)]
        ops += [put(key, b'v2-' + key) for key in keys[::2]] + [delete(key) for key in keys[1::3]] + [snapshot(1)]
        ops += [batch([(key, None) for key in keys[::4]] + [(keys[-1], b'v3')]), snapshot(2)]
        if index % 2:
            ops.append(compact())
        for identifier in (0, 1, 2, NO_SNAPSHOT):
            ops += [get(key, identifier) for key in keys] + [iterate(snapshot=identifier),
                                                              iterate([LAST, DRAIN_BACK], identifier)]
        ops += [release(0), compact(), get(keys[0], 1), iterate(snapshot=1), release(1), release(2), compact(),
                READ_ALL, CLOSE, OPEN, READ_ALL, CLOSE]
        yield f'snapshot-{index}', make(caller(name), ops)


def compaction(rng):
    for index in range(10):
        name = ('dom-storage', 'gcm', 'sync')[index % 3]
        keys = [b'k%06d' % i for i in range(rng.randrange(200, 2000))]
        ops = [OPEN]
        for round_ in range(rng.randrange(2, 6)):
            entries = [(key, value(rng, rng.randrange(100, 1500))) for key in rng.sample(keys, len(keys) // 2)]
            for start in range(0, len(entries), 200):
                ops.append(batch(entries[start:start + 200]))
            ops += [delete(key) for key in rng.sample(keys, len(keys) // 10)]
            ops.append(get(rng.choice(keys)))
            if round_ == 1:
                ops.extend((CLOSE, OPEN))
        low, high = sorted(rng.sample(keys, 2))
        ops += [compact(low, high), get(low), compact(None, high), compact(low, None), iterate([seek(low), DRAIN]),
                compact(), READ_BACK, CLOSE, OPEN, CHECKED, CLOSE]
        yield f'compaction-{name}-{index}', make(caller(name), ops)


def written_log(rng, count, size):
    keys = key_set(rng, count)
    return [OPEN] + [put(key, value(rng, rng.choice(size))) for key in keys] + [CLOSE]


def truncation(rng):
    for index, keep in enumerate((0, 1, 6, 7, 8, 20, 100, 32767, 32768, 32769, 32775, 40000, 65536, 70000)):
        for extra, label in ((PARANOID, 'paranoid'), (0, 'lenient')):
            name = ('dom-storage', 'value-store', 'plain')[index % 3]
            ops = written_log(rng, 40, (10, 500, 3000, 40000)) + [truncate(LOG, 0, keep), OPEN, READ_ALL,
                                                                  put(b'after', b'crash'), CLOSE, OPEN, READ_ALL, CLOSE]
            yield f'truncate-{keep}-{label}', make(caller(name, extra, 0 if extra else PARANOID), ops)
    for index in range(12):
        keep_back = rng.randrange(1, 60)
        reuse = index % 2
        ops = written_log(rng, 20, (5, 60)) + [OPEN, put(b'second', b'x'), CLOSE]
        ops += [truncate(LOG, 0, 0), OPEN, READ_ALL, CLOSE] if index % 3 == 0 else [OPEN, READ_ALL, CLOSE]
        options = caller('dom-storage', 0, 0 if reuse else REUSE_LOGS)
        yield f'truncate-reuse-{index}-{keep_back}', make(options, ops)


def log_corruption(rng):
    offsets = (0, 3, 4, 5, 6, 7, 8, 30, 200, 32760, 32767, 32768, 32772, 32774, 32775, 33000, 60000)
    for offset in offsets:
        for extra, label in ((PARANOID, 'paranoid'), (0, 'lenient')):
            mask = bytes([rng.randrange(1, 256) for _ in range(rng.choice((1, 1, 3)))])
            ops = written_log(rng, 30, (10, 700, 5000, 30000)) + [corrupt(LOG, 0, offset, mask), OPEN, READ_ALL,
                                                                  get(b'missing'), CLOSE, OPEN, READ_ALL, CLOSE]
            options = (DEFAULT | CREATE | extra, 4 << 20, 1000)
            yield f'log-corrupt-{offset}-{label}', make(options, ops)
    for index in range(8):
        ops = written_log(rng, 4, (100,)) + [corrupt(LOG, 0, rng.randrange(12, 200), b'\x80'), OPEN, READ_ALL, CLOSE]
        yield f'log-corrupt-batch-{index}', make(caller('plain', PARANOID if index % 2 else 0, REUSE_LOGS), ops)


def flushed(rng, options, count=60, size=(50, 900)):
    keys = key_set(rng, count)
    data = {key: value(rng, rng.choice(size)) for key in keys}
    ops = [OPEN] + [put(key, data[key]) for key in keys] + [compact(), CLOSE]
    return keys, ops


def table_corruption(rng, ask):
    for index in range(36):
        extra = PARANOID if index % 2 else 0
        bloom = BLOOM if index % 3 == 0 else 0
        compression = SNAPPY if index % 4 < 2 else 0
        options = ((CREATE | REUSE_LOGS | extra | bloom | compression), 4 << 20, 1000)
        keys, ops = flushed(rng, options)
        region = index % 6
        if region == 0:
            target = corrupt(TABLE, 0, rng.randrange(0, 200), bytes([1 << rng.randrange(8)]))
        elif region == 1:
            target = truncate(TABLE, 0, rng.randrange(0, 400))
        elif region == 2:
            target = corrupt(TABLE, 0, (1 << 62) + 0, b'\x01')
        else:
            target = corrupt(TABLE, 0, rng.randrange(1000, 60000), bytes([rng.randrange(1, 256)]))
        probes = rng.sample(keys, 6)
        ops += [target, OPEN] + [get(key, read=read) for key in probes for read in (FILL, VERIFY)]
        ops += [iterate(read=FILL), iterate(read=VERIFY), iterate([LAST, PREV, PREV], read=VERIFY), CLOSE]
        yield f'table-corrupt-{index}', make(options, ops)
    for index in range(6):
        options = caller('indexed-db')
        keys, ops = flushed(rng, options)
        response = ask(make(options, ops + [EXPORT]))
        tables = [contents for name, contents in exported(response, len(ops)) if name.endswith(('.ldb', '.sst'))]
        if not tables:
            raise RuntimeError('no table to damage')
        offset = max(0, len(tables[0]) - 16)
        ops += [corrupt(TABLE, 0, offset, b'\xff'), OPEN, READ_ALL, CLOSE]
        yield f'table-footer-{index}', make(options, ops)


def metadata(rng):
    for index, (damage, label) in enumerate((
            (remove(CURRENT, 0), 'no-current'),
            (truncate(CURRENT, 0, 5), 'short-current'),
            (corrupt(CURRENT, 0, 0, b'\x01'), 'renamed-current'),
            (truncate(MANIFEST, 0, 0), 'empty-manifest'),
            (truncate(MANIFEST, 0, 20), 'short-manifest'),
            (corrupt(MANIFEST, 0, 10, b'\x40'), 'corrupt-manifest'),
            (remove(MANIFEST, 0), 'no-manifest'),
            (remove(TABLE, 0), 'no-table'),
            (remove(LOG, 0), 'no-log'))):
        for create in (CREATE, 0):
            for extra in (PARANOID, 0):
                options = ((DEFAULT | create | extra), 4 << 20, 1000)
                _, ops = flushed(rng, (DEFAULT | CREATE, 4 << 20, 1000), 20)
                ops = [ops[0]] + ops[1:-1] + [put(b'unflushed', b'log'), CLOSE]
                ops = [OPEN if op == OPEN else op for op in ops]
                yield f'metadata-{label}-{"create" if create else "existing"}-{"paranoid" if extra else "lenient"}', \
                    make(options, ops[:1] + ops[1:] + [damage, OPEN, READ_ALL, CLOSE, REPAIR, OPEN, READ_ALL, CLOSE])


def open_flags(rng):
    for create in (0, CREATE):
        for exclusive in (0, EXCLUSIVE):
            options = (SNAPPY | create | exclusive, 4 << 20, 1000)
            ops = [OPEN, put(b'a', b'1'), CLOSE, OPEN, get(b'a'), CLOSE, DESTROY, OPEN, get(b'a'), CLOSE]
            yield f'open-create-{str(bool(create)).lower()}-exclusive-{str(bool(exclusive)).lower()}', make(options, ops)
    yield 'open-destroy-missing', make(caller('plain'), [DESTROY, DESTROY, OPEN, READ_ALL, CLOSE, DESTROY, DESTROY])
    yield 'open-twice', make(caller('plain'), [OPEN, put(b'a', b'1'), OPEN, get(b'a'), CLOSE, CLOSE])
    yield 'open-closed-operations', make(caller('plain', 0, CREATE), [
        OPEN, put(b'a', b'1'), get(b'a'), snapshot(0), iterate(), compact(), REWRITE, CLOSE])


def repair(rng, lane):
    damages = [
        ('healthy', []),
        ('log-only', None),
        ('corrupt-log', [corrupt(LOG, 0, 40, b'\x10')]),
        ('corrupt-table', [corrupt(TABLE, 0, 30, b'\x08')]),
        ('truncated-table', [truncate(TABLE, 0, 100)]),
        ('no-manifest', [remove(MANIFEST, 0)]),
        ('no-current', [remove(CURRENT, 0)]),
        ('no-manifest-or-current', [remove(MANIFEST, 0), remove(CURRENT, 0)]),
        ('corrupt-manifest', [corrupt(MANIFEST, 0, 8, b'\x7f')]),
        ('corrupt-footer', [corrupt(TABLE, 0, 2 ** 64 - 9, b'\x55')]),
    ]
    for index, (label, damage) in enumerate(damages):
        for name in ('repair', 'repair-paranoid', 'file-system'):
            landlock_incompatible = lane == 'seen' and label == 'corrupt-log' and name == 'repair-paranoid'
            if landlock_incompatible:
                continue
            options = caller(name)
            keys = key_set(rng, 40)
            ops = [OPEN] + [put(key, value(rng, rng.randrange(400))) for key in keys[:20]]
            if damage is not None:
                ops.append(compact())
            ops += [put(key, value(rng, rng.randrange(400))) for key in keys[20:]] + [delete(keys[3]), CLOSE]
            if damage is None:
                ops += [remove(MANIFEST, 0), remove(CURRENT, 0)]
            else:
                ops += damage
            ops += [REPAIR, OPEN, READ_ALL, get(keys[3]), get(keys[25]), CLOSE, REPAIR, OPEN, READ_BACK, CLOSE]
            yield f'repair-{label}-{name}', make(options, ops)
    for index in range(6):
        options = caller('repair')
        ops = [OPEN] + [put(b'r%04d' % i, value(rng, 300)) for i in range(300)] + [compact()]
        ops += [put(b'r%04d' % i, b'new') for i in range(0, 300, 7)] + [compact(b'r0000', b'r0150'), CLOSE]
        ops += [corrupt(TABLE, index % 2, rng.randrange(0, 4000), b'\x21'), REPAIR, OPEN, READ_ALL, CLOSE]
        yield f'repair-tables-{index}', make(options, ops)


def destroy(rng):
    for index in range(4):
        options = caller(('dom-storage', 'proto', 'sync', 'value-store')[index])
        ops = written_log(rng, 10, (20,)) + [DESTROY, OPEN, READ_ALL, put(b'fresh', b'1'), CLOSE, OPEN, READ_ALL, CLOSE]
        yield f'destroy-{index}', make(options, ops)
    files = [(DATABASE, 'notes.txt', b'unrelated'), (DATABASE, 'LOG', b'old info log'), (DATABASE, '000099.bak', b'b')]
    yield 'destroy-foreign-files', make(caller('plain'), [DESTROY, OPEN, READ_ALL, CLOSE], files)


def rewrite(rng):
    for index in range(6):
        name = ('dom-storage', 'file-system', 'proto')[index % 3]
        keys = key_set(rng, 50)
        ops = [OPEN] + [put(key, value(rng, rng.randrange(300))) for key in keys]
        ops += [delete(key) for key in keys[::3]] + [snapshot(4)] + ([compact()] if index % 2 else [])
        ops += [REWRITE, READ_ALL, put(b'after', b'rewrite'), REWRITE, READ_BACK, CLOSE, OPEN, READ_ALL, CLOSE]
        yield f'rewrite-{index}', make(caller(name), ops)


def fixtures(rng, ask):
    """Directories the oracle wrote, read back under the same and other options."""
    writers = []
    for index in range(10):
        name = ('indexed-db', 'dom-storage', 'sync', 'plain', 'proto')[index % 5]
        keys = key_set(rng, rng.randrange(10, 300))
        ops = [OPEN]
        for key in keys:
            ops.append(put(key, value(rng, rng.choice((1, 50, 800, 6000)))))
        if index % 2:
            ops.append(compact())
        ops += [delete(key) for key in keys[::5]] + [put(keys[0], b'last')]
        if index % 3 == 0:
            ops.extend((CLOSE, OPEN, put(b'reopened', b'1')))
        writers.append((f'{name}-{index}', caller(name), ops, keys))
    for label, options, ops, keys in writers:
        response = ask(make(options, ops + [EXPORT]))
        files = [(DATABASE, file, contents) for file, contents in exported(response, len(ops))]
        readers = [('same', options), ('lenient', (DEFAULT, 4 << 20, 1000)), ('paranoid', (DEFAULT | PARANOID, 4 << 20, 1000)),
                   ('fresh-logs', (SNAPPY | PARANOID, 64 << 10, 0))]
        for reader, reader_options in readers:
            probes = [get(key, read=VERIFY | FILL) for key in keys[:20]]
            yield f'fixture-{label}-{reader}', make(reader_options, [OPEN, READ_ALL, CHECKED, READ_BACK] + probes + [
                put(b'appended', b'2'), CLOSE, OPEN, READ_ALL, CLOSE], files)
        for index, keep in enumerate((len(files[-1][1]) // 2, 7)):
            yield f'fixture-{label}-truncated-{index}-{keep}', make(options, [truncate(LOG, 0, keep), OPEN, READ_ALL, CLOSE],
                                                                   files)
        rebuild = [(REBUILD, file, contents) for _, file, contents in files]
        yield f'fixture-{label}-rebuild-only', make(options, [OPEN, READ_ALL, CLOSE], rebuild)
        damaged = [(DATABASE, 'CURRENT', b'MANIFEST-999999\n')]
        yield f'fixture-{label}-rebuild-damaged', make(options, [OPEN, READ_ALL, CLOSE], rebuild + damaged)
        yield f'fixture-{label}-rebuild-both', make(options, [OPEN, READ_ALL, CLOSE], files + rebuild[:2])


def scenarios(lane, ask):
    rng = random.Random({'seen': 5101, 'holdout': 5102}[lane])
    yield from sequences(rng, {'seen': 160, 'holdout': 90}[lane])
    yield from sizes(rng)
    yield from batches(rng)
    yield from iteration(rng)
    yield from snapshots(rng)
    yield from compaction(rng)
    yield from truncation(rng)
    yield from log_corruption(rng)
    yield from table_corruption(rng, ask)
    yield from metadata(rng)
    yield from open_flags(rng)
    yield from repair(rng, lane)
    yield from destroy(rng)
    yield from rewrite(rng)
    yield from fixtures(rng, ask)
