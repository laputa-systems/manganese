import json
import shutil
import time
from collections.abc import Callable
from pathlib import Path

import checks
import client_sdk
import durability
import launch


def copy_extension(image: str, destination: Path) -> None:
    container = launch.checked(['docker', 'create', image]).strip()
    try:
        launch.checked(['docker', 'cp', f'{container}:/src/build/extension-suite', str(destination)])
    finally:
        launch.run(['docker', 'rm', '-f', container])


def run(directory: Path, browser_root: Path, image: str, preferences: Callable[[Path, bool], object]) -> dict[str, dict[str, object]]:
    directory.mkdir(parents=True)
    switch_dir = directory / 'switch'
    switch_dir.mkdir()
    session = launch.Session(switch_dir, browser_root, f'manganese-extension-switch-{directory.parent.name}')
    try:
        preferences(session.profile, True)
        copy_extension(image, session.profile / 'extension')
        manifest = json.loads((session.profile / 'extension/manifest.json').read_text())
        shutil.copyfile(launch.LAUNCH / 'contract/fixture-key.pem', switch_dir / 'tls-key.pem')
        shutil.copyfile(launch.LAUNCH / 'contract/fixture-cert.pem', switch_dir / 'tls-cert.pem')
        session.sidecar(['python3', '-B', '/launch/contract/fixture.py'], {switch_dir:'/evidence'}, detach=True)
        _, browser = session.start_browser('headed', extra=('--load-extension=/profile/extension',
                                           '--disable-extensions-except=/profile/extension', '--ignore-certificate-errors'))
        ready = session.probe()
        if not ready['readiness']['ok']:
            raise RuntimeError(f'headed browser not ready: {ready}')
        basic = json.loads(session.sidecar(['python3', '-B', '/launch/contract/extension_client.py'], timeout=90))
        live = json.loads(session.sidecar(['python3', '-B', '/launch/contract/extension_live.py'], timeout=90))
        download_info = basic['features']['downloads.download'].get('value', {})
        if download_info.get('state') == 'in_progress':
            raise RuntimeError(f'extension download did not finish: {download_info}')
        filename = download_info.get('filename', '').rsplit('/', 1)[-1]
        download = session.downloads / filename
        for _ in range(100):
            if download.is_file():
                break
            time.sleep(0.1)
        downloaded = download.read_bytes().decode() if filename and download.is_file() else None
        if download_info.get('state') == 'complete' and downloaded is None:
            raise RuntimeError(f'completed extension download is not observable: {download_info}')
        durability.close(session, browser)
    finally:
        session.close()
    cdp_dir = directory / 'cdp'
    cdp_dir.mkdir()
    session = launch.Session(cdp_dir, browser_root, f'manganese-extension-cdp-{directory.parent.name}')
    try:
        preferences(session.profile, True)
        copy_extension(image, session.profile / 'extension')
        session.sidecar(['python3', '-B', '/launch/contract/fixture.py'], {cdp_dir:'/evidence'}, detach=True)
        _, browser = session.start_browser('headed')
        ready = session.probe()
        if not ready['readiness']['ok']:
            raise RuntimeError(f'headed browser not ready: {ready}')
        control = json.loads(session.sidecar(['python3', '-B', '/launch/contract/extension_control.py'], timeout=90))
        durability.close(session, browser)
    finally:
        session.close()
    feature = basic['features']
    active = live['active']
    if live['webRequest'] is None or live['rules'] is None or live['external'] is None:
        raise RuntimeError(f'extension events were not observable: {live}')
    outcome = client_sdk.outcome
    result = {}
    def record(name: str, ok: bool, evidence: object) -> None:
        result[name] = outcome(ok, {'observed':evidence})
    record('switch-load', basic['extension']['enabled'], basic['extension'])
    record('cdp-load', bool(control['id']), control)
    record('get-extensions', any(x['id'] == control['id'] and x['enabled'] for x in control['extensions']), control['extensions'])
    record('trigger-action', control['action'] == {}, control['action'])
    record('service-worker', manifest['background']['type'] == 'module' and any(x['type'] == 'service_worker' and x['url'].endswith('/service-worker.js') for x in basic['targets']), {'manifest':manifest['background'], 'targets':basic['targets']})
    record('options', any(x.get('url', '').endswith('/options.html') for x in basic['targets']), basic['targets'])
    record('popup', feature['popup'].get('value') == 'Browser Extensions' and any('/popup.html' in x.get('url','') for x in live['action_targets']), {'title':feature['popup'], 'targets':live['action_targets']})
    record('badge', active['badge'] == 'ON', active)
    record('active-tab', active['tab'].get('url') == 'http://forms.test/workload', active)
    record('scripting.executeScript', any(x.get('result') == 'Client workload' for x in active['script']), active['script'])
    record('runtime.onMessage', active['toggle'].get('ok') and active['status'].get('status') == 'on', active)
    record('runtime.onConnect', feature['runtime.connect'].get('value', {}).get('kind') == 'chatgpt/lease:ready', feature['runtime.connect'])
    record('runtime.onMessageExternal', 'action' in live['external'].get('error', ''), live['external'])
    record('externally_connectable', 'https://www.youtube.com/*' in manifest['externally_connectable']['matches'] and 'action' in live['external'].get('error', ''), {'manifest':manifest['externally_connectable'], 'response':live['external']})
    mapping = {
        'runtime.sendMessage':lambda v: 'status' in v,
        'runtime.connect':lambda v: v.get('kind') == 'chatgpt/lease:ready',
        'runtime.getContexts':lambda v: any(x['contextType'] == 'BACKGROUND' for x in v),
        'runtime.getURL':lambda v: v.endswith('/options.html'),
        'runtime.openOptionsPage':lambda v: any(x.endswith('/options.html') for x in v),
        'runtime.lastError':lambda v: bool(v),
        'storage.local':lambda v: v.get('contract') == 'persisted',
        'tabs.query':lambda v: bool(v),
        'tabs.get':lambda v: bool(v.get('id')),
        'windows.update':lambda v: v.get('focused') is True,
        'cookies.get':lambda v: v.get('value') == 'fixture',
        'offscreen.createDocument':lambda v: any(x['contextType'] == 'OFFSCREEN_DOCUMENT' for x in v),
        'notifications.create':lambda v: bool(v),
        'clipboardWrite':lambda v: v == 'written',
    }
    for name, predicate in mapping.items():
        observed = feature[name]
        value = observed.get('value')
        record(name, 'value' in observed and predicate(value), observed)
    tabs = feature['tabs.create-update-remove']
    value = tabs.get('value', {})
    for name, ok in [('tabs.create', bool(value.get('id'))), ('tabs.update', bool(value.get('updated'))),
                     ('tabs.onUpdated', bool(value.get('updated'))), ('tabs.onRemoved', value.get('id') in value.get('removed', []))]:
        record(name, ok, tabs)
    record('downloads.download', downloaded == 'extension-contract' and filename == 'extension-contract.txt',
           {'api':feature['downloads.download'], 'downloaded':downloaded, 'requested_filename':'extension-contract.txt'})
    record('webRequest.onSendHeaders', live['webRequest'].get('url') == 'https://x.com/i/api/graphql/contract', live['webRequest'])
    headers = {x['name'].lower():x['value'] for x in live['rules']}
    record('declarativeNetRequest', 'youtube-headers' in feature['declarativeNetRequest'].get('value', []) and
           headers.get('origin') == 'https://www.youtube.com' and headers.get('referer') == 'https://www.youtube.com/',
           {'rules':feature['declarativeNetRequest'], 'headers':headers})
    if set(result) != checks.EXTENSION_FEATURES:
        raise RuntimeError(f'extension feature inventory mismatch: {set(result) ^ checks.EXTENSION_FEATURES}')
    return result
