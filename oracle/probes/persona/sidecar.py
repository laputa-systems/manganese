import base64
import json
import re
import sys
import urllib.request

sys.path.insert(0, '/launch')
from cdp import Cdp

with urllib.request.urlopen('http://127.0.0.1:9221/json/version', timeout=5) as response:
    version = json.load(response)
if not re.fullmatch(r'ws://127\.0\.0\.1:9221/devtools/browser/[0-9a-f-]{36}', version['webSocketDebuggerUrl']):
    raise RuntimeError('unexpected debugger URL structure')
version['webSocketDebuggerUrl'] = 'ws://127.0.0.1:9221/devtools/browser/<uuid>'
client = Cdp.connect()
try:
    page = client.page()
    client.send('Fetch.enable', {'patterns':[{'urlPattern':'https://persona.test/*'}]}, session=page)
    client.evaluate(page, "location.href='https://persona.test/'; 'navigating'")
    request = client.wait_event('Fetch.requestPaused', session=page)
    client.send('Fetch.fulfillRequest', {'requestId':request['requestId'], 'responseCode':200, 'responseHeaders':[{'name':'Content-Type','value':'text/html'}], 'body':base64.b64encode(b'<!doctype html><title>persona</title>').decode()}, session=page)
    client.send('Fetch.disable', session=page)
    with open('/persona/page.js', encoding='utf-8') as stream:
        result = client.evaluate(page, stream.read(), timeout=120)
    result['identity']['jsonVersion'] = version
    print(json.dumps(result, sort_keys=True))
finally:
    client.close()
