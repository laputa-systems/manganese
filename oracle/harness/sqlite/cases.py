"""The sqlite corpus: step scripts over Chromium's sql::Database, recorded against the source oracle.

Profile fixtures are the SQLite databases of a frozen-browser profile (`make sqlite-anchor` captures
one); crash images and WAL fixtures are written by the oracle itself through EXPORT, so every
database byte in the corpus comes from Chromium.
"""

import os
import pathlib
import random
import re
import tarfile

import profiles
import protocol as p

PROFILE_ARCHIVE = '7456dfed53ab69f8458c2b499da9794d4dc3b69221d7181c2b2c7c2b529bb996'
SEEDS = {'seen': 4101, 'holdout': 4102}

VALUES = [None, 0, 1, -1, 7, 2147483647, 2147483648, -2147483649, 9223372036854775807, -9223372036854775808,
          0.5, -0.0, 1.5, 3.0, 1e20, 1e-7, 123456.789, 1.0 / 3, 2.5e300, '', 'abc', 'ABC', 'aBc ', ' 12 ', '12',
          '-7', '1e3', '0x1A', '3.0', 'λ', 'Straße', 'İstanbul', 'ΣΑΣ', 'ǆ', 'ﬁ', '%', '_', 'a_c', 'a%c',
          b'', b'\x00', b'abc', b'\xff\xfe', b'12']

EXPRESSIONS = [
    '{a} + {b}', '{a} - {b}', '{a} * {b}', '{a} / {b}', '{a} % {b}', '{a} || {b}', '{a} = {b}', '{a} < {b}',
    '{a} <> {b}', '{a} IS {b}', '{a} IS NOT {b}', '{a} LIKE {b}', '{a} GLOB {b}', '{a} AND {b}', '{a} OR {b}',
    'typeof({a})', 'length({a})', 'upper({a})', 'lower({a})', 'abs({a})', 'round({a})', 'round({a}, 2)',
    'hex({a})', 'quote({a})', 'substr({a}, 2)', 'substr({a}, -2, 1)', 'instr({a}, {b})', 'replace({a}, {b}, \'-\')',
    'trim({a})', 'ltrim({a}, \'a \')', 'coalesce({a}, {b})', 'ifnull({a}, {b})', 'nullif({a}, {b})',
    'max({a}, {b})', 'min({a}, {b})', 'CAST({a} AS INTEGER)', 'CAST({a} AS REAL)', 'CAST({a} AS TEXT)',
    'CAST({a} AS BLOB)', 'CAST({a} AS NUMERIC)', 'unicode({a})', 'char(65, 955)', 'printf(\'%d|%s|%.3f\', {a}, {b}, {a})',
    'zeroblob(3)', '{a} BETWEEN {b} AND 10', '{a} IN (1, \'abc\', {b})', 'CASE WHEN {a} THEN {b} ELSE 0 END',
    '-{a}', 'NOT {a}', '~{a}', '{a} << 3', '{a} >> 1', '{a} & 6', '{a} | 9', 'likely({a})', 'sign({a})',
    'iif({a}, 1, 2)', 'format(\'%5.1f\', {a})', 'concat({a}, {b})', 'concat_ws(\',\', {a}, {b})',
    'octet_length({a})', 'unhex(\'414243\')', '{a} COLLATE NOCASE = {b}', '{a} COLLATE RTRIM = {b}',
]

TIME_EXPRESSIONS = [
    "date('2024-02-29')", "time('12:34:56.789')", "datetime('2024-01-01 10:00:00', '+1 day', '-3 hours')",
    "julianday('2000-01-01')", "strftime('%Y-%m-%d %H:%M:%f %j %w %s', '2023-07-04 05:06:07.891')",
    "unixepoch('2023-07-04 05:06:07')", "datetime(1700000000, 'unixepoch')", "date('2024-01-31', '+1 month')",
    "strftime('%W %u %G %V', '2021-01-03')", "timediff('2024-03-01', '2024-02-01')", "date('invalid')",
    "datetime('2024-06-15', 'start of month', 'weekday 0')", "time(0.5)", "julianday('now') > 2460000",
]

