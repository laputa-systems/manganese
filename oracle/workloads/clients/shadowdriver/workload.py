import asyncio
import base64
import hashlib
import json
import sys
from pathlib import Path

from shadowdriver import connect_browser

sys.path.insert(0, '/launch')
from cdp import Cdp


async def main():
    browser, tab = await connect_browser('http://127.0.0.1:9221', stealth=False)
    try:
        version = await browser.command('Browser.getVersion')
        targets = await browser.command('Target.getTargets')
        await tab.navigate('http://forms.test/workload', wait='load')
        await tab.document_ready()
        url_api_initial = await tab.evaluate('({url:String(URL),create:typeof URL.createObjectURL,blob:typeof Blob,origin:location.origin})')
        user_agent = await tab.evaluate('navigator.userAgent')
        ax = await tab.command('Accessibility.getFullAXTree')
        screenshot = await tab.command('Page.captureScreenshot', {'format':'png'})
        png = base64.b64decode(screenshot['data'])
        (Path('/evidence') / 'shadowdriver.png').write_bytes(png)
        await tab.evaluate("document.querySelector('#text').value='shadowdriver submission'")
        button = await tab.evaluate("(()=>{const r=document.querySelector('#submit').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()")
        await tab.human_click(button['x'], button['y'])
        await asyncio.sleep(1)
        records = Path('/evidence/requests.jsonl')
        requests = [json.loads(line) for line in records.read_text().splitlines()] if records.exists() else []
        submitted = any(item['path'] == '/submit' and item['body'] == 'text=shadowdriver+submission' for item in requests)
        await tab.navigate('http://forms.test/workload', wait='load')
        await tab.document_ready()
        same_frame = await tab.evaluate("document.querySelector('#same').contentDocument.querySelector('#same-text').textContent")
        await browser.command('Target.setDiscoverTargets', {'discover':True})
        frame_targets = await browser.command('Target.getTargets')
        frame = await tab.evaluate("(()=>{let r=document.querySelector('#cross').getBoundingClientRect();return {x:r.x+30,y:r.y+15}})()")
        await tab.human_click(frame['x'], frame['y'])
        await asyncio.sleep(1)
        requests = [json.loads(line) for line in records.read_text().splitlines()] if records.exists() else []
        cross_clicked = any(item['path'] == '/frame-click' and item['body'] == 'clicked' for item in requests)
        before_popup = await browser.command('Target.getTargets')
        button = await tab.evaluate("(()=>{let r=document.querySelector('#popup').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()")
        await tab.human_click(button['x'], button['y'])
        await asyncio.sleep(1)
        after_popup = await browser.command('Target.getTargets')
        popup_ids = {item['targetId'] for item in after_popup['targetInfos'] if item.get('url') == 'http://forms.test/popup'}
        for popup_id in popup_ids:
            await browser.command('Target.closeTarget', {'targetId':popup_id})
        payload = bytes(range(256)) * 585 + bytes(range(240))
        upload = Path('/profile/shadowdriver-upload.bin')
        upload.write_bytes(payload)
        document = await tab.command('DOM.getDocument')
        node = await tab.command('DOM.querySelector', {'nodeId':document['root']['nodeId'], 'selector':'#upload'})
        await tab.command('DOM.setFileInputFiles', {'nodeId':node['nodeId'], 'files':[str(upload)]})
        button = await tab.evaluate("(()=>{let r=document.querySelector('#send-upload').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()")
        await tab.human_click(button['x'], button['y'])
        for _ in range(100):
            requests = [json.loads(line) for line in records.read_text().splitlines()] if records.exists() else []
            if any(item['path'] == '/upload' for item in requests):
                break
            await asyncio.sleep(0.1)
        uploaded = [item for item in requests if item['path'] == '/upload']
        dialogs = []
        async def accept_dialog(event):
            dialogs.append(event)
            await tab.command('Page.handleJavaScriptDialog', {'accept':True})
        tab.add_handler('Page.javascriptDialogOpening', accept_dialog)
        await tab.command('Page.enable')
        await tab.evaluate("setTimeout(()=>document.querySelector('#confirm').click(),0);true")
        for _ in range(100):
            if dialogs:
                break
            await asyncio.sleep(0.1)
        confirm = await tab.evaluate('document.body.dataset.confirm')
        monitor = Cdp.connect()
        monitor.send('Browser.setDownloadBehavior', {'behavior':'allow', 'downloadPath':'/home/browser/Downloads', 'eventsEnabled':True})
        await tab.activate()
        await tab.command('Page.bringToFront')
        await tab.evaluate("window.downloadClicks=0;window.downloadErrors=[];window.addEventListener('error',e=>window.downloadErrors.push(e.message));document.querySelector('#blob').addEventListener('click',e=>{window.downloadClicks++;window.downloadClick={trusted:e.isTrusted,active:navigator.userActivation.isActive}})")
        button = await tab.evaluate("(()=>{let r=document.querySelector('#blob').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()")
        hit = await tab.evaluate(f"document.elementFromPoint({button['x']},{button['y']})?.id")
        await tab.human_click(button['x'], button['y'])
        download = Path('/downloads/client-download.txt')
        for _ in range(100):
            if download.exists():
                break
            await asyncio.sleep(0.1)
        downloaded = download.read_bytes().decode() if download.exists() else None
        clicks = await tab.evaluate('window.downloadClicks')
        click_detail = await tab.evaluate('({click:window.downloadClick,errors:window.downloadErrors,handler:String(document.querySelector("#blob").onclick),url:String(URL),create:typeof URL.createObjectURL})')
        monitor.send('Browser.getVersion')
        events = [event for event in monitor.events if event.get('method', '').startswith('Browser.download')]
        monitor.close()
        print(json.dumps({'version':version, 'targets':targets, 'user_agent':user_agent,
                          'ax_nodes':len(ax['nodes']), 'png_bytes':len(png), 'png_signature':png[:8].hex(),
                          'form_submission':submitted, 'same_frame':same_frame, 'frame_targets':frame_targets,
                          'cross_clicked':cross_clicked, 'before_popup':before_popup, 'after_popup':after_popup,
                          'upload_expected_sha256':hashlib.sha256(payload).hexdigest(), 'uploads':uploaded,
                          'dialogs':dialogs, 'confirm':confirm, 'download':downloaded,
                          'download_clicks':clicks, 'download_url_api_initial':url_api_initial,
                          'download_click_detail':click_detail, 'download_hit':hit, 'download_events':events}))
    finally:
        await tab.close()
        await browser.close(terminate=False)


asyncio.run(main())
