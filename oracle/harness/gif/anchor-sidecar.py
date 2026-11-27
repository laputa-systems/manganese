import base64
import json
import sys
sys.path.insert(0, '/launch')
from cdp import Cdp

client = Cdp.connect_when_ready()
try:
    page = client.page()
    client.send('Fetch.enable', {'patterns': [{'urlPattern': 'https://gif.test/*'}]}, session=page)
    client.evaluate(page, "location.href='https://gif.test/'; 'navigating'")
    event = client.wait_event('Fetch.requestPaused', session=page)
    client.send('Fetch.fulfillRequest', {'requestId': event['requestId'], 'responseCode': 200, 'responseHeaders': [{'name': 'Content-Type', 'value': 'text/html'}], 'body': base64.b64encode(b'<!doctype html><title>gif</title>').decode()}, session=page)
    client.send('Fetch.disable', session=page)
    source = '''(async () => {
        const [images] = INPUTS;
        const decoded = [];
        for (const input of images) {
            const img = new Image();
            img.src = 'data:image/gif;base64,' + input;
            let failed = false;
            try { await img.decode(); } catch (_) { failed = true; }
            let pixels = null;
            if (!failed) {
                const canvas = document.createElement('canvas');
                canvas.width = img.naturalWidth; canvas.height = img.naturalHeight;
                const ctx = canvas.getContext('2d', {willReadFrequently: true, colorSpace: 'srgb'});
                ctx.drawImage(img, 0, 0);
                pixels = btoa(String.fromCharCode(...ctx.getImageData(0, 0, canvas.width, canvas.height).data));
            }
            decoded.push({failed, width: img.naturalWidth, height: img.naturalHeight, pixels});
        }
        return decoded;
    })()'''.replace('INPUTS', sys.argv[1])
    print(json.dumps(client.evaluate(page, source, timeout=90)))
finally:
    client.close()