OMITTED = [
    'INSERT INTO t(a, b) VALUES(1, 2) ON CONFLICT(a) DO UPDATE SET b = 3',
    'SELECT a, row_number() OVER (ORDER BY a) FROM t',
    "SELECT json('{\"a\": 1}')",
    "SELECT json_extract('{\"a\": 1}', '$.a')",
    'EXPLAIN SELECT 1',
    'EXPLAIN QUERY PLAN SELECT 1',
    'ANALYZE',
    'REINDEX',
    'SELECT "a" FROM t',
    'SELECT "missing" FROM t',
    'CREATE INDEX ti ON t("b")',
    'PRAGMA compile_options',
    'PRAGMA function_list',
    "SELECT load_extension('x')",
    'SELECT * FROM generate_series(1, 3)',
    'SELECT * FROM pragma_table_info(\'t\')',
    'SELECT sqlite_version()',
    'SELECT sqlite_compileoption_used(\'ENABLE_ICU\')',
    "SELECT 'abc' REGEXP 'b'",
    'SELECT randomblob(0) IS NOT NULL',
    'ATTACH DATABASE \':memory:\' AS other',
    'SELECT total_changes() >= 0',
    "SELECT percentile(a, 50) FROM t",
    'CREATE VIRTUAL TABLE f USING fts3(x)',
    'CREATE VIRTUAL TABLE f USING fts5(x)',
    'WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 5) SELECT group_concat(x) FROM c',
    'SELECT a FROM t NATURAL JOIN t AS u',
    'SELECT a, b FROM t RETURNING a',
    'UPDATE t SET b = b + 1 RETURNING a, b',
    'DELETE FROM t WHERE a = 1 RETURNING *',
    'INSERT INTO t VALUES(9, 9) RETURNING rowid, *',
    'SELECT * FROM t LIMIT -1 OFFSET 1',
    'SELECT count(*) FILTER (WHERE a > 1) FROM t',
    'SELECT group_concat(b, \'|\' ORDER BY b DESC) FROM t',
    "SELECT a FROM t WHERE a IS DISTINCT FROM 1",
    'SELECT 1 FROM t INDEXED BY ti',
    'PRAGMA integrity_check',
    'PRAGMA quick_check',
    'PRAGMA foreign_keys',
    'PRAGMA secure_delete',
    'PRAGMA journal_mode',
    'PRAGMA locking_mode',
    'PRAGMA auto_vacuum',
    'PRAGMA cache_size',
    'PRAGMA encoding',
    'PRAGMA table_info(t)',
    'PRAGMA index_list(t)',
    'PRAGMA index_info(sqlite_autoindex_t_1)',
    'PRAGMA user_version = 7',
    'PRAGMA schema_version',
    'PRAGMA freelist_count',
    'PRAGMA page_count',
    'PRAGMA case_sensitive_like = 1',
    'PRAGMA recursive_triggers',
    'PRAGMA temp_store',
    'PRAGMA synchronous',
    'PRAGMA mmap_size',
]


def slug(name):
    return re.sub(r'[^a-z0-9]+', '-', name.lower()).strip('-')


def literal(value):
    if value is None:
        return 'NULL'
    if isinstance(value, bytes):
        return "x'" + value.hex() + "'"
    if isinstance(value, str):
        return "'" + value.replace("'", "''") + "'"
    if isinstance(value, float):
        return repr(value) if value == value and abs(value) != float('inf') else '0'
    return str(value)


def profile_files():
    store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store'))
    archive = store / 'sha256' / PROFILE_ARCHIVE
    databases = {}
    with tarfile.open(archive) as tar:
        for member in tar.getmembers():
            if not member.isfile():
                continue
            data = tar.extractfile(member).read()
            for kind, suffix in ((1, '-journal'), (2, '-wal'), (0, '')):
                if member.name.endswith(suffix):
                    databases.setdefault(member.name.removesuffix(suffix), {})[kind] = data
                    break
    return {name: files for name, files in sorted(databases.items()) if files.get(0, b'')[:16] == profiles.MAGIC}


