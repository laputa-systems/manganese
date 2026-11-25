import json
import sys
import time

sys.path.insert(0, '/launch')
from cdp import Cdp


c = Cdp.connect()
extension = next(x for x in c.send('Extensions.getExtensions')['extensions'] if x['name'] == 'Browser Extensions' and x['enabled'])
extension_id = extension['id']
options = c.page(f'chrome-extension://{extension_id}/options.html')
for _ in range(100):
    if c.evaluate(options, "location.protocol === 'chrome-extension:' && document.readyState === 'complete'"):
        break
    time.sleep(0.1)
else:
    raise RuntimeError('extension options did not load')
target = c.send('Target.createTarget', {'url':'http://forms.test/workload'})['targetId']
page = c.send('Target.attachToTarget', {'targetId':target, 'flatten':True})['sessionId']
for _ in range(100):
    if c.evaluate(page, "location.href === 'http://forms.test/workload' && document.readyState === 'complete'"):
        break
    time.sleep(0.1)
else:
    raise RuntimeError('extension fixture did not load')
c.send('Target.setDiscoverTargets', {'discover':True, 'filter':[{'type':'tab'}]})
tab = next(x['targetId'] for x in c.send('Target.getTargets')['targetInfos'] if x['type'] == 'tab' and x['url'] == 'http://forms.test/workload')
action = c.send('Extensions.triggerAction', {'id':extension_id, 'targetId':tab})
action_targets = []
for _ in range(100):
    action_targets = c.send('Target.getTargets', {'filter':[{}]})['targetInfos']
    if any('/popup.html' in x.get('url', '') for x in action_targets):
        break
    time.sleep(0.1)
active = c.evaluate(options, '''(async()=>{
    const tabs=await chrome.tabs.query({active:true,currentWindow:true});
    const tab=tabs[0];
    const script=await chrome.scripting.executeScript({target:{tabId:tab.id},func:()=>document.title});
    const toggle=await chrome.runtime.sendMessage({kind:'allow-right-click/toggle',tabId:tab.id});
    const status=await chrome.runtime.sendMessage({kind:'allow-right-click/status',tabId:tab.id});
    return {tab,script,toggle,status,badge:await chrome.action.getBadgeText({tabId:tab.id}),title:await chrome.action.getTitle({tabId:tab.id})};
})()''')
external = c.page('https://www.youtube.com/contract')
for _ in range(100):
    if c.evaluate(external, "location.href === 'https://www.youtube.com/contract' && document.readyState === 'complete'"):
        break
    time.sleep(0.1)
else:
    raise RuntimeError('external messaging fixture did not load')
message = c.evaluate(external, f"chrome.runtime.sendMessage({json.dumps(extension_id)}, {{action:'invalid-contract-action'}})")
web_request = c.evaluate(options, '''(async()=>{
    let observed;
    const event=new Promise(resolve=>{
    let timer;const listener=details=>{chrome.webRequest.onSendHeaders.removeListener(listener);clearTimeout(timer);resolve({url:details.url,method:details.method})};
    chrome.webRequest.onSendHeaders.addListener(listener,{urls:['https://x.com/i/api/graphql/*']},['requestHeaders']);
    timer=setTimeout(()=>{chrome.webRequest.onSendHeaders.removeListener(listener);resolve(null)},5000);
    });
    await fetch('https://x.com/i/api/graphql/contract').catch(()=>{});
    observed=await event;return observed;
})()''')
rules = c.evaluate(options, '''(async()=>{
    const event=new Promise(resolve=>{
    let timer;const listener=details=>{chrome.webRequest.onSendHeaders.removeListener(listener);clearTimeout(timer);resolve(details.requestHeaders)};
    chrome.webRequest.onSendHeaders.addListener(listener,{urls:['https://www.youtube.com/youtubei/*']},['requestHeaders','extraHeaders']);
    timer=setTimeout(()=>{chrome.webRequest.onSendHeaders.removeListener(listener);resolve(null)},5000);
    });
    await fetch('https://www.youtube.com/youtubei/contract').catch(()=>{});
    return await event;
})()''')
print(json.dumps({'extension':extension,'action':action,'action_targets':action_targets,'active':active,'external':message,'webRequest':web_request,'rules':rules}))
c.close()
