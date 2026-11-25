from pathlib import Path

import launch


def start(session, directory, endpoint, label):
    if endpoint == 'http://127.0.0.1:9221':
        return
    if endpoint != 'http://127.0.0.1:9229':
        raise ValueError(endpoint)
    proxy = launch.ROOT / 'oracle/probes/cdp'
    session.sidecar(['python3', '-B', '/trace/proxy.py', label, '/evidence/cdp.jsonl'],
                    {proxy:'/trace:ro', Path(directory):'/evidence'}, detach=True)


def command(endpoint, args):
    return ['env', f'CDP_ENDPOINT={endpoint}', *args]
