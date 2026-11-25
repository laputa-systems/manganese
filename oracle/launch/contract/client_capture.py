import time
from collections.abc import Callable
from pathlib import Path

import checks
import client_sdk
import durability
import launch


REASONS = {
    'discovery':'The capture CLI does not expose the browser version or target list.',
    'forms':'The capture CLI does not expose form input or submission.',
    'accessibility':'The capture CLI does not expose an accessibility tree or snapshot.',
    'screenshot':'The capture CLI saves page content, not screenshots.',
    'dialogs':'The capture CLI does not expose dialog handling.',
    'popups':'The capture CLI does not expose popup actions or target lists.',
    'frames':'The capture CLI does not expose frame interaction or target lists.',
    'page-js':'The capture CLI does not expose page JavaScript evaluation.',
    'files':'The capture CLI does not expose file input or blob download actions.',
    'profiles':'The capture CLI does not expose cookie reads across browser sessions.',
}


def sockets(session: launch.Session) -> list[str]:
    return session.root("awk '$4 == \"01\" && ($2 ~ /:2405$/ || $3 ~ /:2405$/) { print $2, $3 }' /proc/net/tcp").splitlines()


def run(directory: Path, browser_root: Path, image: str, preferences: Callable[[Path, bool], object],
        client: str) -> dict[str, dict[str, object]]:
    if client not in ('extension-cli', 'shadowdriver-crawler'):
        raise ValueError(client)
    directory.mkdir(parents=True)
    session = launch.Session(directory, browser_root, f'manganese-{client}-{directory.parent.parent.name}')
    try:
        preferences(session.profile, True)
        session.sidecar(['python3', '-B', '/launch/contract/fixture.py'], {directory:'/evidence'}, detach=True)
        browser = durability.start(session)
        before = sockets(session)
        outputs = []
        for cycle in range(10):
            output = f'/evidence/capture-{cycle}'
            if client == 'extension-cli':
                command = ['node', '/src/onefile-cli/src/cli.ts', 'save', 'http://forms.test/workload',
                           '--backend', 'helium', '--cdp-url', 'http://127.0.0.1:9221', '--out', output]
            else:
                command = ['env', 'ALL_PROXY=socks5h://127.0.0.1:1080', 'grab', '--control-url',
                           'http://127.0.0.1:9221', '--single', '--no-sitemap', '--no-ublock', '--no-stealth',
                           '--out', output, 'http://forms.test/workload']
            outputs.append(session.sidecar(command, {directory:'/evidence'}, image=image, timeout=150))
            files = list((directory / f'capture-{cycle}').rglob('*.html'))
            if not files or not any(b'Client workload' in path.read_bytes() for path in files):
                raise RuntimeError(f'{client} capture did not contain the fixture: {files}')
        for _ in range(50):
            after = sockets(session)
            if not after:
                break
            time.sleep(0.1)
        durability.close(session, browser)
    finally:
        session.close()
    result = {name:{'status':'n/a', 'reason':reason} for name, reason in REASONS.items()}
    result['sessions'] = client_sdk.outcome(not before and not after, {'cycles':10, 'before':before, 'after':after, 'outputs':outputs})
    if set(result) != checks.WORKLOADS:
        raise RuntimeError('capture workload inventory mismatch')
    return result
