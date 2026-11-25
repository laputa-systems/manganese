import json
import time

import checks
import client_sdk
import durability
import launch


def run(directory, browser_root, image, preferences, mode):
    if mode not in ('cli', 'library'):
        raise ValueError(f'unknown browser-use mode: {mode}')
    directory.mkdir(parents=True)
    session = launch.Session(directory, browser_root, f'manganese-client-bu-{mode}-{directory.parent.name}')
    workload = launch.ROOT / 'oracle/workloads/clients/browser-use'
    mounts = {directory:'/evidence', session.profile:'/profile', session.downloads:'/downloads', workload:'/workload:ro'}
    try:
        preferences(session.profile, True)
        session.sidecar(['python3', '-B', '/launch/contract/fixture.py'], {directory:'/evidence'}, detach=True)
        browser = durability.start(session)
        if mode == 'cli':
            for script in ('workload.sh', 'extra.sh', 'cycles.sh'):
                session.sidecar(['sh', f'/workload/{script}'], mounts, image=image, timeout=180)
            sockets = {'before':(directory / 'browser-use-sockets-before').read_text(),
                       'after':(directory / 'browser-use-sockets-after').read_text(), 'cycles':10}
        else:
            sockets = json.loads(session.sidecar(['python', '-B', '/workload/library.py'], mounts, image=image, timeout=180))
        for _ in range(100):
            if (session.downloads / 'client-download.txt').is_file():
                break
            time.sleep(0.1)
        command = (lambda phase: ['sh', '/workload/profile.sh', phase]) if mode == 'cli' else (
            lambda phase: ['env', f'PROFILE_MODE={phase}', 'python', '-B', '/workload/library.py'])
        session.sidecar(command('seed'), mounts, image=image, timeout=60)
        durability.close(session, browser)
        browser = durability.start(session)
        session.sidecar(command('read'), mounts, image=image, timeout=60)
        durability.close(session, browser)
    finally:
        session.close()
    core = (directory / 'browser-use.txt').read_text().splitlines()
    discovery, page = (json.loads(line) for line in core[:2])
    extra = [json.loads(line) for line in (directory / 'browser-use-extra.txt').read_text().splitlines()]
    frame, dialog, popup, click = extra
    if click['download_hit'] != 'blob' or click['download_clicks'] != 1:
        raise RuntimeError(f'download click did not reach the fixture: {click}')
    records = [json.loads(line) for line in (directory / 'requests.jsonl').read_text().splitlines()]
    uploads = [item for item in records if item['path'] == '/upload']
    if not uploads or not any(item['path'] == '/frame-click' for item in records):
        raise RuntimeError(f'the fixture did not record the cross-origin click or upload: {frame}')
    expected = (directory / 'browser-use-upload.sha256').read_text().split()[0]
    download = session.downloads / 'client-download.txt'
    download_ok = download.is_file() and download.read_bytes() == b'client download'
    cookies = [json.loads((directory / f'browser-use-profile-{phase}.json').read_text())['cookies'] for phase in ('seed', 'read')]
    cookie_ok = all(any(item['name'] == 'contract_profile' and item['value'] == 'persisted' and item['httpOnly']
                        for item in values) for values in cookies)
    png = (directory / 'browser-use.png').read_bytes()
    outcome = client_sdk.outcome
    return {
        'discovery':outcome(discovery['version']['product'] == checks.VERSION['Browser'] and bool(discovery['tabs']), discovery),
        'forms':outcome(any(item['path'] == '/submit' and item['body'] == 'text=browser-use+submission' for item in records), {'requests':records}),
        'accessibility':outcome(bool(page['ax']['nodes']), {'ax_nodes':len(page['ax']['nodes']), 'snapshot':'no separate snapshot API'}),
        'screenshot':outcome(png.startswith(b'\x89PNG\r\n\x1a\n') and len(png) > 8, {'bytes':len(png), 'signature':png[:8].hex()}),
        'dialogs':outcome(dialog['confirm'] == 'true', dialog),
        'popups':outcome(any(item['url'] == 'http://forms.test/popup' for item in popup['popup_targets']), popup),
        'frames':outcome(frame['same'] == 'same-origin frame' and any(item.get('type') == 'iframe' and item.get('url') == 'http://frame.test/cross'
                         for item in frame['targets']['targetInfos']), {'frame':frame, 'requests':records}),
        'page-js':outcome(page['user_agent'] == checks.VERSION['User-Agent'], {'user_agent':page['user_agent']}),
        'sessions':outcome(sockets['cycles'] == 10 and not sockets['before'] and not sockets['after'], sockets),
        'files':outcome(download_ok and any(item['length'] == 150000 and item['sha256'] == expected for item in uploads),
                        {'uploads':uploads, 'expected_sha256':expected, 'downloaded':download_ok, 'click':click}),
        'profiles':outcome(cookie_ok, {'cookies':cookies}),
    }
