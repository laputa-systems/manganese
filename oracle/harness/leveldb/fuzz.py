#!/usr/bin/env -S python3 -B
import pathlib
import random
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import fuzzdriver
from protocol import OPEN, CLOSE, REPAIR, DESTROY, batch, compact, delete, get, iterate, put, release, request, seek, snapshot

seed, cases = fuzzdriver.budget()
rng = random.Random(seed)


def generate(_oracle):
    flags = rng.randrange(32)
    keys = [rng.randbytes(rng.choice((0, 1, 2, 4, 16, 48))) for _ in range(rng.randrange(2, 12))]
    ops = [OPEN]
    snapshots = set()
    for _ in range(rng.randrange(4, 35)):
        choice = rng.randrange(100)
        key = rng.choice(keys)
        if choice < 27:
            value = rng.randbytes(rng.choice((0, 1, 8, 64, 512, 2048)))
            ops.append(put(key, value, rng.randrange(8) == 0))
        elif choice < 38:
            ops.append(delete(key, rng.randrange(8) == 0))
        elif choice < 48:
            entries = []
            for _ in range(rng.randrange(6)):
                key = rng.choice(keys)
                value = rng.randbytes(rng.randrange(80)) if rng.randrange(2) else None
                entries.append((key, value))
            ops.append(batch(entries, rng.randrange(8) == 0))
        elif choice < 61:
            selected = rng.choice(tuple(sorted(snapshots)) + (255,))
            ops.append(get(key, selected))
        elif choice < 69:
            identifier = rng.randrange(3)
            snapshots.add(identifier)
            ops.append(snapshot(identifier))
        elif choice < 73:
            identifier = rng.randrange(3)
            snapshots.discard(identifier)
            ops.append(release(identifier))
        elif choice < 88:
            selected = rng.choice(tuple(sorted(snapshots)) + (255,))
            steps = [bytes([rng.choice((0, 1, 3, 4, 5, 6))]), seek(key)]
            ops.append(iterate(steps, selected))
        elif choice < 93:
            ops.append(compact())
        elif choice < 97:
            ops.extend((CLOSE, OPEN))
            snapshots.clear()
        elif choice < 99:
            ops.extend((CLOSE, REPAIR, OPEN))
            snapshots.clear()
        else:
            ops.extend((CLOSE, DESTROY, OPEN))
            snapshots.clear()
    ops.extend((iterate(), CLOSE))
    return request(flags, ops)


fuzzdriver.run('leveldb', seed, cases, generate)
