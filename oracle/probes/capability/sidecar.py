import json
import base64
import os
import sys

sys.path.insert(0, '/launch')
from cdp import Cdp, CdpError
from probe import processes

def service_worker(target, source):
    connection = Cdp.connect()
    try:
        page = connection.send('Target.attachToTarget', {'targetId':target, 'flatten':True})['sessionId']
        connection.send('Fetch.enable', {'patterns':[{'urlPattern':'https://capability.test/worker.js'}]}, session=page)
        connection.evaluate(page, "globalThis.swResult = new Promise((resolve, reject) => { navigator.serviceWorker.addEventListener('message', e => resolve(e.data), {once:true}); navigator.serviceWorker.register('/worker.js').then(r => { const w = r.active || r.installing || r.waiting; if (!w) { reject(new Error('no worker')); return; } const send = () => { if (w.state === 'activated') w.postMessage('inventory'); }; w.addEventListener('statechange', send); send(); }, reject); }); globalThis.swResult.catch(() => {}); 'started'")
        try:
            request = connection.wait_event('Fetch.requestPaused', session=page, timeout=5)
        except (ConnectionError, TimeoutError, OSError):
            return {'status':'unreachable', 'reason':'service worker script request was not intercepted'}
        inventory = source[source.index('  const inventory ='):source.index('  const gl =')]
        script = inventory + "\nself.addEventListener('install', e => self.skipWaiting()); self.addEventListener('activate', e => e.waitUntil(self.clients.claim())); self.addEventListener('message', e => e.source.postMessage(inventory(globalThis)));"
        connection.send('Fetch.fulfillRequest', {'requestId':request['requestId'], 'responseCode':200, 'responseHeaders':[{'name':'Content-Type','value':'text/javascript'}], 'body':base64.b64encode(script.encode()).decode()}, session=page)
        result = connection.evaluate(page, "Promise.race([globalThis.swResult.then(value => ({status:'reachable', inventory:value}), error => ({status:'unreachable', reason:error.name + ': ' + error.message})), new Promise(resolve => setTimeout(() => resolve({status:'unreachable', reason:'registration or worker message remained pending'}), 5000))])", timeout=10)
        return result
    except (CdpError, ConnectionError, OSError, TimeoutError) as error:
        return {'status':'unreachable', 'reason':type(error).__name__}
    finally:
        connection.close()


client = Cdp.connect()
try:
    page = client.page()
    client.send('Fetch.enable', {'patterns':[{'urlPattern':'https://capability.test/*'}]}, session=page)
    client.evaluate(page, "location.href='https://capability.test/'; 'navigating'")
    request = client.wait_event('Fetch.requestPaused', session=page)
    client.send('Fetch.fulfillRequest', {'requestId':request['requestId'], 'responseCode':200, 'responseHeaders':[{'name':'Content-Type','value':'text/html'}], 'body':'PCFkb2N0eXBlIGh0bWw+PHRpdGxlPnByb2JlPC90aXRsZT4='}, session=page)
    client.send('Fetch.disable', session=page)
    with open('/capability/probe.js', encoding='utf-8') as stream:
        source = stream.read()
    names = {'image/png':'png','image/jpeg':'jpg','image/webp':'webp','image/gif':'gif','image/jxl':'jxl','image/bmp':'bmp','image/x-icon':'ico','image/svg+xml':'svg'}
    fixtures = {kind:base64.b64encode(open('/capability/fixtures/sample.' + suffix, 'rb').read()).decode() for kind, suffix in names.items()}
    result = client.evaluate(page, source.replace('__IMAGE_FIXTURES__', json.dumps(fixtures)), timeout=90)
    target = client.send('Target.getTargetInfo', session=page)['targetInfo']['targetId']
    result['inventory']['serviceWorker'] = service_worker(target, source)
    found = []
    for item in processes():
        kind = next((arg for arg in ' '.join(item['argv']).split(' ') if arg in ('--type=zygote', '--type=renderer')), None)
        if kind or any('chrome_sandbox' in arg for arg in item['argv']):
            try:
                exe = os.readlink('/proc/' + str(item['pid']) + '/exe')
            except OSError:
                exe = 'inaccessible'
            try:
                user_namespace = os.readlink('/proc/' + str(item['pid']) + '/ns/user') != os.readlink('/proc/' + sys.argv[1] + '/ns/user')
            except OSError:
                user_namespace = 'unavailable'
            found.append({'user_namespace_distinct_from_browser':user_namespace, 'kind':kind or 'chrome_sandbox', 'executable':exe, 'no_new_privs':item['no_new_privs'], 'seccomp':item['seccomp'], 'uid':item['uid'], 'gid':item['gid']})
    result['processes'] = [json.loads(x) for x in sorted({json.dumps(item, sort_keys=True) for item in found})]
    if not any(x['kind'] == '--type=renderer' for x in found):
        raise RuntimeError('renderer missing from process scan')
    print(json.dumps(result, sort_keys=True))
finally:
    client.close()
