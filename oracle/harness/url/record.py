import hashlib
import json
import os
import pathlib
import struct
import subprocess
import urllib.request
from wire import request

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)

def blob(data):
    digest = hashlib.sha256(data).hexdigest()
    (store / digest).write_bytes(data)
    return digest, len(data)

process = subprocess.Popen(['.work/tmp/url/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
process.stdin.write(b'MGB1')
process.stdin.flush()
if process.stdout.read(4) != b'MGB1': raise SystemExit('invalid oracle header')
rows = []
seen_inputs = set()
corpus = pathlib.Path('.work/tmp/url/urltestdata.json')
if not corpus.exists():
    corpus.write_bytes(urllib.request.urlopen('https://raw.githubusercontent.com/web-platform-tests/wpt/225b2dff0e30c69e0393b0a5051a64ab1436a7d9/url/resources/urltestdata.json').read())
contents = corpus.read_bytes()
if hashlib.sha256(contents).hexdigest() != 'f11f7dc444cc9dcb01fd5c1de13720cdc55e579d0f0412e760c081403e3c4684':
    raise SystemExit('unexpected WPT corpus')
cases = json.loads(contents)
for index, case in enumerate(cases):
    if not isinstance(case, dict): continue
    data = request(case['input'], case.get('base'), None, index % 2)
    if data in seen_inputs: continue
    seen_inputs.add(data)
    process.stdin.write(struct.pack('>I', len(data)) + data)
    process.stdin.flush()
    header = process.stdout.read(4)
    if len(header) != 4: raise SystemExit(f'oracle failed at WPT case {index}')
    length = struct.unpack('>I', header)[0]
    result = process.stdout.read(length)
    if len(result) != length: raise SystemExit(f'short response at WPT case {index}')
    lane = 'holdout' if index % 3 == 0 else 'seen'
    a, b = blob(data), blob(result)
    rows.append(f'url {lane} {lane}-wpt-{index} {a[0]} {a[1]} {b[0]} {b[1]}')
process.stdin.close()
if process.wait(): raise SystemExit('oracle failed')
lock = pathlib.Path('goldens.lock')
existing = [line for line in lock.read_text().splitlines() if not line.startswith('url ')]
lock.write_text('\n'.join(existing + rows) + '\n')
print('recorded', len(rows))