def schema_of(ask, files, name):
    flags, page_size, cache_size = profiles.options(pathlib.Path(name).name, files[0])
    script = profiles.load(p.Script(), files).open(flags, page_size, cache_size).dump()
    request = script.encode()
    dumped = p.decode(request, ask(request))[-1][1]
    rows = dumped['schema'][1]
    statements = [row[3] for row in rows if row[3] and not row[1].startswith('sqlite_')]
    tables = {}
    for row in rows:
        if row[0] == 'table' and not row[1].startswith('sqlite_') and not (row[3] or '').startswith('CREATE VIRTUAL'):
            info = p.Script().open().execute(row[3]).prepare(1, f'PRAGMA table_info("{row[1]}")').step(1)
            columns = p.decode(info.encode(), ask(info.encode()))[-1][1][0]
            tables[row[1]] = [(column[1], (column[2] or '').upper(), column[3], column[5]) for column in columns]
    return (flags, page_size, cache_size), statements, tables


def value_for(rng, declared, unique):
    if 'INT' in declared:
        return rng.choice([unique, unique * 1000003, -unique, 0, 2 ** 40 + unique, 13 * unique])
    if 'REAL' in declared or 'FLOA' in declared or 'DOUB' in declared:
        return rng.choice([unique + 0.25, -unique / 3, 1e10 * unique, 0.0])
    if 'BLOB' in declared:
        return bytes(rng.randrange(256) for _ in range(rng.choice([0, 1, 4, 16, 64])))
    if 'BOOL' in declared:
        return rng.choice([0, 1])
    text = rng.choice(['https://example.test/', 'Title', 'λ-ü-ß ', 'a"b\'c', 'MiXeD', '', '0001', 'x' * 300])
    return f'{text}{unique}' if rng.randrange(4) else text


def profile_cases(lane, rng, ask):
    for name, files in profile_files().items():
        label = slug(name)
        flags, page_size, cache_size = profiles.options(pathlib.Path(name).name, files[0])
        dumped = p.decode(*(lambda request: (request, ask(request)))(
            profiles.load(p.Script(), files).open(flags, page_size, cache_size).dump().encode()))[-1][1]
        tables = [table for table in dumped['tables']]
        has_meta = 'meta' in tables
        if lane == 'seen':
            yield f'profile-unchanged-{label}', profiles.unchanged_script(pathlib.Path(name).name, files)
            script = profiles.load(p.Script(), files).open(flags, page_size, cache_size)
            for slot, table in enumerate(tables[:8]):
                script.prepare(slot, f'SELECT * FROM "{table}" ORDER BY rowid LIMIT 4').step(slot, getters=['string'] * 40)
                script.exists('table', table).exists('column', table, 'rowid')
            if has_meta:
                script.meta_init(1, 1).meta_get_version().meta_get_compatible().meta_get_value('mmap_status', 'int')
            yield f'profile-read-{label}', script.changes().close().digest()
        else:
            script = profiles.load(p.Script(), files).open(flags, page_size, cache_size)
            if has_meta:
                script.meta_init(1, 1).meta_set_value('manganese', 'probe').meta_get_value('manganese', 'text')
            script.begin(0)
            for table in tables:
                script.execute(f'DELETE FROM "{table}" WHERE rowid IN (SELECT rowid FROM "{table}" ORDER BY rowid LIMIT 1)')
            script.commit(0).changes().close().open(flags, page_size, cache_size).dump()
            yield f'profile-write-{label}', script.close()
            script = profiles.load(p.Script(), files).open(flags, page_size, cache_size).begin(0)
            for table in tables:
                script.execute(f'DELETE FROM "{table}"')
            yield f'profile-crash-{label}', script.crash().open(flags, page_size, cache_size).dump().close()


