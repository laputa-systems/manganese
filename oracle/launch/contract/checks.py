import re

AD = 'pagead2.googlesyndication.com'
VERSION = {'Browser': 'Chrome/156.0.8078.79', 'Protocol-Version': '1.3', 'User-Agent': 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/156.0.0.0 Safari/537.36', 'V8-Version': '15.6.75.27', 'WebKit-Version': '537.36 (@a21ce95c24ce3964416f09ae1862f53f319eb4d0)', 'File-Path-Style': 'posix'}
CLIENTS = {'browser-use', 'agent-browser', 'extension-cli', 'shadowdriver-sdk', 'shadowdriver-crawler'}
WORKLOADS = {'discovery', 'forms', 'accessibility', 'screenshot', 'dialogs', 'popups', 'frames', 'page-js', 'sessions', 'files', 'profiles'}
EXTENSION_FEATURES = {'switch-load', 'cdp-load', 'get-extensions', 'trigger-action', 'service-worker', 'popup', 'badge', 'options', 'active-tab', 'scripting.executeScript', 'runtime.sendMessage', 'runtime.onMessage', 'runtime.connect', 'runtime.onConnect', 'runtime.getContexts', 'runtime.getURL', 'runtime.openOptionsPage', 'runtime.lastError', 'runtime.onMessageExternal', 'storage.local', 'tabs.query', 'tabs.get', 'tabs.create', 'tabs.update', 'tabs.onUpdated', 'tabs.onRemoved', 'windows.update', 'downloads.download', 'cookies.get', 'webRequest.onSendHeaders', 'declarativeNetRequest', 'offscreen.createDocument', 'notifications.create', 'clipboardWrite'}


def adblock(observed):
    enabled, disabled = observed
    return not any(x['host'] == AD for x in enabled['entries']) and any(x['host'] == AD for x in disabled['entries'])


def egress(clause, observed):
    enabled, disabled = observed
    entries = enabled['entries'] + disabled['entries']
    if clause == '5.1':
        return all(x['preferences'] == {'contract': {'sentinel':'kept'}, 'adblock': {'other':'kept', 'enabled':x['enabled']}} for x in observed) and adblock(observed)
    if clause == '5.2':
        return all(not x['local_state']['exists'] or x['local_state']['uid'] == 1000 for x in observed)
    if clause == '6.1':
        interfaces = [{line.split(':',1)[0].strip() for line in x['netdev'].splitlines() if ':' in line} for x in observed]
        return bool(entries) and all(x['version'] == 5 and x['command'] == 1 and x['address_type'] == 3 for x in entries) and all(interfaceset == {'lo'} for interfaceset in interfaces) and all(len(x['route'].splitlines()) == 1 for x in observed)
    if clause == '6.2':
        return not any(x['seconds_after_readiness'] is not None and 0 <= x['seconds_after_readiness'] < 120 for x in enabled['entries'])
    if clause == '6.3':
        return all(x['host'] in ('forms.test', 'frame.test', AD) for x in entries) and all(x['fixture']['frames'] == 1 and x['fixture']['url'] == 'http://forms.test/' for x in observed) and {'forms.test','frame.test'} <= {x['host'] for x in entries}
    if clause == '6.4':
        return adblock(observed)
    raise ValueError(clause)


def endpoints(listing, host_url, browser_upgrade, target_upgrade, target_host):
    return bool(listing and browser_upgrade and target_upgrade and target_host and host_url.startswith('ws://localhost:9221/devtools/browser/'))


def version(actual):
    return all(actual.get(k) == v for k, v in VERSION.items())


def webmcp(value):
    return value == 'undefined'


def robust(outgoing, incoming, before_targets, after_targets, before_pages, after_pages):
    return outgoing == 33554432 and incoming == 33554432 and before_targets == after_targets and before_pages == after_pages


def hydrogen_revision(archive_name):
    """Hydrogen package revisions are below 100 and Manganese's start at 100 (contract 1.1)."""
    match = re.fullmatch(r'chromium-[0-9.]+-r([0-9]+)-linux-(?:aarch64|x86_64)-musl\.tar\.xz', archive_name)
    if match is None:
        raise ValueError(f'not an appliance archive name: {archive_name}')
    return int(match.group(1)) < 100


def protocol(hard_ok, browser_answers, oracle):
    return hard_ok and (browser_answers or oracle)


def native(observed, crash, inventory):
    covered = set(observed['methods']) | set(observed['events']) | {'Browser.close', 'Browser.crash', 'Target.targetCrashed'}
    missing = sorted(inventory - covered)
    ok = not missing and observed['stable'] and observed['screenshot_bytes'] > 0 and crash['target'] == crash['crashed_target'] and crash['browser_exit'] != 0 and crash['shutdown_ok']
    return ok, missing


def audio(pages, measurements, events):
    return (len(pages) == 3 and len(measurements) == 13 and all(info and info['state'] == 'running' and info['node'] and info['sampleRate'] == 48000 for info in pages)
            and not events and all(row['listed'] and len(row['contexts']) == 3 and all(x['state'] == 'running' and x['node'] for x in row['contexts']) for row in measurements)
            and all(measurements[-1]['contexts'][i]['time'] - measurements[0]['contexts'][i]['time'] >= 20 for i in range(3))
            and all(measurements[j]['contexts'][i]['time'] > measurements[j-1]['contexts'][i]['time'] for i in range(3) for j in range(1, len(measurements))))


def storage(value):
    return value == {'indexeddb':'persisted', 'service_worker':True, 'cache':'persisted'}


def profile_modes(observed):
    cookie = {'name':'contract_profile', 'value':'persisted', 'httpOnly':True}
    return (len(observed) == 3 and [item['mode'] for item in observed] == ['headed', 'headless', 'headed']
            and all(item['storage'] == 'persisted' and cookie in item['cookies'] for item in observed)
            and observed[0].get('download') == 'headed download evidence')


def client_workloads(observed):
    if set(observed) != CLIENTS:
        return False
    for workloads in observed.values():
        if set(workloads) != WORKLOADS:
            return False
        for item in workloads.values():
            if item.get('status') == 'ok' and item.get('evidence'):
                continue
            if item.get('status') == 'n/a' and item.get('reason'):
                continue
            return False
    return True


def extension_features(observed):
    return set(observed) == EXTENSION_FEATURES and all(item.get('ok') is True and item.get('evidence') for item in observed.values())


def durability(cases):
    disk = cases['full_disk']
    return storage(cases['restart']) and storage(cases['interrupted']) and (disk.get('preserved') is True or disk.get('explicit_failure') is True)
