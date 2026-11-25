import hashlib
import json
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'oracle/launch'))
import launch
from normalize import wire

DIRECTORY = Path(__file__).resolve().parent


def capture(mode, iteration):
    run_dir = launch.new_run_dir('persona', mode + '-' + str(iteration))
    archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
    session = launch.open_session(run_dir, archive)
    browser = None
    try:
        _, browser = session.start_browser(mode)
        witness = session.probe()
        if not witness.get('sandbox', {}).get('ok'):
            raise RuntimeError('sandbox witness failed: ' + json.dumps(witness))
        result = json.loads(session.sidecar(['python3', '-B', '/persona/sidecar.py'], {DIRECTORY:'/persona:ro'}, timeout=180))
        session.sidecar(['python3', '-B', '/persona/fixture.py'], {DIRECTORY:'/persona:ro', run_dir:'/evidence'}, detach=True)
        for _ in range(100):
            if (run_dir / 'ready').exists():
                break
            time.sleep(0.1)
        else:
            raise RuntimeError('wire fixture not ready')
        session.sidecar(['python3', '-B', '/persona/wire_sidecar.py'], {DIRECTORY:'/persona:ro'}, timeout=90)
        result['wire'] = wire([json.loads(line) for line in (run_dir / 'wire.jsonl').read_text().splitlines()])
        shutdown = session.probe('--close')
        status = browser.wait(timeout=15)
        log = (run_dir / 'browser.log').read_text(errors='replace')
        wrapper_pipe_after_clean_child = (mode == 'headed' and status == 1
            and 'TINYLAND_CHILD_EXIT status=exit status: 0' in log
            and 'kind: BrokenPipe' in log)
        if not shutdown.get('shutdown', {}).get('ok') or (status != 0 and not wrapper_pipe_after_clean_child):
            raise RuntimeError('browser shutdown failed: ' + json.dumps(shutdown.get('shutdown')) + ', wrapper status ' + str(status))
        return result
    finally:
        if browser is not None and browser.poll() is None:
            browser.kill()
            browser.wait()
        session.close()


def main():
    lock = ROOT / 'oracle/frozen/artifacts.lock'
    row = next(line.split() for line in lock.read_text().splitlines() if line.endswith('manganese/oracle-inputs:hydrogen-r2-amd64') and '.tar.xz ' in line)
    identity = {'oci_tag':row[3], 'sha256':row[0]}
    captures = {}
    for mode in ('headless', 'headed'):
        a = capture(mode, 1)
        b = capture(mode, 2)
        for key in a:
            if a[key] != b[key]:
                print(json.dumps({'mode':mode, 'surface':key, 'first':a[key], 'second':b[key]}, sort_keys=True), file=sys.stderr)
                raise RuntimeError('exact surface differs')
        captures[mode] = a
        print(mode + ' matched across fresh profiles', flush=True)
    output = ROOT / 'oracle/frozen/persona'
    files = {}
    for mode, areas in captures.items():
        inventory = ROOT / ('oracle/frozen/capability/' + ('headed-' if mode == 'headed' else '') + 'inventory.json')
        areas['global-inventory'] = {'path':inventory.relative_to(ROOT).as_posix(), 'sha256':hashlib.sha256(inventory.read_bytes()).hexdigest()}
        gl = ROOT / 'oracle/frozen/capability/headed-webgl.json'
        areas['webgl']['capabilityReference'] = {'path':gl.relative_to(ROOT).as_posix(), 'sha256':hashlib.sha256(gl.read_bytes()).hexdigest()}
        if areas['webgl']['available'] != (mode == 'headed'):
            raise RuntimeError('unexpected WebGL availability: ' + mode)
        for name, value in areas.items():
            content = {'archive':identity,'mode':mode,'comparator':{'class':'exact','surface':name},'result':value}
            if name == 'identity':
                content['varying'] = {'webSocketDebuggerUrl':{'class':'tolerance','bound':'UUID-shaped browser identifier','relationship':'host and port match the DevTools endpoint'}}
            if name == 'wire':
                content['varying'] = {'tls':value['tls']['varying'],'http':{'class':'tolerance','bound':'WebSocket key decodes to exactly 16 bytes; fixture cookie and referer match their declared structure'}}
            files[output / (mode + '-' + name + '.json')] = (json.dumps(content, sort_keys=True, indent=2, ensure_ascii=True) + '\n').encode()
    manifest = ROOT / 'oracle/frozen/SHA256SUMS'
    lines = manifest.read_text().splitlines()
    for path, content in sorted(files.items()):
        if len(content) >= 1024 * 1024:
            raise RuntimeError('persona file exceeds 1 MiB: ' + str(path))
        if path.exists() and path.read_bytes() != content:
            raise RuntimeError('frozen persona differs: ' + str(path))
        relative = path.relative_to(ROOT / 'oracle/frozen').as_posix()
        line = hashlib.sha256(content).hexdigest() + '  ./' + relative
        prior = [item for item in lines if item.endswith('  ./' + relative)]
        if prior and prior != [line]:
            raise RuntimeError('persona manifest differs: ' + relative)
        if not prior:
            lines.append(line)
    existing = set(output.glob('*.json')) if output.exists() else set()
    if existing - files.keys():
        raise RuntimeError('unexpected frozen persona files: ' + repr(existing - files.keys()))
    output.mkdir(parents=True, exist_ok=True)
    for path, content in files.items():
        path.write_bytes(content)
    manifest.write_text('\n'.join(lines) + '\n')
    print('recorded ' + str(len(files)) + ' persona files')


if __name__ == '__main__':
    main()