def schema_cases(lane, rng, ask):
    for name, files in profile_files().items():
        label = slug(name)
        (flags, page_size, cache_size), statements, tables = schema_of(ask, files, name)
        flags &= ~p.WAL if rng.randrange(2) else ~0
        for variant in range(2 if lane == 'seen' else 1):
            script = p.Script().open(flags, page_size, cache_size)
            for sql in statements:
                script.execute(sql)
            script.begin(0)
            unique = rng.randrange(1, 1000)
            for slot, (table, columns) in enumerate(sorted(tables.items())):
                slot %= 200
                placeholders = ', '.join('?' for _ in columns)
                names = ', '.join(f'"{column[0]}"' for column in columns)
                script.prepare(slot, f'INSERT OR IGNORE INTO "{table}" ({names}) VALUES ({placeholders})')
                for _ in range(rng.choice([1, 3, 12])):
                    unique += 1
                    for index, (_, declared, notnull, pk) in enumerate(columns):
                        value = value_for(rng, declared, unique)
                        if value is None and notnull:
                            value = 0
                        script.bind(slot, index, value)
                    script.step(slot).reset(slot)
                script.finalize(slot)
            script.commit(0).changes()
            for slot, (table, columns) in enumerate(sorted(tables.items())):
                column = rng.choice(columns)[0]
                script.prepare(slot % 200, f'SELECT * FROM "{table}" ORDER BY "{column}" DESC, rowid LIMIT 20').step(slot % 200)
                script.execute(f'UPDATE "{table}" SET "{column}" = "{column}" WHERE rowid % 2 = 0')
                script.execute(f'DELETE FROM "{table}" WHERE rowid % 3 = 0')
                script.finalize(slot % 200)
            yield f'schema-{label}-{variant}', script.changes().dump().close().open(flags, page_size, cache_size).dump()


def expression_cases(lane, rng):
    yield from expression_cases_with(rng, 120 if lane == 'seen' else 80)
    for index, expression in enumerate(TIME_EXPRESSIONS):
        if (index % 3 == 0) == (lane == 'seen'):
            continue
        yield f'time-{index}', p.Script().open().prepare(1, 'SELECT ' + expression).step(1)


def expression_cases_with(rng, count):
    for index in range(count):
        script = p.Script().open()
        expressions = []
        for _ in range(12):
            template = rng.choice(EXPRESSIONS)
            expressions.append(template.format(a=literal(rng.choice(VALUES)), b=literal(rng.choice(VALUES))))
        script.prepare(1, 'SELECT ' + ', '.join(expressions)).step(1)
        getters = [rng.choice(list(p.GETTERS)) for _ in expressions]
        script.reset(1).step(1, getters=getters)
        yield f'expression-{index}', script


def affinity_cases(lane, rng):
    yield from affinity_cases_with(rng, 30 if lane == 'seen' else 20)


def affinity_cases_with(rng, count):
    for index in range(count):
        script = p.Script().open().execute(
            'CREATE TABLE a(i INTEGER, r REAL, t TEXT, b BLOB, n NUMERIC, x, c TEXT COLLATE NOCASE, '
            'v VARCHAR(10), d DATETIME, bo BOOLEAN)')
        script.prepare(1, 'INSERT INTO a VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?)')
        for _ in range(8):
            value = rng.choice(VALUES)
            for column in range(10):
                script.bind(1, column, value if column % 3 else rng.choice(VALUES))
            script.step(1).reset(1)
        script.prepare(2, 'SELECT typeof(i), typeof(r), typeof(t), typeof(b), typeof(n), typeof(x), i, r, t, n FROM a ORDER BY rowid').step(2)
        order = rng.choice(['i', 'r', 't', 'b', 'n', 'x', 'c', 'c COLLATE BINARY', 't COLLATE NOCASE', 't COLLATE RTRIM'])
        script.prepare(3, f'SELECT rowid, {order.split()[0]} FROM a ORDER BY {order}, rowid').step(3)
        script.prepare(4, f'SELECT count(*), count(DISTINCT {order.split()[0]}), min(t), max(n), sum(i), total(r), avg(i), group_concat(t, \'|\') FROM a').step(4)
        probe = literal(rng.choice(VALUES))
        script.prepare(5, f'SELECT rowid FROM a WHERE t = {probe} OR i = {probe} OR c = {probe} OR t LIKE {probe} ORDER BY rowid').step(5)
        yield f'affinity-{index}', script.dump()


def statement_cases(lane, rng):
    yield from statement_cases_with(rng, 40 if lane == 'seen' else 25)
    for index, sql in enumerate(OMITTED):
        if (index % 3 == 1) == (lane == 'seen'):
            continue
        script = p.Script().open().execute('CREATE TABLE t(a PRIMARY KEY, b); CREATE INDEX ti ON t(b); INSERT INTO t VALUES(1, 2)')
        yield f'feature-{index}', script.prepare(1, sql).step(1).execute(sql).sql_valid(sql).dump()


