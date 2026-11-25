import base64
import json
import sys
import time
import urllib.request

sys.path.insert(0, '/launch')
from cdp import Cdp, CdpError

seed = sys.argv[1] == 'seed'
c = Cdp.connect()
p = c.page('http://forms.test/profile?seed' if seed else 'http://forms.test/profile')
for _ in range(200):
    try:
        if c.evaluate(p, "location.host === 'forms.test' && document.readyState === 'complete'"):
            break
    except Exception:
        pass
    time.sleep(0.1)
else:
    raise RuntimeError('profile fixture did not load')
if seed:
    c.evaluate(p, "localStorage.setItem('contract_profile','persisted')")
cookies = c.send('Network.getCookies', {'urls':['http://forms.test/']}, session=p)['cookies']
result = {'storage':c.evaluate(p, "localStorage.getItem('contract_profile')"),
          'cookies':[{'name':v['name'], 'value':v['value'], 'httpOnly':v['httpOnly']} for v in cookies]}
if seed:
    request = urllib.request.Request('http://127.0.0.1:18080/profile', headers={'Host':'forms.test'})
    with urllib.request.urlopen(request, timeout=5) as response:
        html = response.read()
    secure = c.page()
    c.send('Fetch.enable', {'patterns':[{'urlPattern':'https://forms.test/*'}]}, session=secure)
    c.evaluate(secure, "setTimeout(()=>location.href='https://forms.test/profile',0);true")
    paused = c.wait_event('Fetch.requestPaused', secure)
    c.send('Fetch.fulfillRequest', {'requestId':paused['requestId'], 'responseCode':200,
           'responseHeaders':[{'name':'Content-Type', 'value':'text/html'}],
           'body':base64.b64encode(html).decode()}, session=secure)
    c.send('Fetch.disable', session=secure)
    for _ in range(100):
        if c.evaluate(secure, "location.href==='https://forms.test/profile' && !!document.querySelector('#download')"):
            break
        time.sleep(0.1)
    else:
        raise RuntimeError('secure profile fixture did not load')
    c.evaluate(secure, "(()=>{let a=document.querySelector('#download');a.href=URL.createObjectURL(new Blob(['headed download evidence'],{type:'application/octet-stream'}));a.download='contract-profile.txt'})()")
    c.send('Browser.setDownloadBehavior', {'behavior':'default', 'eventsEnabled':True})
    c.send('Page.bringToFront', session=secure)
    rect = c.evaluate(secure, "(()=>{let r=document.querySelector('#download').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()")
    def click_download():
        c.send('Input.dispatchMouseEvent', {'type':'mousePressed', 'button':'left', 'clickCount':1, **rect}, session=secure)
        c.send('Input.dispatchMouseEvent', {'type':'mouseReleased', 'button':'left', 'clickCount':1, **rect}, session=secure)
        events = []
        try:
            events.append({'method':'Browser.downloadWillBegin', 'params':c.wait_event('Browser.downloadWillBegin', timeout=15)})
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                event = c.wait_event('Browser.downloadProgress', timeout=max(0.1, deadline-time.monotonic()))
                events.append({'method':'Browser.downloadProgress', 'params':event})
                if event.get('state') in ('completed', 'canceled'):
                    break
        except (TimeoutError, ConnectionError):
            pass
        return events

    default_events = click_download()
    try:
        histogram = c.send('Browser.getHistogram', {'name':'Download.CancelReason'})
    except CdpError as error:
        histogram = {'error':str(error)}
    canceled = any(x['method'] == 'Browser.downloadProgress' and x['params'].get('state') == 'canceled' for x in default_events)
    confirmation = any(x['low'] == 1 and x['count'] > 0 for x in histogram.get('histogram', {}).get('buckets', []))
    result['default_behavior_download'] = 'canceled-target-confirmation' if canceled and confirmation else 'other'
    result['default_download_events'] = default_events
    result['default_cancel_histogram'] = histogram
    c.send('Browser.setDownloadBehavior', {'behavior':'allow', 'downloadPath':'/home/browser/Downloads', 'eventsEnabled':True})
    result['download_events'] = click_download()
print(json.dumps(result))
c.close()
