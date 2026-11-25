import json
import sys
import time

sys.path.insert(0, '/launch')
from cdp import Cdp

c = Cdp.connect()
c.send('Browser.setDownloadBehavior', {'behavior':'allow', 'downloadPath':'/home/browser/Downloads', 'eventsEnabled':True})
extensions = c.send('Extensions.getExtensions')['extensions']
extension = next(item for item in extensions if item['name'] == 'Browser Extensions' and item['enabled'])
extension_id = extension['id']
target = c.send('Target.createTarget', {'url':f'chrome-extension://{extension_id}/options.html'})['targetId']
page = c.send('Target.attachToTarget', {'targetId':target, 'flatten':True})['sessionId']
for _ in range(100):
    if c.evaluate(page, "location.protocol === 'chrome-extension:' && document.readyState === 'complete'"):
        break
    time.sleep(0.1)
else:
    raise RuntimeError('extension options did not load')
features = c.evaluate(page, '''(async()=>{
    const out={};
    async function test(name, action){try{out[name]={value:await action()}}catch(e){out[name]={error:String(e)}}}
    await test('runtime.getURL',async()=>chrome.runtime.getURL('options.html'));
    await test('runtime.getContexts',async()=>await chrome.runtime.getContexts({}));
    await test('storage.local',async()=>{await chrome.storage.local.set({contract:'persisted'});return await chrome.storage.local.get('contract')});
    await test('tabs.query',async()=>await chrome.tabs.query({}));
    await test('tabs.get',async()=>{let t=(await chrome.tabs.query({active:true}))[0];return await chrome.tabs.get(t.id)});
    await test('runtime.sendMessage',async()=>{let t=(await chrome.tabs.query({active:true}))[0];return await chrome.runtime.sendMessage({kind:'allow-right-click/status',tabId:t.id})});
    await test('runtime.connect',async()=>await new Promise((resolve,reject)=>{let p=chrome.runtime.connect({name:'chatgpt/lease:contract:fixture'});let timer=setTimeout(()=>reject(Error('port response timeout')),5000);p.onMessage.addListener(message=>{clearTimeout(timer);p.disconnect();resolve(message)});p.onDisconnect.addListener(()=>{if(chrome.runtime.lastError){clearTimeout(timer);reject(Error(chrome.runtime.lastError.message))}})}));
    await test('runtime.openOptionsPage',async()=>{await chrome.runtime.openOptionsPage();return (await chrome.runtime.getContexts({contextTypes:['TAB']})).map(c=>c.documentUrl)});
    await test('cookies.get',async()=>{let url='https://www.youtube.com/';await chrome.cookies.set({url,name:'contract',value:'fixture'});let cookie=await chrome.cookies.get({url,name:'contract'});await chrome.cookies.remove({url,name:'contract'});return cookie});
    await test('downloads.download',async()=>{let id=await chrome.downloads.download({url:'data:text/plain,extension-contract',filename:'extension-contract.txt',conflictAction:'overwrite'});for(let i=0;i<100;i++){let [item]=await chrome.downloads.search({id});if(item&&item.state!=='in_progress')return item;await new Promise(resolve=>setTimeout(resolve,100))}return {id,state:'in_progress'}});
    await test('clipboardWrite',async()=>{await navigator.clipboard.writeText('extension contract');return 'written'});
    await test('tabs.create-update-remove',async()=>{let updated=[],removed=[],wake,targetId;let onUpdate=(id,change)=>{if(id===targetId){updated.push({id,change});if(wake)wake()}};let onRemove=id=>removed.push(id);chrome.tabs.onUpdated.addListener(onUpdate);chrome.tabs.onRemoved.addListener(onRemove);let tab=await chrome.tabs.create({url:chrome.runtime.getURL('options.html'),active:false});targetId=tab.id;let wait=new Promise(resolve=>{wake=resolve;setTimeout(resolve,5000)});await chrome.tabs.update(tab.id,{url:chrome.runtime.getURL('options.html#contract')});await wait;await chrome.tabs.remove(tab.id);chrome.tabs.onUpdated.removeListener(onUpdate);chrome.tabs.onRemoved.removeListener(onRemove);return {id:tab.id,updated,removed}});
    await test('windows.update',async()=>{let t=(await chrome.tabs.query({active:true}))[0];return await chrome.windows.update(t.windowId,{focused:true})});
    await test('runtime.lastError',async()=>await new Promise(resolve=>chrome.runtime.sendMessage('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',{},()=>resolve(chrome.runtime.lastError?.message||null))));
    await test('offscreen.createDocument',async()=>{await chrome.offscreen.createDocument({url:'features/onefile/offscreen.html',reasons:['BLOBS'],justification:'Contract fixture'});let contexts=await chrome.runtime.getContexts({contextTypes:['OFFSCREEN_DOCUMENT']});await chrome.offscreen.closeDocument();return contexts});
    await test('notifications.create',async()=>{let id=await chrome.notifications.create({type:'basic',iconUrl:'icons/128.png',title:'Contract',message:'Fixture'});await chrome.notifications.clear(id);return id});
    await test('declarativeNetRequest',async()=>await chrome.declarativeNetRequest.getEnabledRulesets());
    await test('popup',async()=>await chrome.action.getTitle({}));
    await test('badge',async()=>await chrome.action.getBadgeText({}));
    return out;
})()''')
targets = c.send('Target.getTargets')['targetInfos']
print(json.dumps({'extension':extension, 'targets':targets, 'features':features}))
c.close()