def statement_cases_with(rng, count):
    base = ('CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT NOT NULL DEFAULT \'d\', c UNIQUE, d REAL CHECK (d IS NULL OR d >= 0));'
            'CREATE INDEX ti ON t(b); CREATE TABLE s(k TEXT PRIMARY KEY, v BLOB) WITHOUT ROWID;'
            'CREATE TABLE au(id INTEGER PRIMARY KEY AUTOINCREMENT, x)')
    statements = [
        "INSERT INTO t(b, c, d) VALUES('x', 1, 1.5)", "INSERT INTO t(b, c) VALUES('y', 1)", "INSERT INTO t(b) VALUES(NULL)",
        "INSERT INTO t(a, b) VALUES(1, 'dup')", "INSERT OR REPLACE INTO t(a, b, c) VALUES(1, 'rep', 2)",
        "INSERT OR IGNORE INTO t(b, c) VALUES('z', 2)", "INSERT INTO t(b, d) VALUES('neg', -1)", "INSERT INTO t DEFAULT VALUES",
        "UPDATE t SET c = c + 10 WHERE a > 0", "UPDATE t SET b = upper(b)", "DELETE FROM t WHERE c IS NULL",
        "INSERT INTO s VALUES('k1', x'01'), ('k2', x'02')", "INSERT INTO s VALUES('k1', x'03')", "REPLACE INTO s VALUES('k1', x'04')",
        "INSERT INTO au(x) VALUES(1), (2)", "DELETE FROM au WHERE id = 2", "INSERT INTO au(x) VALUES(3)",
        "ALTER TABLE t ADD COLUMN e TEXT DEFAULT 'new'", "ALTER TABLE t ADD COLUMN f INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE t RENAME TO t2", "ALTER TABLE s RENAME COLUMN v TO w", "CREATE TABLE IF NOT EXISTS t(z)",
        "CREATE TABLE t(z)", "DROP TABLE IF EXISTS missing", "DROP TABLE missing", "CREATE UNIQUE INDEX tu ON t(c, b)",
        "DROP INDEX ti", "CREATE INDEX IF NOT EXISTS tp ON t(d) WHERE d IS NOT NULL", "CREATE VIEW v AS SELECT a FROM t",
        "SELECT * FROM v", "CREATE TRIGGER tr AFTER INSERT ON t BEGIN INSERT INTO au(x) VALUES(new.a); END",
        "INSERT INTO t(b, c) VALUES('trig', 99)", "SELECT count(*) FROM au", "DELETE FROM t", "BEGIN", "COMMIT",
        "ROLLBACK", "SAVEPOINT sp", "RELEASE sp", "ROLLBACK TO sp", "SELECT * FROM sqlite_sequence",
        "CREATE TABLE fk(p REFERENCES t(a) ON DELETE CASCADE)", "INSERT INTO fk VALUES(12345)",
        "SELECT a, b FROM t WHERE b LIKE 'X%' ORDER BY a DESC LIMIT 2 OFFSET 1", "SELECT b, count(*) FROM t GROUP BY b HAVING count(*) > 0",
        "SELECT t.a, s.k FROM t LEFT JOIN s ON s.k = 'k' || t.a ORDER BY 1", "SELECT a FROM t UNION SELECT c FROM t ORDER BY 1",
        "SELECT (SELECT max(a) FROM t), EXISTS (SELECT 1 FROM s)", "SELECT a FROM t WHERE a IN (SELECT c FROM t)",
        "CREATE TABLE big(x); INSERT INTO big VALUES(zeroblob(5000)), (zeroblob(0))", "SELECT length(x) FROM big",
        "PRAGMA user_version = 42", "PRAGMA application_id = 7", "VACUUM",
    ]
    for index in range(count):
        flags = rng.choice([0, 0, p.WAL, p.TRIGGERS, p.VIEWS, p.TRIGGERS | p.VIEWS, p.SHARED_LOCKING])
        script = p.Script().open(flags).execute(base)
        for _ in range(rng.randrange(6, 18)):
            script.execute(rng.choice(statements)).changes()
        script.prepare(1, 'SELECT * FROM t ORDER BY 1').step(1)
        yield f'statement-{index}', script.dump()


