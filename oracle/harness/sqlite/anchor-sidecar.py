import base64
import json
import sys
import time

sys.path.insert(0, '/launch')
from cdp import Cdp

PAGES = {
    '/': b'<!doctype html><title>sqlite profile</title><link rel="icon" href="/icon.png"><a href="/second">second</a>',
    '/second': b'<!doctype html><title>second page \xce\xbb</title><link rel="icon" href="/icon.png">',
    '/third?q=search+terms': b'<!doctype html><title>third</title>',
}
ICON = base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==')

client = Cdp.connect_when_ready()
try:
    page = client.page()
    client.send('Fetch.enable', {'patterns': [{'urlPattern': 'https://sqlite.test/*'}]}, session=page)

    def fulfill(event):
        url = event['request']['url'].removeprefix('https://sqlite.test')
        body, kind = (ICON, 'image/png') if url == '/icon.png' else (PAGES.get(url, b'<!doctype html>'), 'text/html')
        headers = [{'name': 'Content-Type', 'value': kind}]
        if url == '/':
            headers += [{'name': 'Set-Cookie', 'value': 'session=abc123; Secure; HttpOnly; Path=/; Max-Age=86400'},
                        {'name': 'Set-Cookie', 'value': 'pref=lambda-value; Secure; SameSite=Lax; Path=/; Max-Age=3600'}]
        client.send('Fetch.fulfillRequest', {'requestId': event['requestId'], 'responseCode': 200,
                                              'responseHeaders': headers, 'body': base64.b64encode(body).decode()},
                    session=page)
        return url

    def serve(path):
        client.evaluate(page, f"location.href='https://sqlite.test{path}'; 'navigating'")
        while fulfill(client.wait_event('Fetch.requestPaused', session=page, timeout=30)) != path:
            pass
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            try:
                fulfill(client.wait_event('Fetch.requestPaused', session=page, timeout=deadline - time.monotonic()))
            except TimeoutError:
                break

    for path in PAGES:
        serve(path)
    result = client.evaluate(page, '''(async () => {
        document.cookie = 'script=' + 'x'.repeat(64) + '; Secure; Path=/; Max-Age=7200';
        const db = await new Promise((resolve, reject) => {
            const request = indexedDB.open('sqlite-profile', 1);
            request.onupgradeneeded = () => request.result.createObjectStore('records');
            request.onsuccess = () => resolve(request.result);
            request.onerror = () => reject(request.error);
        });
        await new Promise((resolve, reject) => {
            const tx = db.transaction('records', 'readwrite');
            tx.objectStore('records').put({text: 'profile'}, 'key');
            tx.oncomplete = resolve;
            tx.onerror = () => reject(tx.error);
        });
        db.close();
        const cache = await caches.open('sqlite-profile');
        await cache.put('https://sqlite.test/cached', new Response('cached body'));
        return document.cookie.split('; ').length;
    })()''', timeout=90)
    client.send('Fetch.disable', session=page)
    time.sleep(3)
    print(json.dumps({'cookies': result}))
finally:
    client.close()
