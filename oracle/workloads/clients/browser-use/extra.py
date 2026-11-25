import json
import time

goto_url('http://forms.test/workload')
wait_for_load()
activate_tab(current_tab())
for _ in range(100):
    if js('window.crossReady === true'):
        break
    time.sleep(0.1)
else:
    raise RuntimeError('cross-origin frame did not become ready')
targets = cdp('Target.getTargets')
frame = js("(()=>{let r=document.querySelector('#cross').getBoundingClientRect();return {x:r.x+30,y:r.y+15}})()")
hit = cdp('DOM.getNodeForLocation', x=int(frame['x']), y=int(frame['y']))
node = cdp('DOM.describeNode', backendNodeId=hit['backendNodeId'])
for attempt in range(100):
    cdp('Input.dispatchMouseEvent', type='mouseMoved', x=frame['x'] + attempt % 2, y=frame['y'])
    time.sleep(0.1)
    if any(event['type'] == 'pointermove' and event['target'] == 'cross-click' for event in js('window.crossEvents')):
        break
else:
    raise RuntimeError('cross-origin button did not receive pointer movement')
click_at_xy(frame['x'], frame['y'])
for _ in range(100):
    events = js('window.crossEvents')
    if any(event['type'] == 'click' for event in events):
        break
    time.sleep(0.1)
print(json.dumps({'targets':targets, 'same':js("document.querySelector('#same').contentDocument.querySelector('#same-text').textContent"), 'frame':frame, 'hit':hit, 'node':node, 'events':events}))
document = cdp('DOM.getDocument')
node = cdp('DOM.querySelector', nodeId=document['root']['nodeId'], selector='#upload')
cdp('DOM.setFileInputFiles', nodeId=node['nodeId'], files=['/profile/browser-use-upload.bin'])
button = js("(()=>{let r=document.querySelector('#send-upload').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()")
click_at_xy(button['x'], button['y'])
cdp('Page.enable')
js("setTimeout(()=>document.querySelector('#confirm').click(),100);true")
time.sleep(0.5)
cdp('Page.handleJavaScriptDialog', accept=True)
print(json.dumps({'confirm':js('document.body.dataset.confirm')}))
button = js("(()=>{let r=document.querySelector('#popup').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()")
click_at_xy(button['x'], button['y'])
time.sleep(1)
targets = cdp('Target.getTargets')['targetInfos']
print(json.dumps({'popup_targets':targets}))
for target in targets:
    if target.get('url') == 'http://forms.test/popup':
        cdp('Target.closeTarget', targetId=target['targetId'])
cdp('Browser.setDownloadBehavior', behavior='allow', downloadPath='/home/browser/Downloads', eventsEnabled=True)
js("window.downloadClicks=0;document.querySelector('#blob').addEventListener('click',()=>window.downloadClicks++)")
button = js("(()=>{let r=document.querySelector('#blob').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()")
hit = js(f"document.elementFromPoint({button['x']},{button['y']})?.id")
click_at_xy(button['x'], button['y'])
print(json.dumps({'download_hit':hit, 'download_clicks':js('window.downloadClicks')}))
