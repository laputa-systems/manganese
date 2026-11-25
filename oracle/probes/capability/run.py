import argparse
import hashlib
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'oracle/launch'))
import launch

DIRECTORY = Path(__file__).resolve().parent
OUTPUT = ROOT / 'oracle/frozen/capability'
LOCK = ROOT / 'oracle/frozen/artifacts.lock'


def capture(mode):
    run_dir = launch.new_run_dir('capability', mode)
    archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
    session = launch.open_session(run_dir, archive)
    browser = None
    try:
        _, browser = session.start_browser(mode)
        witness = session.probe()
        if not witness.get('sandbox', {}).get('ok'):
            raise RuntimeError('sandbox witness failed: ' + json.dumps(witness))
        result = json.loads(session.sidecar(['python3', '-B', '/capability/sidecar.py', str(witness['browser']['pid'])], {DIRECTORY: '/capability:ro'}, timeout=120))
        helper = session.root('stat -c \"%a %u %g\" /opt/browser/chrome_sandbox').strip().split()
        result['sandbox_helper'] = {'mode':helper[0], 'uid':helper[1], 'gid':helper[2]}
        result['sandbox'] = {k: witness['sandbox'][k] for k in ('ok', 'no_new_privs', 'seccomp')}
        result['sandbox']['nested_pid_namespace'] = len(witness['sandbox']['nspid']) > 1
        shutdown = session.probe('--close')
        if not shutdown.get('shutdown', {}).get('ok') or browser.wait(timeout=15) != 0:
            raise RuntimeError('browser shutdown failed')
        return result
    finally:
        if browser is not None and browser.poll() is None:
            browser.kill()
            browser.wait()
        session.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--mode', choices=('headless', 'headed'), default='headless')
    mode = parser.parse_args().mode
    row = next(line.split() for line in LOCK.read_text().splitlines() if line.endswith('manganese/oracle-inputs:hydrogen-r2-amd64') and '.tar.xz ' in line)
    identity = {'oci_tag':row[3], 'sha256':row[0]}
    headless = capture(mode)
    if headless['inventory']['modelContext'] != 'undefined':
        raise RuntimeError('document.modelContext unexpectedly exists')
    cuts = {
        'SUID sandbox helper': {'status':'needs-environment', 'evidence':'sandbox.json records the native container run, renderer namespace witness, NoNewPrivs and Seccomp, process scan and helper file mode; appliance-kernel fallback remains untested'},
        'Privacy Sandbox implementations': {'status':'compatible', 'evidence':'privacy.json records exposed interfaces and rejection behavior, which must be preserved'},
        'Proxy and auth machinery beyond the appliance': {'status':'needs-environment', 'evidence':'requires proxy and authentication server interactions'},
        'Revocation and CT enforcement': {'status':'needs-environment', 'evidence':'requires certificate and stale-list network tests'},
        'Desktop product state': {'status':'needs-environment', 'evidence':'inventory.json records exposed interfaces; profile and visited-link behavior require workloads'},
        'Tab discarding and Memory Saver': {'status':'needs-environment', 'evidence':'requires parked-page lifecycle testing'},
        'General Wayland compatibility': {'status':'needs-headed', 'evidence':'headed launch pending; Tinyland protocol evidence required'},
        'XSLT': {'status':'not-compatible' if headless['xslt'].get('transform', {}).get('result') else 'compatible', 'evidence':'xslt.json'},
        'Speculative loading': {'status':'not-compatible' if headless['inventory']['speculationRules'] or headless['inventory']['prerendering'] != 'undefined' else 'compatible', 'evidence':'inventory.json records speculationRules and prerendering'},
        'Media encoders and capture': {'status':'not-compatible', 'evidence':'codecs.json records MediaRecorder and WebCodecs encoder support; webrtc.json records capture'},
    }
    OUTPUT.mkdir(parents=True, exist_ok=True)
    areas = {k:v for k,v in headless.items() if k not in ('sandbox','processes','sandbox_helper')}
    areas['sandbox'] = {'witness':headless['sandbox'], 'processes':headless['processes'], 'helper_file':headless['sandbox_helper']}
    areas['cuts'] = cuts
    for name, values in areas.items():
        content = {'archive':identity, 'mode':mode, 'result':values, 'headed': 'pending: headed launch' if mode == 'headless' else 'captured'}
        (OUTPUT / (('headed-' if mode == 'headed' else '') + name + '.json')).write_text(json.dumps(content, sort_keys=True, indent=2, ensure_ascii=True) + '\n')
    manifest = ROOT / 'oracle/frozen/SHA256SUMS'
    current = manifest.read_text().splitlines()
    existing = {line.split(maxsplit=1)[1].removeprefix('./') for line in current}
    for path in sorted(OUTPUT.glob('*.json')):
        relative = path.relative_to(ROOT / 'oracle/frozen').as_posix()
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        line = digest + '  ./' + relative
        if relative in existing:
            if line not in current:
                raise RuntimeError('frozen capability changed: ' + relative)
        else:
            current.append(line)
    manifest.write_text('\n'.join(current) + '\n')
    print('recorded ' + str(len(areas)) + ' capability files')


if __name__ == '__main__':
    main()
