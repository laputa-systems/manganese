import argparse
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


def capture(mode, iteration, archive_path):
    run_dir = launch.new_run_dir('persona', mode + '-' + str(iteration))
    archive = launch.resolve_archive(run_dir, 'x86_64', archive_path, None)
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
        if not shutdown.get('shutdown', {}).get('ok') or status != 0:
            raise RuntimeError('browser shutdown failed: ' + json.dumps(shutdown.get('shutdown')) + ', wrapper status ' + str(status))
        return result
    finally:
        if browser is not None and browser.poll() is None:
            browser.kill()
            browser.wait()
        session.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--archive', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--capability', type=Path, default=ROOT / 'oracle/frozen/capability')
    args = parser.parse_args()
    if args.output is not None and args.output.resolve().is_relative_to((ROOT / 'oracle/frozen').resolve()):
        parser.error('--output must be outside oracle/frozen')
    lock = ROOT / 'oracle/frozen/artifacts.lock'
    row = next(line.split() for line in lock.read_text().splitlines() if line.endswith('manganese/oracle-inputs:hydrogen-r2-amd64') and '.tar.xz ' in line)
    identity = {'oci_tag':row[3], 'sha256':row[0]}
    if args.archive is not None:
        with args.archive.open('rb') as source:
            identity = {'sha256':hashlib.file_digest(source, 'sha256').hexdigest()}
    captures = {}
    for mode in ('headless', 'headed'):
        a = capture(mode, 1, args.archive)
        b = capture(mode, 2, args.archive)
        for key in a:
            if a[key] != b[key]:
                print(json.dumps({'mode':mode, 'surface':key, 'first':a[key], 'second':b[key]}, sort_keys=True), file=sys.stderr)
                raise RuntimeError('exact surface differs')
        captures[mode] = a
        print(mode + ' matched across fresh profiles', flush=True)
    output = args.output or ROOT / 'oracle/frozen/persona'
    files = {}
    for mode, areas in captures.items():
        inventory = args.capability / (('headed-' if mode == 'headed' else '') + 'inventory.json')
        inventory_path = 'oracle/frozen/capability/' + inventory.name if args.output is None else str(inventory.resolve())
        areas['global-inventory'] = {'path':inventory_path, 'sha256':hashlib.sha256(inventory.read_bytes()).hexdigest()}
        gl = args.capability / 'headed-webgl.json'
        gl_path = 'oracle/frozen/capability/' + gl.name if args.output is None else str(gl.resolve())
        areas['webgl']['capabilityReference'] = {'path':gl_path, 'sha256':hashlib.sha256(gl.read_bytes()).hexdigest()}
        if areas['webgl']['available'] != (mode == 'headed'):
            raise RuntimeError('unexpected WebGL availability: ' + mode)
        for name, value in areas.items():
            content = {'archive':identity,'mode':mode,'comparator':{'class':'exact','surface':name},'result':value}
            if name == 'identity':
                content['varying'] = {'webSocketDebuggerUrl':{'class':'tolerance','bound':'UUID-shaped browser identifier','relationship':'host and port match the DevTools endpoint'}}
            if name == 'wire':
                content['varying'] = {'tls':value['tls']['varying'],'http':{'class':'tolerance','bound':'WebSocket key decodes to exactly 16 bytes; fixture cookie and referer match their declared structure'}}
            files[output / (mode + '-' + name + '.json')] = (json.dumps(content, sort_keys=True, indent=2, ensure_ascii=True) + '\n').encode()
    if args.output is not None:
        output.mkdir(parents=True, exist_ok=True)
        for path, content in files.items():
            path.write_bytes(content)
        print('recorded ' + str(len(files)) + ' persona files')
        return
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
