import base64
import json
import sys
import time
import urllib.request

sys.path.insert(0, '/launch')
from cdp import Cdp, WebSocket
import checks


def get(path, host='127.0.0.1:9221'):
    request = urllib.request.Request('http://127.0.0.1:9221' + path, headers={'Host': host})
    with urllib.request.urlopen(request, timeout=10) as response:
        return json.load(response)


def run():
    result = {}
    version = get('/json/version')
    listing = get('/json/list')
    custom = get('/json/version', 'localhost:9221')
    client = Cdp(version['webSocketDebuggerUrl'])
    page = client.page()
    pages = get('/json/list')
    custom_pages = get('/json/list', 'localhost:9221')
    target = next(item for item in pages if item['type'] == 'page' and item.get('webSocketDebuggerUrl'))
    ws = WebSocket(target['webSocketDebuggerUrl'])
    ws.close()
    result['7.2'] = {'ok': checks.version(version), 'actual': version, 'expected': checks.VERSION}
    value = client.evaluate(page, 'typeof document.modelContext')
    result['7.7'] = {'ok': checks.webmcp(value), 'value': value}
    client.close()
    custom_urls = [item['webSocketDebuggerUrl'] for item in custom_pages if item.get('webSocketDebuggerUrl')]
    target_host_ok = bool(custom_urls) and all(url.startswith('ws://localhost:9221/devtools/page/') for url in custom_urls)
    result['7.1'] = {'ok': checks.endpoints(listing, custom['webSocketDebuggerUrl'], True, True, target_host_ok), 'version': custom, 'target': target['webSocketDebuggerUrl'], 'custom_target_host': target_host_ok}
    return result

print(json.dumps(run()))
