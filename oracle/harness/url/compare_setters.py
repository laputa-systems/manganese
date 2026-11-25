import json
import os
import pathlib

from compare import ask, decode, peer
from wire import request

oracle = peer('.work/tmp/url/oracle')
candidate = peer('.work/tmp/target/x86_64-unknown-linux-musl/debug/url')
cases = json.loads(pathlib.Path('.work/tmp/url/setters_tests.json').read_text())
for name, entries in cases.items():
    if name == 'comment' or not isinstance(entries, list): continue
    for index, case in enumerate(entries):
        if not isinstance(case, dict) or 'href' not in case or 'new_value' not in case: continue
        setter = ('blink:' if os.environ.get('URL_SETTER_DIRECT') == '1' else '') + name
        data = request(case['href'], None, None, index % 2, (setter, case['new_value']))
        expected = decode(ask(oracle, data))
        actual = decode(ask(candidate, data))
        if expected != actual:
            difference = {key: [expected.get(key), actual.get(key)] for key in expected.keys() | actual.keys() if expected.get(key) != actual.get(key)}
            print(name, index, repr(case['href']), repr(case['new_value']), repr(difference), flush=True)
for process in (oracle, candidate):
    process.stdin.close()
    if process.wait(): raise SystemExit('peer failed')
