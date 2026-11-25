import os
import json
import sys
import time
sys.path.insert(0, '/launch')
from cdp import Cdp

c = Cdp.connect(os.environ.get('CDP_ENDPOINT', 'http://127.0.0.1:9221'))
methods = set()
events = set()
def send(method, params=None, session=None):
    result = c.send(method, params, session)
    methods.add(method)
    return result
def event(name, session=None):
    value = c.wait_event(name, session, timeout=15)
    events.add(name)
    return value

send('Target.setDiscoverTargets', {'discover': True})
target = send('Target.createTarget', {'url': 'about:blank'})['targetId']
event('Target.targetCreated')
session = send('Target.attachToTarget', {'targetId': target, 'flatten': True})['sessionId']
event('Target.attachedToTarget')
send('Target.setAutoAttach', {'autoAttach': True, 'waitForDebuggerOnStart': False, 'flatten': True})
send('Browser.getVersion')
send('Page.enable', session=session)
send('Page.setLifecycleEventsEnabled', {'enabled': True}, session)
send('Network.enable', session=session)
send('Network.setCacheDisabled', {'cacheDisabled': True}, session)
send('Emulation.setDeviceMetricsOverride', {'width': 800, 'height': 600, 'deviceScaleFactor': 1, 'mobile': False}, session)
send('Fetch.enable', {'patterns': [{'urlPattern': 'http://native.test/*'}]}, session)
send('Page.navigate', {'url': 'about:blank#native'}, session)
send('Runtime.evaluate', {'expression': "setTimeout(() => location.href = 'http://native.test/', 0); true", 'awaitPromise': True}, session)
paused = event('Fetch.requestPaused', session)
import base64
html = b'<html><body><input id="field"><button id="button">click</button><iframe id="frame" srcdoc="child"></iframe></body></html>'
send('Fetch.fulfillRequest', {'requestId': paused['requestId'], 'responseCode': 200, 'responseHeaders': [{'name':'Content-Type','value':'text/html'}], 'body':base64.b64encode(html).decode()}, session)
for name in ('Page.frameNavigated', 'Page.lifecycleEvent', 'Page.frameAttached'):
    event(name, session)
for _ in range(100):
    if c.evaluate(session, "!!document.querySelector('#button')"):
        break
    time.sleep(0.05)
send('Page.bringToFront', session=session)
eval_result = send('Runtime.evaluate', {'expression': "document.querySelector('#field').focus(); Promise.resolve(location.href)", 'awaitPromise': True, 'returnByValue': True}, session)
send('Input.insertText', {'text':'hello'}, session)
send('Input.dispatchKeyEvent', {'type':'keyDown', 'key':'a', 'code':'KeyA', 'windowsVirtualKeyCode':65}, session)
send('Input.dispatchKeyEvent', {'type':'keyUp', 'key':'a', 'code':'KeyA', 'windowsVirtualKeyCode':65}, session)
send('Input.dispatchMouseEvent', {'type':'mouseMoved', 'x':10, 'y':10}, session)
doc = send('DOM.getDocument', {'depth': -1, 'pierce': True}, session)['root']
node = send('DOM.querySelector', {'nodeId': doc['nodeId'], 'selector':'#button'}, session)['nodeId']
frame_node = send('DOM.querySelector', {'nodeId': doc['nodeId'], 'selector':'#frame'}, session)['nodeId']
described = send('DOM.describeNode', {'nodeId': node}, session)['node']
send('DOM.getBoxModel', {'nodeId': node}, session)
send('DOM.getContentQuads', {'nodeId': node}, session)
send('DOM.scrollIntoViewIfNeeded', {'nodeId': node}, session)
remote = send('DOM.resolveNode', {'nodeId': node, 'objectGroup':'contract'}, session)['object']['objectId']
send('Runtime.getProperties', {'objectId': remote, 'ownProperties': True}, session)
send('Runtime.callFunctionOn', {'objectId':remote,'functionDeclaration':'function(){return this.textContent}', 'returnByValue':True}, session)
send('Runtime.releaseObjectGroup', {'objectGroup':'contract'}, session)
send('Accessibility.getFullAXTree', session=session)
send('Accessibility.getPartialAXTree', {'nodeId':node}, session)
frame_tree = send('Page.getFrameTree', session=session)['frameTree']
stable_frame = send('Page.getFrameTree', session=session)['frameTree']['frame']['id']
stable_document = send('DOM.getDocument', session=session)['root']['backendNodeId']
stable_target = target in [x['targetId'] for x in send('Target.getTargets')['targetInfos']]
child = frame_tree['childFrames'][0]['frame']['id']
send('DOM.getFrameOwner', {'frameId': child}, session)
send('Page.createIsolatedWorld', {'frameId': child, 'worldName':'contract'}, session)
send('Network.setCookie', {'name':'contract', 'value':'yes', 'url':'http://native.test/'}, session)
screenshot = send('Page.captureScreenshot', {'format':'png'}, session)['data']
send('Runtime.evaluate', {'expression':'setTimeout(()=>confirm("contract"),0)', 'awaitPromise': True}, session)
event('Page.javascriptDialogOpening', session)
send('Page.handleJavaScriptDialog', {'accept': True}, session)
event('Page.javascriptDialogClosed', session)
send('Runtime.evaluate', {'expression':"document.querySelector('#frame').remove()", 'awaitPromise':True}, session)
event('Page.frameDetached', session)
send('Target.getTargets')
send('Target.detachFromTarget', {'sessionId':session})
event('Target.detachedFromTarget')
send('Target.closeTarget', {'targetId': target})
event('Target.targetDestroyed')
for e in list(c.events):
    if e.get('method') in ('Target.targetInfoChanged',):
        events.add(e['method'])
print(json.dumps({'methods': sorted(methods), 'events': sorted(events), 'evaluation':eval_result, 'screenshot_bytes':len(base64.b64decode(screenshot)), 'frame_id':frame_tree['frame']['id'], 'document_backend_id':doc['backendNodeId'], 'button_backend_id':described['backendNodeId'], 'stable':stable_target and stable_frame == frame_tree['frame']['id'] and stable_document == doc['backendNodeId']}))
c.close()
