"""Hydrogen profile databases and the sql::DatabaseOptions their callers open them with."""

import protocol as p

MAGIC = b'SQLite format 3\x00'

CALLERS = {
    'History': (0, 4096, 1000),
    'Favicons': (0, 2048, 32),
    'Web Data': (0, 2048, 32),
    'Account Web Data': (0, 2048, 32),
    'Cookies': (p.SHARED_LOCKING, 4096, 0),
    'Trust Tokens': (p.VIEWS | p.MMAP_ALT_STATUS, 4096, 0),
    'Network Action Predictor': (p.VIEWS | p.MMAP_ALT_STATUS, 4096, 0),
    'DIPS': (p.WAL, 4096, 32),
    'heavy_ad_intervention_opt_out.db': (0, 4096, 250),
}


def find(profile):
    found = []
    for path in sorted(profile.rglob('*')):
        if path.is_file() and not path.name.endswith(('-journal', '-wal', '-shm')):
            with open(path, 'rb') as handle:
                if handle.read(16) == MAGIC:
                    found.append(path)
    return found


def options(name, header):
    flags, page_size, cache_size = CALLERS.get(name, (0, 4096, 0))
    if header[18] == 2:
        flags |= p.WAL
    else:
        flags &= ~p.WAL
    return flags, page_size, cache_size


def load(script, files):
    for kind in sorted(files):
        script.file(kind, files[kind])
    return script


def unchanged_script(name, files):
    flags, page_size, cache_size = options(name, files[0])
    script = load(p.Script(), files).open(flags, page_size, cache_size).dump().close()
    return script.digest().artifacts()
