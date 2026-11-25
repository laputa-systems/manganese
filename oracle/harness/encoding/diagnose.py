import os
import pathlib
import struct
import sys
from compare import ask, peer

store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
rows = [row.split() for row in pathlib.Path('goldens.lock').read_text().splitlines()]
row = next(row for row in rows if row[:3] == ['encoding', sys.argv[1], sys.argv[2]])
record = (store / row[3]).read_bytes()
if record[0] != 4:
    raise SystemExit('not a batch')
count = struct.unpack_from('>I', record, 1)[0]
pos = 5
oracle = peer('.work/tmp/encoding/oracle')
candidate = peer('.work/tmp/target/x86_64-unknown-linux-musl/debug/encoding')
for i in range(count):
    length = struct.unpack_from('>I', record, pos)[0]
    pos += 4
    item = record[pos:pos + length]
    pos += length
    expected, actual = ask(oracle, item), ask(candidate, item)
    if expected != actual:
        print('case', i, 'request', item.hex(), 'oracle', expected.hex(), 'candidate', actual.hex())
oracle.stdin.close()
candidate.stdin.close()
if oracle.wait() or candidate.wait():
    raise SystemExit('peer failed')
