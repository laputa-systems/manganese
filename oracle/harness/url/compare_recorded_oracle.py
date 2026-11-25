import os
import pathlib

from compare import ask, peer

store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
oracle = peer('.work/tmp/url/oracle')
for row in pathlib.Path('goldens.lock').read_text().splitlines():
    fields = row.split()
    if fields[0] != 'url': continue
    expected = (store / fields[5]).read_bytes()
    actual = ask(oracle, (store / fields[3]).read_bytes())
    if expected != actual: print(fields[2], expected.hex(), actual.hex())
oracle.stdin.close()
if oracle.wait(): raise SystemExit('oracle failed')
