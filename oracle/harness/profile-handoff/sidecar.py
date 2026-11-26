"""Sidecar that drives the frozen browser's profile through CDP."""

from __future__ import annotations

import base64
import json
import sys
import time
import urllib.error

sys.path.insert(0, '/launch')
from cdp import Cdp

ORIGIN = 'https://profile.test/'


def connect_when_ready(deadline_seconds: float = 60.0) -> Cdp:
    deadline = time.monotonic() + deadline_seconds
    while True:
        try:
            return Cdp.connect()
        except urllib.error.URLError as error:
            if not isinstance(error.reason, ConnectionRefusedError) or time.monotonic() >= deadline:
                raise
        time.sleep(0.05)


def page_at(client: Cdp) -> str:
    page = client.page()
    client.send('Fetch.enable', {'patterns': [{'urlPattern': 'https://profile.test/*'}]}, session=page)
    client.evaluate(page, f"location.href='{ORIGIN}'; 'navigating'")
    event = client.wait_event('Fetch.requestPaused', session=page)
    client.send('Fetch.fulfillRequest', {
        'requestId': event['requestId'], 'responseCode': 200,
        'responseHeaders': [{'name': 'Content-Type', 'value': 'text/html'}],
        'body': base64.b64encode(b'<!doctype html><title>profile handoff</title>').decode(),
    }, session=page)
    client.send('Fetch.disable', session=page)
    return page


WRITE_SCRIPT = '''(async () => {
    localStorage.setItem('alpha', 'hydrogen-alpha');
    localStorage.setItem('beta', 'hydrogen-beta');
    localStorage.removeItem('removed');
    const db = await new Promise((resolve, reject) => {
      const request = indexedDB.open('handoff', 1);
      request.onupgradeneeded = () => request.result.createObjectStore('records');
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    await new Promise((resolve, reject) => {
      const tx = db.transaction('records', 'readwrite');
      tx.objectStore('records').put('hydrogen-record', 'id');
      tx.oncomplete = resolve;
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error);
    });
    db.close();
    return {local: localStorage.length, alpha: localStorage.getItem('alpha'),
            beta: localStorage.getItem('beta'), indexed: 'hydrogen-record'};
})()'''

READ_SCRIPT = '''(async () => {
    const db = await new Promise((resolve, reject) => {
      const request = indexedDB.open('handoff', 1);
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    const record = await new Promise((resolve, reject) => {
      const request = db.transaction('records', 'readonly').objectStore('records').get('id');
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    db.close();
    return {local: localStorage.length, alpha: localStorage.getItem('alpha'),
            beta: localStorage.getItem('beta'), gamma: localStorage.getItem('gamma'),
            removed: localStorage.getItem('removed'), indexed: record};
})()'''

def main() -> int:
    mode = sys.argv[1]
    client = connect_when_ready()
    try:
        page = page_at(client)
        if mode == 'write':
            result = client.evaluate(page, WRITE_SCRIPT, timeout=90)
        elif mode == 'read':
            result = client.evaluate(page, READ_SCRIPT, timeout=90)
        else:
            raise RuntimeError(f'unknown mode {mode}')
        print(json.dumps(result))
        if not isinstance(result, dict):
            raise RuntimeError(f'{mode} returned {type(result).__name__}, want an object')
        if len(sys.argv) == 4 and result.get(sys.argv[2]) != sys.argv[3]:
            raise RuntimeError(f'{sys.argv[2]}: expected {sys.argv[3]!r}, observed {result.get(sys.argv[2])!r}')
    finally:
        client.close()
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
