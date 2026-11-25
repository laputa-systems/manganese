import os
import base64
import json
import sys
import time

sys.path.insert(0, '/launch')
from cdp import Cdp
import checks

HTML = '''<!doctype html><script>
async function startAudio() {
 window.context = new AudioContext({sampleRate: 48000});
 let source = `class Pass extends AudioWorkletProcessor {
 constructor(){super();this.n=0} process(inputs,outputs){let i=inputs[0],o=outputs[0];for(let c=0;c<o.length;c++)if(i[c])o[c].set(i[c]);if(++this.n%256===0)this.port.postMessage(this.n);return true} }
 registerProcessor('pass',Pass);`;
 await context.audioWorklet.addModule(URL.createObjectURL(new Blob([source], {type:'text/javascript'})));
 window.node = new AudioWorkletNode(context,'pass',{numberOfInputs:1,numberOfOutputs:1,outputChannelCount:[2]});
 window.osc = new OscillatorNode(context,{frequency:220,type:'sine'});
 osc.connect(node).connect(context.destination); osc.start(); await context.resume();
 return {state:context.state,node:!!node,sampleRate:context.sampleRate};
}
</script>'''
client = Cdp.connect(os.environ.get('CDP_ENDPOINT', 'http://127.0.0.1:9221'))
client.send('Target.setDiscoverTargets', {'discover': True})
pages = []
targets = []
for _ in range(3):
    target = client.send('Target.createTarget', {'url': 'about:blank'})['targetId']
    session = client.send('Target.attachToTarget', {'targetId': target, 'flatten': True})['sessionId']
    client.send('Fetch.enable', {'patterns': [{'urlPattern': 'https://audio.test/*'}]}, session)
    client.evaluate(session, "setTimeout(() => location.href = 'https://audio.test/', 0); true")
    event = client.wait_event('Fetch.requestPaused', session)
    client.send('Fetch.fulfillRequest', {'requestId': event['requestId'], 'responseCode': 200,
                'responseHeaders': [{'name': 'Content-Type', 'value': 'text/html'}],
                'body': base64.b64encode(HTML.encode()).decode()}, session)
    client.send('Fetch.disable', session=session)
    for attempt in range(100):
        if client.evaluate(session, 'typeof startAudio') == 'function':
            break
        time.sleep(0.05)
    started = client.send('Runtime.evaluate', {'expression': 'startAudio()', 'userGesture': True,
                          'awaitPromise': True, 'returnByValue': True}, session, timeout=30)
    pages.append((session, started.get('result', {}).get('value')))
    targets.append(target)
target = client.send('Target.createTarget', {'url': 'about:blank'})['targetId']
session = client.send('Target.attachToTarget', {'targetId': target, 'flatten': True})['sessionId']
client.evaluate(session, """(()=>{const code='function task(){let t=performance.now();while(performance.now()-t<30){};setTimeout(task,0)}task()';window.workers=Array.from({length:3},()=>new Worker(URL.createObjectURL(new Blob([code]))));return workers.length})()""")
targets.append(target)
measurements = []
start = time.monotonic()
for index in range(13):
    delay = start + 5 * (index + 1) - time.monotonic()
    if delay > 0:
        time.sleep(delay)
    state = [client.evaluate(session_id, '({state:context.state,node:!!node,time:context.currentTime})') for session_id, _ in pages]
    listed = [item['targetId'] for item in client.send('Target.getTargets')['targetInfos']]
    measurements.append({'seconds': time.monotonic() - start, 'contexts': state, 'listed': all(t in listed for t in targets)})
events = [event for event in client.events if event.get('method') in ('Target.targetCrashed', 'Target.targetDestroyed') and event.get('params', {}).get('targetId') in targets]
ok = checks.audio([info for _, info in pages], measurements, events)
print(json.dumps({'ok': ok, 'started': [info for _, info in pages], 'measurements': measurements, 'events': events}))
client.close()