def api_cases(lane, rng):
    yield from api_cases_with(rng, 50 if lane == 'seen' else 35)


def api_cases_with(rng, count):
    for index in range(count):
        script = p.Script().open(rng.choice([0, p.WAL, p.NO_SYNC]))
        script.execute('CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT UNIQUE, c)')
        active, slots, meta, open_ = {}, set(), False, True
        for _ in range(rng.randrange(10, 40)):
            choice = rng.randrange(100)
            if not open_:
                script.open(rng.choice([0, p.WAL]))
                open_, active, slots, meta = True, {}, set(), False
            elif choice < 15:
                slot = rng.randrange(4)
                sql = rng.choice(['INSERT INTO t(b, c) VALUES(?, ?)', 'SELECT a, b, c FROM t WHERE b > ? OR c = ? ORDER BY a',
                                  'UPDATE t SET c = ? WHERE b = ?', 'DELETE FROM t WHERE b = ? OR c = ?', 'SELECT ?1 || ?2, ?3',
                                  'SELECT * FROM missing WHERE x = ?', 'SELECT count(*) FROM t'])
                script.prepare(slot, sql)
                slots.add(slot)
            elif choice < 30 and slots:
                slot = rng.choice(sorted(slots))
                for parameter in range(rng.randrange(4)):
                    script.bind(slot, parameter, rng.choice(VALUES))
                script.step(slot, rng.choice([1, 2, 100]), [rng.choice(list(p.GETTERS)) for _ in range(3)])
            elif choice < 36 and slots:
                script.reset(rng.choice(sorted(slots)), rng.randrange(2) == 1)
            elif choice < 40 and slots:
                slot = rng.choice(sorted(slots))
                script.finalize(slot)
                slots.discard(slot)
            elif choice < 50:
                slot = rng.randrange(3)
                script.begin(slot)
                active[slot] = True
            elif choice < 56 and active:
                slot = rng.choice(sorted(active))
                script.commit(slot)
            elif choice < 60 and active:
                slot = rng.choice(sorted(active))
                script.rollback(slot)
            elif choice < 63 and active:
                slot = rng.choice(sorted(active))
                script.end_transaction(slot)
                del active[slot]
            elif choice < 70:
                script.execute(f"INSERT INTO t(b, c) VALUES({literal(rng.choice(VALUES))}, {literal(rng.choice(VALUES))})")
            elif choice < 74:
                script.changes()
            elif choice < 80:
                if not meta:
                    script.meta_init(rng.randrange(1, 5), rng.randrange(1, 5))
                    meta = True
                op = rng.randrange(6)
                if op == 0:
                    script.meta_set_version(rng.randrange(1, 9))
                elif op == 1:
                    script.meta_get_version().meta_get_compatible()
                elif op == 2:
                    script.meta_set_value(rng.choice(['a', 'b']), rng.choice([5, 'five', -1]))
                elif op == 3:
                    script.meta_get_value(rng.choice(['a', 'b', 'version']), rng.choice(['int', 'text']))
                elif op == 4:
                    script.meta_delete_key(rng.choice(['a', 'b']))
                else:
                    script.meta_raze_if_incompatible(rng.randrange(0, 4), rng.randrange(1, 6))
            elif choice < 84:
                script.exists(rng.choice(['table', 'index', 'view', 'meta']), rng.choice(['t', 'meta', 'sqlite_autoindex_t_1', 'x']))
                script.exists('column', 't', rng.choice(['a', 'B', 'rowid', 'oid', 'zz']))
            elif choice < 87:
                script.sql_valid(rng.choice(['SELECT 1', 'SELECT', 'SELECT * FROM t', 'SELECT * FROM nope', '', ';', '-- only']))
            elif choice < 90 and not active:
                script.raze().dump()
                slots.clear()
            elif choice < 92:
                script.vacuum()
            elif choice < 95:
                script.dump()
            elif choice < 97:
                script.close()
                open_, active, slots, meta = False, {}, set(), False
            else:
                script.raze_and_poison().changes().exists('table', 't').close()
                open_, active, slots, meta = False, {}, set(), False
        yield f'api-{index}', script.dump() if open_ else script


def durability_cases(lane, rng, ask):
    yield from crash_cases_with(rng, 16 if lane == 'seen' else 12)
    yield from durability_fixture_cases(lane, rng, ask)


