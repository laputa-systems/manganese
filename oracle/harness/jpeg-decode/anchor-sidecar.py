import base64
import json
import sys
sys.path.insert(0, '/launch')
from cdp import Cdp

client = Cdp.connect_when_ready()
try:
    page = client.page()
    client.send('Fetch.enable', {'patterns': [{'urlPattern': 'https://jpeg.test/*'}]}, session=page)
    client.evaluate(page, "location.href='https://jpeg.test/'; 'navigating'")
    event = client.wait_event('Fetch.requestPaused', session=page)
    client.send('Fetch.fulfillRequest', {'requestId': event['requestId'], 'responseCode': 200, 'responseHeaders': [{'name': 'Content-Type', 'value': 'text/html'}], 'body': base64.b64encode(b'<!doctype html><title>jpeg</title>').decode()}, session=page)
    client.send('Fetch.disable', session=page)
    images = json.load(open(sys.argv[1]))
    decoded = []
    source = '''(async () => {
        const input = INPUT;
        const img = new Image();
        img.src = 'data:image/jpeg;base64,' + input;
        let failed = false;
        try { await img.decode(); } catch (_) { failed = true; }
        let pixels = null;
        if (!failed && img.naturalWidth && img.naturalHeight) {
            const canvas = document.createElement('canvas');
            canvas.width = img.naturalWidth; canvas.height = img.naturalHeight;
            const ctx = canvas.getContext('2d', {willReadFrequently: true, colorSpace: 'srgb'});
            ctx.drawImage(img, 0, 0);
            const data = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
            let text = '';
            for (let i = 0; i < data.length; i += 32768) text += String.fromCharCode(...data.subarray(i, i + 32768));
            pixels = btoa(text);
        }
        return {failed, width: img.naturalWidth, height: img.naturalHeight, pixels};
    })()'''
    for item in images:
        decoded.append(client.evaluate(page, source.replace('INPUT', json.dumps(item)), timeout=90))
    print(json.dumps(decoded))
finally:
    client.close()
