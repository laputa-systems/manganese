import random
import sys
from compare import ask, peer, request

random.seed(7211)
data = [b'', b'a', b'hello world' * 100, bytes(range(256)) * 20, b'a' * 5000,
        (b'abracadabra' * 1000 + b'xyz' * 100),
        bytes(random.randrange(256) for _ in range(8192)),
        (b'chromium compression stream deflate test\n' * 900)]
params = [(level, 15, 8, 0) for level in range(-1, 10)]
params += [(6, window, 8, 0) for window in list(range(8, 16)) + list(range(-15, -7)) + list(range(24, 32))]
params += [(6, 15, mem, 0) for mem in range(1, 10)]
params += [(6, 15, 8, strategy) for strategy in range(5)]
a, b = peer('.work/tmp/zlib/oracle'), peer('.work/tmp/target/x86_64-unknown-linux-musl/debug/zlib')
passed = total = 0
for level, window, mem, strategy in params:
    for i, value in enumerate(data):
        rec = request(0, level, window, mem, strategy, [(value, 4)])
        expected, actual = ask(a, rec), ask(b, rec)
        total += 1
        if expected == actual:
            passed += 1
        else:
            offset = next((j for j, (x, y) in enumerate(zip(expected, actual)) if x != y), min(len(expected), len(actual)))
            print(f'mismatch level={level} window={window} mem={mem} strategy={strategy} input={i} expected_len={len(expected)} actual_len={len(actual)} first_offset={offset} expected={expected[offset:offset+16].hex()} actual={actual[offset:offset+16].hex()}', flush=True)
print(f'deflate {passed}/{total}', flush=True)
for p in (a, b):
    p.stdin.close()
    assert p.wait() == 0
sys.exit(passed != total)
