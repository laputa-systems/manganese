import hashlib
import json
import os
import pathlib
import struct
import subprocess
import tempfile
import urllib.request

from wire import request

root = pathlib.Path(__file__).resolve().parents[3]
os.chdir(root)
corpus = pathlib.Path('.work/tmp/url/setters_tests.json')
if not corpus.exists():
    with urllib.request.urlopen('https://raw.githubusercontent.com/web-platform-tests/wpt/225b2dff0e30c69e0393b0a5051a64ab1436a7d9/url/resources/setters_tests.json', timeout=30) as response:
        corpus.write_bytes(response.read())
contents = corpus.read_bytes()
if hashlib.sha256(contents).hexdigest() != '662918385b1998218f8bf3df324662bb61d3efa1d74f65b4777579f410126972':
    raise SystemExit('unexpected WPT setters corpus')
store = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'
store.mkdir(parents=True, exist_ok=True)
lock = pathlib.Path('goldens.lock')
existing = lock.read_text().splitlines()
existing_inputs = {line.split()[3] for line in existing if line.startswith('url ')}
rows = []
cases = []

def write_blob(digest: str, data: bytes) -> None:
    path = store / digest
    if path.exists() and path.read_bytes() == data: return
    name = None
    try:
        with tempfile.NamedTemporaryFile(dir=store, prefix='.url-', delete=False) as output:
            name = output.name
            output.write(data)
        os.replace(name, path)
    finally:
        if name is not None and os.path.exists(name): os.unlink(name)
for name, entries in json.loads(contents).items():
    if name == 'comment' or not isinstance(entries, list): continue
    for index, case in enumerate(entries):
        if not isinstance(case, dict) or 'href' not in case or 'new_value' not in case: continue
        cases.append((f'blink-setter-{name}-{index}', request(case['href'], None, None, index % 2, ('blink:' + name, case['new_value']))))

for index, value in enumerate([
    'file:///home/me', 'file:c:\\foo\\bar.html', 'file:c|//foo\\bar.html', 'file:///./s:',
    'file://localhost', 'file://localhost/test', 'filesystem:http://www.Foo.com:80/tempoRary',
    'filesystem:https://www.foo.com/temporary/', 'filesystem:http://www.foo.com//',
    'filesystem:http://www.foo.com/persistent/bob?query#ref', 'filesystem:file:///temporary/Bob?qUery#reF',
    'http://0x7f.1/', 'http://0177.0.0.1/', 'http://256.256.256.256/', 'http://[::ffff:192.0.2.1]/',
    'https://bücher.example/', 'https://faß.de/', 'blob:https://example.com/id', 'blob:file://host/path',
    'data:text/plain,hello', 'javascript:alert(1)', 'about:blank', 'chrome://settings/',
]):
    cases.append((f'chromium-{index}', request(value, None, None, index % 2)))

hosts = ['a.example.co.uk', 'b.example.co.uk', 'a.blogspot.com', 'b.blogspot.com',
         'a.github.io', 'b.github.io', 'a.b.ck', 'www.ck', 'a.city.kawasaki.jp',
         'example.com.', '127.1', '[::1]', 'localhost', 'a.unknown']
for private in (0, 1):
    for index, first in enumerate(hosts):
        for second in (first, hosts[(index + 1) % len(hosts)], hosts[(index + 3) % len(hosts)]):
            cases.append((f'suffix-{private}-{index}-{len(cases)}', request('https://' + first + '/', None, 'http://' + second + '/', private)))

for index, (first, second) in enumerate([
    ('https://a.example.com/', 'https://b.example.com/'),
    ('https://a.example.com/', 'http://b.example.com/'),
    ('https://a.blogspot.com/', 'https://b.blogspot.com/'),
    ('https://a.blogspot.com/', 'https://a.blogspot.com:8443/'),
    ('https://a.github.io/', 'https://b.github.io/'),
    ('http://127.1/', 'http://127.0.0.1/'),
    ('https://[::1]/', 'https://[::1]:444/'),
    ('file:///one', 'file:///two'),
    ('file://a.example.com/one', 'file://b.example.com/two'),
    ('data:text/plain,a', 'data:text/plain,a'),
    ('blob:https://a.example.com/id', 'https://b.example.com/'),
    ('https://example.com/', 'invalid'),
]):
    for private in (0, 1):
        cases.append((f'site-{index}-{private}', request(first, None, second, private, ('schemeful-site', ''))))

process = subprocess.Popen(['.work/tmp/url/oracle'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
process.stdin.write(b'MGB1')
process.stdin.flush()
if process.stdout.read(4) != b'MGB1': raise SystemExit('invalid oracle header')
for index, (name, data) in enumerate(cases):
    digest = hashlib.sha256(data).hexdigest()
    if digest in existing_inputs: continue
    existing_inputs.add(digest)
    process.stdin.write(struct.pack('>I', len(data)) + data)
    process.stdin.flush()
    header = process.stdout.read(4)
    if len(header) != 4: raise SystemExit(f'oracle failed at {name}')
    length = struct.unpack('>I', header)[0]
    result = process.stdout.read(length)
    if len(result) != length: raise SystemExit(f'short response at {name}')
    output_digest = hashlib.sha256(result).hexdigest()
    write_blob(digest, data)
    write_blob(output_digest, result)
    lane = 'holdout' if index % 5 < 2 else 'seen'
    rows.append(f'url {lane} {lane}-{name} {digest} {len(data)} {output_digest} {len(result)}')
process.stdin.close()
if process.wait(): raise SystemExit('oracle failed')
temporary = pathlib.Path('.work/tmp/url/goldens-extra.tmp')
try:
    temporary.write_text('\n'.join(existing + rows) + '\n')
    os.replace(temporary, lock)
finally:
    temporary.unlink(missing_ok=True)
print('recorded', len(rows))
