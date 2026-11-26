import base64
import json
import sys
sys.path.insert(0, '/launch')
from cdp import Cdp

cases = json.load(open('/cases/cases.json'))
client = Cdp.connect_when_ready()
try:
    page = client.page()
    client.send('Fetch.enable', {'patterns': [{'urlPattern': 'https://media.test/*'}]}, session=page)
    client.evaluate(page, "location.href='https://media.test/'; 'navigating'")
    event = client.wait_event('Fetch.requestPaused', session=page)
    client.send('Fetch.fulfillRequest', {'requestId': event['requestId'], 'responseCode': 200,
                                         'responseHeaders': [{'name': 'Content-Type', 'value': 'text/html'}],
                                         'body': base64.b64encode(b'<!doctype html><title>media</title>').decode()}, session=page)
    client.send('Fetch.disable', session=page)
    source = '''(async () => {
        const media = new Audio();
        media.muted = true;
        media.preload = 'auto';
        const outcome = await new Promise(resolve => {
            media.onloadedmetadata = () => resolve('metadata');
            media.onerror = () => resolve('error');
            media.src = 'data:' + MIME + ';base64,' + INPUT;
        });
        const number = value => value === Infinity ? 'inf' : value;
        if (outcome === 'error') return {code: media.error.code, message: media.error.message};
        const duration = number(media.duration);
        media.playbackRate = 16;
        let ended_message = null;
        const ended = await new Promise(resolve => {
            media.onended = () => resolve(null);
            media.onerror = () => {
                ended_message = media.error.message;
                resolve(media.error.code);
            };
            setTimeout(() => resolve('timeout'), 60000);
            media.play().catch(error => resolve('play ' + error.name));
        });
        return {duration, final: number(media.duration), ended_code: ended, ended_message};
    })()'''
    observed = {}
    for name, case in cases.items():
        expression = source.replace('MIME', json.dumps(case['mime'])).replace('INPUT', json.dumps(case['data']))
        result = client.send('Runtime.evaluate', {'expression': expression, 'awaitPromise': True,
                                                  'returnByValue': True, 'userGesture': True}, session=page, timeout=90)
        if 'exceptionDetails' in result:
            raise RuntimeError(json.dumps(result['exceptionDetails'])[:2000])
        observed[name] = result['result']['value']
    print(json.dumps(observed))
finally:
    client.close()
