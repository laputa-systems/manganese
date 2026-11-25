import os
import pathlib
from compare import ask, decode, peer

store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
candidate = peer('.work/tmp/target/x86_64-unknown-linux-musl/debug/url')
for row in pathlib.Path('goldens.lock').read_text().splitlines():
    fields = row.split()
    if fields[0] != 'url' or not any(tag in fields[2] for tag in ('-setter-', '-chromium-', '-suffix-', '-site-')): continue
    data = (store / fields[3]).read_bytes()
    expected = decode((store / fields[5]).read_bytes())
    actual = decode(ask(candidate, data))
    if expected != actual:
        difference = {key: [expected.get(key), actual.get(key)] for key in expected.keys() | actual.keys() if expected.get(key) != actual.get(key)}
        print(fields[2], data.hex(), repr(difference), flush=True)
candidate.stdin.close()
if candidate.wait(): raise SystemExit('candidate failed')
