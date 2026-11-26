import base64
import json
import sys
import time
import urllib.error

sys.path.insert(0, '/launch')
from cdp import Cdp


def connect_when_ready(deadline_seconds: float = 60.0) -> Cdp:
    """Retry only a refused connection, within the appliance readiness deadline (4.1)."""
    deadline = time.monotonic() + deadline_seconds
    while True:
        try:
            return Cdp.connect()
        except urllib.error.URLError as error:
            if not isinstance(error.reason, ConnectionRefusedError) or time.monotonic() >= deadline:
                raise
        time.sleep(0.05)


client = connect_when_ready()
try:
    page = client.page()
    client.send('Fetch.enable', {'patterns': [{'urlPattern': 'https://leveldb.test/*'}]}, session=page)
    client.evaluate(page, "location.href='https://leveldb.test/'; 'navigating'")
    event = client.wait_event('Fetch.requestPaused', session=page)
    client.send('Fetch.fulfillRequest', {
        'requestId': event['requestId'], 'responseCode': 200,
        'responseHeaders': [{'name': 'Content-Type', 'value': 'text/html'}],
        'body': base64.b64encode(b'<!doctype html><title>leveldb profile</title>').decode(),
    }, session=page)
    client.send('Fetch.disable', session=page)
    result = client.evaluate(page, '''(async () => {
        for (let i = 0; i < 80; ++i) {
            localStorage.setItem(`profile-${i}`, `${i}:` + 'local-storage-\u03bb-'.repeat(40 + i));
        }
        localStorage.removeItem('profile-2');
        const db = await new Promise((resolve, reject) => {
            const request = indexedDB.open('leveldb-profile', 1);
            request.onupgradeneeded = () => request.result.createObjectStore('records');
            request.onsuccess = () => resolve(request.result);
            request.onerror = () => reject(request.error);
        });
        const count = await new Promise((resolve, reject) => {
            const tx = db.transaction('records', 'readwrite');
            const store = tx.objectStore('records');
            for (let i = 0; i < 120; ++i) {
                store.put({index: i, text: 'indexed-value-\u03bb-'.repeat(20 + i % 31)}, `key-${i.toString().padStart(4, '0')}`);
            }
            store.delete('key-0003');
            tx.oncomplete = () => resolve(119);
            tx.onerror = () => reject(tx.error);
            tx.onabort = () => reject(tx.error);
        });
        let bulk = 0;
        for (let round = 0; round < 3; ++round) {
            bulk += await new Promise((resolve, reject) => {
                const tx = db.transaction('records', 'readwrite');
                const store = tx.objectStore('records');
                for (let i = 0; i < 400; ++i) {
                    const n = round * 400 + i;
                    store.put(`bulk-${n}:` + `${n * 7919 % 10007}-table-record-`.repeat(200), `bulk-${n.toString().padStart(5, '0')}`);
                }
                tx.oncomplete = () => resolve(400);
                tx.onerror = () => reject(tx.error);
                tx.onabort = () => reject(tx.error);
            });
        }
        db.close();
        return {local: localStorage.length, indexed: count, bulk};
    })()''', timeout=90)
    if result != {'local': 79, 'indexed': 119, 'bulk': 1200}:
        raise RuntimeError('profile workload did not complete: ' + repr(result))
    print(json.dumps(result))
finally:
    client.close()
