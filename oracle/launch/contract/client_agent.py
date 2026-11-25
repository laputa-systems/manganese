import json
import time

import checks
import client_sdk
import durability
import launch


def read(directory, name):
    value = json.loads((directory / name).read_text())
    if not value.get('success'):
        raise RuntimeError(f'{name}: {value}')
    return value['data']


def run(directory, browser_root, image, preferences):
    directory.mkdir(parents=True)
    session = launch.Session(directory, browser_root, f'manganese-client-agent-{directory.parent.name}')
    workload = launch.ROOT / 'oracle/workloads/clients/agent-browser'
    mounts = {directory:'/evidence', session.profile:'/profile', session.downloads:'/downloads', workload:'/workload:ro'}
    try:
        preferences(session.profile, True)
        session.sidecar(['python3', '-B', '/launch/contract/fixture.py'], {directory:'/evidence'}, detach=True)
        browser = durability.start(session)
        session.sidecar(['sh', '/workload/workload.sh'], mounts, image=image, timeout=180)
        session.sidecar(['python3', '-B', '/launch/contract/client_ax.py'], {directory:'/evidence'}, detach=True)
        session.sidecar(['sh', '/workload/extra.sh'], mounts, image=image, timeout=120)
        for _ in range(100):
            if (directory / 'full-ax.json').is_file():
                break
            time.sleep(0.1)
        else:
            raise RuntimeError('full AX observer did not finish')
        session.sidecar(['sh', '/workload/profile.sh', 'seed'], mounts, image=image, timeout=60)
        durability.close(session, browser)
        browser = durability.start(session)
        session.sidecar(['sh', '/workload/profile.sh', 'read'], mounts, image=image, timeout=60)
        durability.close(session, browser)
    finally:
        session.close()
    records = [json.loads(line) for line in (directory / 'requests.jsonl').read_text().splitlines()]
    version = json.loads((directory / 'version.json').read_text())
    endpoint = read(directory, 'cdp-url.json')
    tabs = read(directory, 'tabs.json')
    snapshot = read(directory, 'snapshot.json')
    ax = json.loads((directory / 'full-ax.json').read_text())
    png = (directory / 'agent-browser.png').read_bytes()
    targets = json.loads((directory / 'extra-targets.json').read_text())
    same = read(directory, 'same-frame.json')['result']
    dialog = read(directory, 'dialog-status.json')
    accepted = read(directory, 'dialog-accept.json')
    confirmation = read(directory, 'confirm-result.json')['result']
    popup = read(directory, 'popup-tabs.json')
    sockets = {'before':(directory / 'agent-sockets-before').read_text(), 'after':(directory / 'agent-sockets-after').read_text(), 'cycles':10}
    expected = (directory / 'agent-upload.sha256').read_text().split()[0]
    uploads = [item for item in records if item['path'] == '/upload']
    if not uploads or not any(item['path'] == '/frame-click' for item in records):
        raise RuntimeError('the fixture did not record the cross-origin click or upload')
    download = json.loads((directory / 'download.json').read_text())
    selector = read(directory, 'download-selector.json')
    if selector['count'] != 1:
        raise RuntimeError(f'download selector did not match the fixture: {selector}')
    if not download.get('success') and 'download is not supported over --cdp' not in download.get('error', ''):
        raise RuntimeError(f'download command failed: {download}')
    cookie_sets = [read(directory, f'agent-profile-{mode}-cookies.json')['cookies'] for mode in ('seed', 'read')]
    cookie_ok = all(any(item['name'] == 'contract_profile' and item['value'] == 'persisted' and item['httpOnly']
                        for item in cookies) for cookies in cookie_sets)
    upload_ok = any(item['length'] == 150000 and item['sha256'] == expected for item in uploads)
    downloaded = session.downloads / 'client-download.txt'
    download_ok = downloaded.is_file() and downloaded.read_bytes() == b'client download'
    outcome = client_sdk.outcome
    return {
        'discovery':outcome(version['Browser'] == checks.VERSION['Browser'] and bool(endpoint['cdpUrl']) and bool(tabs['tabs']),
                            {'version':version, 'endpoint':endpoint, 'tabs':tabs}),
        'forms':outcome(any(item['path'] == '/submit' and item['body'] == 'text=agent-browser+submission' for item in records),
                        {'requests':records}),
        'accessibility':outcome(bool(ax['nodes']) and bool(snapshot['snapshot']), {'ax_nodes':len(ax['nodes']), 'snapshot':snapshot}),
        'screenshot':outcome(png.startswith(b'\x89PNG\r\n\x1a\n') and len(png) > 8, {'bytes':len(png), 'signature':png[:8].hex()}),
        'dialogs':outcome(dialog['hasDialog'] and accepted['accepted'] and confirmation == 'true',
                          {'dialog':dialog, 'accepted':accepted, 'confirmation':confirmation}),
        'popups':outcome(any(item['url'] == 'http://forms.test/popup' for item in popup['tabs']), popup),
        'frames':outcome(same == 'same-origin frame' and any(item['type'] == 'iframe' and item['url'] == 'http://frame.test/cross' for item in targets),
                         {'same':same, 'targets':targets, 'requests':records}),
        'page-js':outcome(read(directory, 'user-agent.json')['result'] == checks.VERSION['User-Agent'], read(directory, 'user-agent.json')),
        'sessions':outcome(not sockets['before'] and not sockets['after'], sockets),
        'files':outcome(upload_ok and download_ok, {'uploads':uploads, 'expected_sha256':expected, 'download':download, 'downloaded':download_ok}),
        'profiles':outcome(cookie_ok, {'cookies':cookie_sets}),
    }
