import sys
from compare import ask, peer, request_decode

label = sys.argv[1].encode()
oracle = peer('.work/tmp/encoding/oracle')
candidate = peer('.work/tmp/target/x86_64-unknown-linux-musl/debug/encoding')
count = 0
for first in range(0x80, 0x100):
    for second in range(0x100):
        data = bytes((first, second))
        request = request_decode(label, 0, False, [data])
        expected, actual = ask(oracle, request), ask(candidate, request)
        if expected != actual:
            count += 1
            if count <= 30:
                print(data.hex(), expected.hex(), actual.hex())
print('differences', count)
oracle.stdin.close()
candidate.stdin.close()
if oracle.wait() or candidate.wait():
    raise SystemExit('peer failed')
