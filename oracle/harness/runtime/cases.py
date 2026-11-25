import random


def script(rng: random.Random, family: int | None = None, runner_override: int | None = None, id_offset: int = 0) -> bytes:
    if family is None:
        family = rng.randrange(26)
    ids = [value + id_offset for value in rng.sample(range(100, 1000000), 12)]
    a, b, c, d, e, f, g, h, i, j, k, l = ids
    runner = rng.randrange(1, 100)
    if runner_override is not None:
        runner = runner_override
    kind = rng.choice((1, 2))
    priority = rng.randrange(3)
    if family == 23:
        priority = 2
    shutdown = rng.randrange(3)
    if 18 <= family <= 20:
        shutdown = family - 18
    delay = rng.randrange(1, 12)
    other = rng.randrange(1, 12)
    if family == 14 and other == delay:
        other += 1
    create = f'R {runner} {kind} {priority} {shutdown}'
    nop = '0 0 0 0'
    if family == 0:
        lines = [create, f'P {runner} {a} 0 1 {runner} {c} 0', f'P {runner} {b} 0 {nop}', 'I']
    elif family == 1:
        lines = [f'L {a} {delay} {nop}', f'L {b} {delay} {nop}', f'C {a}', f'A {delay}', f'C {a}', f'C {b}']
    elif family == 2:
        lines = [f'P 0 {a} 0 2 0 {b} 0', f'L {b} {delay} {nop}', 'I', f'A {delay}', f'C {b}']
    elif family == 3:
        lines = [create, f'K {runner} {a}', f'X {a}', 'I', f'X {a}', f'K {runner} {b}', 'I', f'X {b}']
    elif family == 4:
        lines = [f'W {a} {delay}', f'V {a}', f'W {b} {other}', f'A {max(delay, other)}']
    elif family == 5:
        lines = [f'T {a} 0 {delay} {b}', f'A {delay - 1}', f'T {a} 0 {other} {c}', f'A {other}', f'A {delay}']
    elif family == 6:
        lines = [f'T {a} 1 {delay} {b}', f'A {delay * 2 + delay // 2}', f'T {a} 1 {other} {c}', f'A {other * 2}', f'S {a}', f'A {other * 2}']
    elif family == 7:
        lines = [create, f'Q {runner} {a} {b}', 'I', f'P 0 {c} 0 {nop}', 'I']
    elif family == 8:
        lines = [create, f'B {a} {runner} {b} {c}', f'F {a} {d}', f'F {a} {e}', f'D {a}', 'I']
    elif family == 9:
        lines = [create, f'P 0 {a} {delay} {nop}', f'P {runner} {b} 0 {nop}', f'P {runner} {c} {delay} {nop}', 'H']
    elif family == 10:
        lines = [f'P 0 {a} {delay} {nop}', f'P 0 {b} {delay} {nop}', f'P 0 {c} {delay} 1 0 {d} 0', f'A {delay}']
    elif family == 11:
        lines = [create, f'P {runner} {a} {delay} 1 0 {b} 0', f'A {delay}', f'P 0 {c} 0 1 {runner} {d} {other}', 'I', f'A {other}']
    elif family == 12:
        lines = [create, f'B {a} {runner} {b} {c}', f'J {a} {d} {e}', 'I', f'D {a}', 'I']
    elif family == 13:
        lines = [f'P 0 {a} {delay} {nop}', f'P 0 {b} {other} {nop}', f'P 0 {c} {delay} 1 0 {d} 0', f'P 0 {e} 0 {nop}', f'A {max(delay, other)}']
    elif family == 14:
        lines = [create, f'P {runner} {a} {delay} {nop}', f'P {runner} {b} {delay} 1 {runner} {c} 0', f'P {runner} {d} {other} {nop}', f'A {max(delay, other)}']
    elif family == 15:
        lines = [f'T {a} 1 {delay} {b}', f'T {c} 0 {delay} {d}', f'A {delay * 2}', f'S {a}', f'A {delay}']
    elif family == 16:
        lines = [f'L {a} 0 {nop}', f'C {a}', f'L {b} {delay} 1 0 {c} 0', f'A {delay}', f'C {b}', 'I']
    elif family == 17:
        lines = [create, f'Y {runner} {a} {b}', f'X {a}', 'I', f'X {a}', f'Y {runner} {c} {d}', 'I', f'X {c}']
    elif family <= 20:
        lines = [create, f'Z {runner} {a}', f'Z {runner} {b}', f'P {runner} {c} 0 {nop}', 'I']
    elif family == 21:
        lines = [f'Y 0 {a} {b}', f'P 0 {c} 0 3 0 {a} 0', 'I', f'X {a}']
    elif family == 22:
        lines = [create, f'P {runner} {a} 0 1 {runner} {b} 0', f'P {runner} {c} 0 {nop}', 'H']
    elif family == 23:
        second = runner + 100
        other_kind = rng.choice((1, 2))
        lines = [create, f'R {second} {other_kind} {priority} {shutdown}', f'G {runner} {second}']
    elif family == 24:
        lines = [create] + [f'P {runner} {item} 0 {nop}' for item in ids[:8]] + ['I']
    else:
        lines = [create, f'U {a}', f'O {a}', f'N {runner} {a}', 'I', f'X {a}', f'O {a}', f'N {runner} {a}', 'I', f'X {a}']
    return ('\n'.join(lines) + '\n').encode()


def corpus(seed: int, count: int) -> list[bytes]:
    rng = random.Random(seed)
    return [script(rng, index % 12) for index in range(count)]


def replies(seed: int, count: int) -> list[bytes]:
    rng = random.Random(seed)
    return [script(rng, 12) for _ in range(count)]


def extended(seed: int, count: int) -> list[bytes]:
    rng = random.Random(seed)
    return [script(rng, 13 + index % 5) for index in range(count)]


def shutdown_cases(seed: int, count: int) -> list[bytes]:
    rng = random.Random(seed)
    return [script(rng, 18 + index % 3) for index in range(count)]


def reply_cancellations(seed: int, count: int) -> list[bytes]:
    rng = random.Random(seed)
    return [script(rng, 21) for _ in range(count)]


def shutdown_nested(seed: int, count: int) -> list[bytes]:
    rng = random.Random(seed)
    return [script(rng, 22) for _ in range(count)]


def concurrency_and_bursts(seed: int, count: int) -> list[bytes]:
    rng = random.Random(seed)
    return [script(rng, 23 + index % 2) for index in range(count)]


def tracked_callbacks(seed: int, count: int) -> list[bytes]:
    rng = random.Random(seed)
    return [script(rng, 25) for _ in range(count)]


def fuzz_script(rng: random.Random) -> bytes:
    count = rng.randrange(1, 5)
    chunks = []
    for index in range(count):
        family = rng.randrange(26)
        if family in (9, 22) and index + 1 < count:
            family = 14
        chunks.append(script(rng, family, index + 1, index * 1000000))
    return b''.join(chunks)
