import json
import sys
import urllib.request

sys.path.insert(0, '/launch')
from cdp import Cdp, WebSocket
import checks


def get(path):
    with urllib.request.urlopen('http://127.0.0.1:9221' + path, timeout=10) as response:
        return json.load(response)

observed = {}

try:
    client = Cdp.connect()
    page = client.page()
    outgoing = client.evaluate(page, "'x'.repeat(32 * 1024 * 1024)", timeout=120)
    incoming = client.evaluate(page, "'" + 'x' * (32 * 1024 * 1024) + "'.length", timeout=120)
    observed['response_bytes'] = len(outgoing)
    observed['request_payload_bytes'] = incoming
    targets = client.send('Target.getTargets')['targetInfos']
    before_targets = len(targets)
    before_pages = len(get('/json/list'))
    url = get('/json/version')['webSocketDebuggerUrl']
    sockets = [Cdp(url) for _ in range(30)]
    target = next(item['targetId'] for item in targets if item['type'] == 'page')
    for ws in sockets:
        ws.send('Target.attachToTarget', {'targetId':target, 'flatten':True})
    for ws in sockets:
        ws.close()
    for _ in range(10):
        reconnect = Cdp.connect()
        reconnect.send('Browser.getVersion')
        reconnect.close()
    after_targets = len(client.send('Target.getTargets')['targetInfos'])
    after_pages = len(get('/json/list'))
    observed.update({'before_targets': before_targets, 'after_targets': after_targets,
                     'before_pages': before_pages, 'after_pages': after_pages})
    observed['checks_ok'] = checks.robust(len(outgoing), incoming, before_targets, after_targets, before_pages, after_pages)
    client.close()
except Exception as error:
    observed['checks_ok'] = False
    observed['checks_error'] = repr(error)
print(json.dumps(observed))
