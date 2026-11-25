import base64
import json
import sys
sys.path.insert(0, '/launch')
from cdp import Cdp

client = Cdp.connect()
try:
    page = client.page()
    client.send('Fetch.enable', {'patterns':[{'urlPattern':'https://zlib.test/*'}]}, session=page)
    client.evaluate(page, "location.href='https://zlib.test/'; 'navigating'")
    event = client.wait_event('Fetch.requestPaused', session=page)
    client.send('Fetch.fulfillRequest', {'requestId':event['requestId'], 'responseCode':200, 'responseHeaders':[{'name':'Content-Type','value':'text/html'}], 'body':base64.b64encode(b'<!doctype html><title>zlib</title>').decode()}, session=page)
    client.send('Fetch.disable', session=page)
    source = '''(async () => { const inputs = INPUTS; const result = []; for (const format of ['deflate', 'deflate-raw', 'gzip']) for (const input of inputs) { const bytes = Uint8Array.from(atob(input), c => c.charCodeAt(0)); const stream = new Blob([bytes]).stream().pipeThrough(new CompressionStream(format)); const out = new Uint8Array(await new Response(stream).arrayBuffer()); result.push(btoa(String.fromCharCode(...out))); } return result; })()'''.replace('INPUTS', sys.argv[1])
    print(json.dumps(client.evaluate(page, source, timeout=90)))
finally:
    client.close()