def crash_cases_with(rng, count):
    for index in range(count):
        wal = rng.randrange(2) == 1
        flags = p.WAL if wal else 0
        cache = rng.choice([0, 0, 2, 10])
        script = p.Script().open(flags, 4096, cache).execute('CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT)')
        script.prepare(1, 'INSERT INTO t(b) VALUES(?)')
        for row in range(rng.choice([3, 50])):
            script.bind(1, 0, f'committed-{row}-' + 'c' * rng.choice([10, 900])).step(1).reset(1)
        script.finalize(1).begin(0)
        script.prepare(1, 'INSERT INTO t(b) VALUES(?)')
        for row in range(rng.choice([2, 40, 200])):
            script.bind(1, 0, f'pending-{row}-' + 'p' * rng.choice([10, 900])).step(1).reset(1)
        script.finalize(1)
        if rng.randrange(3) == 0:
            script.execute('DELETE FROM t WHERE a % 2 = 0')
        if rng.randrange(4) == 0:
            script.commit(0)
        drop = rng.randrange(4) == 0
        script.crash(drop).open(flags, 4096, cache).dump().prepare(1, 'PRAGMA integrity_check').step(1).finalize(1)
        yield f'crash-{index}', script.close()


def durability_fixture_cases(lane, rng, ask):
    for index in range(6 if lane == 'seen' else 4):
        wal = index % 2 == 1
        flags = p.WAL if wal else 0
        maker = p.Script().open(flags, 4096, 2).execute('CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT); CREATE INDEX tb ON t(b)')
        maker.execute('INSERT INTO t(b) VALUES ' + ', '.join(f"('base-{row}-{'b' * 200}')" for row in range(30)))
        maker.begin(0).execute('UPDATE t SET b = b || \'-updated\'')
        maker.execute('INSERT INTO t(b) VALUES ' + ', '.join(f"('hot-{row}-{'h' * 300}')" for row in range(40 + index)))
        if wal:
            maker.commit(0).begin(1).execute('DELETE FROM t WHERE a < 10')
        request = maker.crash().export().encode()
        exported = p.decode(request, ask(request))[-1][1]
        files = {kind: data for kind, (present, data) in enumerate(exported) if present}
        script = profiles.load(p.Script(), files).artifacts().open(flags).dump()
        script.prepare(1, 'PRAGMA integrity_check').step(1).finalize(1)
        yield f'hot-image-{index}', script.close().artifacts()
    for index in range(5 if lane == 'seen' else 3):
        secret = f'secret-{lane}-{index}-' + 'S' * 40
        flags = rng.choice([0, p.WAL])
        script = p.Script().open(flags).execute('CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT)')
        script.execute(f"INSERT INTO t(b) VALUES('{secret}'), ('keep')").execute(f"DELETE FROM t WHERE b = '{secret}'")
        if index % 2:
            script.execute('DROP TABLE t')
        yield f'residue-{index}', script.close().residue(secret).open(flags).dump().close().residue(secret)
    for index in range(4 if lane == 'seen' else 2):
        first, second = (0, p.WAL) if index % 2 == 0 else (p.WAL, 0)
        script = p.Script().open(first).execute('CREATE TABLE t(a, b); INSERT INTO t VALUES(1, \'one\')').close().artifacts()
        script.open(second).execute('INSERT INTO t VALUES(2, \'two\')').dump().close().artifacts()
        yield f'mode-switch-{index}', script.open(first).dump().close().artifacts()


def scenarios(lane, ask):
    rng = random.Random(SEEDS[lane])
    families = [profile_cases(lane, rng, ask), schema_cases(lane, rng, ask), expression_cases(lane, rng),
                affinity_cases(lane, rng), statement_cases(lane, rng), api_cases(lane, rng),
                durability_cases(lane, rng, ask)]
    for family in families:
        for name, script in family:
            yield name, script.encode()


def fuzz_script(rng):
    choice = rng.randrange(10)
    family = expression_cases_with if choice < 3 else statement_cases_with if choice < 5 else \
        api_cases_with if choice < 8 else affinity_cases_with if choice < 9 else crash_cases_with
    return next(family(rng, 1))[1]
