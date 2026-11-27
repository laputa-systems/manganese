import base64
import json
import pathlib
import sys
sys.path.insert(0, '/launch')
from cdp import Cdp

cases = json.loads(pathlib.Path('/inputs/inputs.json').read_text())
engine = pathlib.Path('/regex/engine.js').read_text()

client = Cdp.connect_when_ready()
try:
    page = client.page()
    client.send('Fetch.enable', {'patterns': [{'urlPattern': 'https://regex.test/*'}]}, session=page)
    client.evaluate(page, "location.href='https://regex.test/'; 'navigating'")
    event = client.wait_event('Fetch.requestPaused', session=page)
    client.send('Fetch.fulfillRequest', {'requestId': event['requestId'], 'responseCode': 200,
                                         'responseHeaders': [{'name': 'Content-Type', 'value': 'text/html'}],
                                         'body': base64.b64encode(b'<!doctype html><title>regex</title>').decode()},
                session=page)
    client.send('Fetch.disable', session=page)
    client.evaluate(page, engine)
    results = []
    for start in range(0, len(cases), 16):
        batch = [[c['pattern'], c['flags'], c['subject'], c['iterations'], 0] for c in cases[start:start + 16]]
        script = 'JSON.stringify(' + json.dumps(batch) + '.map(function (c) { return __regexEngine(c[0], c[1], c[2], c[3], c[4]); }))'
        results.extend(json.loads(client.evaluate(page, script, timeout=120)))
    print(json.dumps(results))
finally:
    client.close()
