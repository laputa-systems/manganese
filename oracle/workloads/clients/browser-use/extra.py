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
print(json.dumps({'targets':cdp('Target.getTargets'), 'same':js("document.querySelector('#same').contentDocument.querySelector('#same-text').textContent")}))
frame = js("(()=>{let r=document.querySelector('#cross').getBoundingClientRect();return {x:r.x+30,y:r.y+15}})()")
click_at_xy(frame['x'], frame['y'])
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
