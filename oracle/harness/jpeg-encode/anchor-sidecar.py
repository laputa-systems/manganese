import base64
import json
import pathlib
import sys
sys.path.insert(0, '/launch')
from cdp import Cdp

cases = json.loads(pathlib.Path(sys.argv[1]).read_text())
source = '''(async () => {
    const results = [];
    for (const [width, height, quality, input] of INPUTS) {
        const canvas = document.createElement('canvas');
        canvas.width = width; canvas.height = height;
        const ctx = canvas.getContext('2d', {willReadFrequently: true, colorSpace: 'srgb'});
        const rgba = Uint8ClampedArray.from(atob(input), c => c.charCodeAt(0));
        ctx.putImageData(new ImageData(rgba, width, height), 0, 0);
        const readback = btoa(String.fromCharCode(...ctx.getImageData(0, 0, width, height).data));
        const url = canvas.toDataURL('image/jpeg', quality).split(',')[1];
        results.push([readback, url]);
    }
    return results;
})()'''

client = Cdp.connect_when_ready()
try:
    page = client.page()
    client.send('Fetch.enable', {'patterns': [{'urlPattern': 'https://jpeg.test/*'}]}, session=page)
    client.evaluate(page, "location.href='https://jpeg.test/'; 'navigating'")
    event = client.wait_event('Fetch.requestPaused', session=page)
    client.send('Fetch.fulfillRequest', {'requestId': event['requestId'], 'responseCode': 200, 'responseHeaders': [{'name': 'Content-Type', 'value': 'text/html'}], 'body': base64.b64encode(b'<!doctype html><title>jpeg</title>').decode()}, session=page)
    client.send('Fetch.disable', session=page)
    results = []
    for start in range(0, len(cases), 4):
        results.extend(client.evaluate(page, source.replace('INPUTS', json.dumps(cases[start:start + 4])), timeout=120))
    print(json.dumps(results))
finally:
    client.close()
