import json
import os
import platform
import shutil
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path[:0] = [str(ROOT / 'oracle/launch'), str(ROOT / 'oracle/launch/contract')]
import clients
import contract
import launch
import trace

ENDPOINT = 'http://127.0.0.1:9229'
OUTPUT = ROOT / 'oracle/frozen/cdp'


def paths(value, prefix, output):
    if value is None:
        kind = 'null'
    elif isinstance(value, bool):
        kind = 'boolean'
    elif isinstance(value, (int, float)):
        kind = 'number'
    elif isinstance(value, str):
        kind = 'string'
    elif isinstance(value, list):
        kind = 'array'
    elif isinstance(value, dict):
        kind = 'object'
    else:
        raise TypeError(type(value))
    output[prefix].add(kind)
    if isinstance(value, dict):
        for key, item in value.items():
            paths(item, f'{prefix}.{key}' if prefix else key, output)
    elif isinstance(value, list):
        for item in value:
            paths(item, prefix + '[]', output)


def reduce_logs(directory, identity):
    methods = defaultdict(Counter)
    events = defaultdict(set)
    shapes = defaultdict(lambda: defaultdict(lambda: defaultdict(set)))
    errors = defaultdict(set)
    for file in sorted(directory.rglob('cdp.jsonl')):
        pending = {}
        for line in file.read_text().splitlines():
            entry = json.loads(line)
            if entry['opcode'] != 'message' or 'message' not in entry:
                continue
            msg = entry['message']
            if not isinstance(msg, dict):
                continue
            label = entry['client']
            key = (entry['connection'], msg.get('id'))
            if entry['direction'] == 'sent' and 'method' in msg and 'id' in msg:
                method = msg['method']
                pending[key] = method
                methods[method][label] += 1
                paths(msg.get('params', {}), '', shapes[method]['params'])
            elif entry['direction'] == 'received' and 'id' in msg:
                method = pending.pop(key, None)
                if method is None:
                    raise RuntimeError(f'unmatched response in {file}: {msg}')
                if 'result' in msg:
                    paths(msg['result'], '', shapes[method]['result'])
                if 'error' in msg:
                    error = msg['error']
                    errors[method].add((str(error.get('code', '')), str(error.get('message', ''))))
            elif entry['direction'] == 'received' and 'method' in msg:
                event = msg['method']
                events[event].add(label)
                paths(msg.get('params', {}), '', shapes[event]['event'])
    labels = set().union(*(set(counts) for counts in methods.values()))
    expected = {'browser-use-cli', 'browser-use-library', 'agent-browser', 'extension-cli',
                'shadowdriver-sdk', 'shadowdriver-crawler', 'extension-features', 'native', 'audio'}
    if labels != expected or not events:
        raise RuntimeError(f'CDP capture missing clients: {sorted(expected - labels)}; unexpected: {sorted(labels - expected)}')
    OUTPUT.mkdir(parents=True, exist_ok=True)
    (OUTPUT / 'methods.tsv').write_text('method\tclients\tcount_class\n' + ''.join(
        f"{method}\t{','.join(sorted(counts))}\t{'once' if sum(counts.values()) == 1 else 'multiple'}\n"
        for method, counts in sorted(methods.items())))
    (OUTPUT / 'events.tsv').write_text('event\tclients\n' + ''.join(
        f"{event}\t{','.join(sorted(labels))}\n" for event, labels in sorted(events.items())))
    (OUTPUT / 'errors.tsv').write_text('method\tcode\tmessage\n' + ''.join(
        f'{method}\t{code}\t{message}\n' for method, values in sorted(errors.items()) for code, message in sorted(values)))
    result = {'archive':identity, 'methods':{}, 'events':{}}
    for name, sections in sorted(shapes.items()):
        converted = {section:{path:sorted(types) for path, types in sorted(values.items())}
                     for section, values in sorted(sections.items())}
        target = result['events'] if 'event' in converted else result['methods']
        target[name] = converted
    (OUTPUT / 'shapes.json').write_text(json.dumps(result, indent=2, sort_keys=True) + '\n')
    print(f'methods {len(methods)} events {len(events)} clients {len(set().union(*(set(c) for c in methods.values())))}')


def main():
    if platform.machine() != 'x86_64':
        raise RuntimeError('native x86_64 required')
    directory = launch.new_run_dir('cdp-trace', 'x86_64')
    print(f'run {directory}', flush=True)
    try:
        archive = launch.resolve_archive(directory, 'x86_64', None, 'hydrogen-r2-amd64')
        results = clients.run(directory, archive, contract.preferences, {'7.5', '7.6'}, ENDPOINT)
        if not all(result['ok'] for result in results.values()):
            raise RuntimeError('client contract checks failed')
        for label, script, timeout in [('native', 'native.py', 100), ('audio', 'audio.py', 150)]:
            path = directory / label
            path.mkdir()
            session = launch.open_session(path, archive)
            try:
                trace.start(session, path, ENDPOINT, label)
                contract.preferences(session.profile, True)
                _, browser = session.start_browser('headless')
                if not session.probe()['readiness']['ok']:
                    raise RuntimeError(f'{label}: browser not ready')
                result = json.loads(session.sidecar(trace.command(ENDPOINT, ['python3', '-B', '/launch/contract/' + script]), timeout=timeout))
                if label == 'audio' and not result['ok']:
                    raise RuntimeError('audio failed')
                shutdown = session.probe('--close')
                if not shutdown['shutdown']['ok'] or browser.wait(timeout=15) != 0:
                    raise RuntimeError(f'{label}: shutdown failed')
            finally:
                session.close()
        lock = (ROOT / 'oracle/frozen/artifacts.lock').read_text().splitlines()
        digest = next(line.split()[0] for line in lock if line.split() and line.split()[0] != '#'
                      and len(line.split()) >= 4 and line.split()[3] == 'manganese/oracle-inputs:hydrogen-r2-amd64')
        reduce_logs(directory, {'identity':archive.identity, 'sha256':digest})
    finally:
        launch.checked(['docker', 'run', '--rm', '-v', f'{directory}:/cleanup', launch.PROBE_IMAGE,
                        'chown', '-R', f'{os.getuid()}:{os.getgid()}', '/cleanup'])
        shutil.rmtree(directory)


if __name__ == '__main__':
    main()
