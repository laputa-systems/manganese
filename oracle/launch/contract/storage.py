import json
import sys
import time
sys.path.insert(0, '/launch')
from cdp import Cdp

mode = sys.argv[1]
c = Cdp.connect()
p = c.page('http://localhost/')
for _ in range(100):
    try:
        if c.evaluate(p, "location.href === 'http://localhost/' && document.readyState === 'complete'"):
            break
    except Exception:
        pass
    time.sleep(0.1)
else:
    raise RuntimeError('storage fixture did not load')

if mode == 'seed':
    expr = '''(async()=>{
    let db = await new Promise((resolve,reject)=>{let r=indexedDB.open('contract',1);r.onupgradeneeded=()=>r.result.createObjectStore('items');r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error)});
    await new Promise((resolve,reject)=>{let t=db.transaction('items','readwrite');t.objectStore('items').put('persisted','key');t.oncomplete=resolve;t.onerror=()=>reject(t.error)});db.close();
    let registration=await navigator.serviceWorker.register('/sw.js');await navigator.serviceWorker.ready;
    let cache=await caches.open('contract');await cache.put('/saved',new Response('persisted'));
    return {registration:registration.scope};})()'''
elif mode == 'read':
    expr = '''(async()=>{
    let db = await new Promise((resolve,reject)=>{let r=indexedDB.open('contract');r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error)});
    let value=await new Promise((resolve,reject)=>{let t=db.transaction('items');let r=t.objectStore('items').get('key');r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error)});db.close();
    let registration=await navigator.serviceWorker.getRegistration('/');let cache=await caches.open('contract');let response=await cache.match('/saved');
    return {indexeddb:value,service_worker:!!registration,cache:response?await response.text():null};})()'''
else:
    expr = '''new Promise((resolve,reject)=>{let r=indexedDB.open('contract');r.onerror=()=>reject(r.error);r.onsuccess=()=>{let db=r.result;let t=db.transaction('items','readwrite');let store=t.objectStore('items');let n=0;function write(){let request=store.put('x'.repeat(1048576),'interrupted'+(n++%256));request.onerror=()=>reject(request.error);request.onsuccess=()=>{if(n===1)resolve('write in progress');write()}}write()}})'''
print(json.dumps({'result':c.evaluate(p, expr, timeout=45)}))
c.close()
