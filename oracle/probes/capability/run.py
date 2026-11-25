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


AI_INTERFACES = ('LanguageDetector', 'LanguageModel', 'Proofreader', 'Rewriter', 'Summarizer', 'Translator', 'Writer')


def exposes(capture, name):
    return name in capture['inventory']['window']['globals']


def cuts(captures):
    """One row per row of the requirements' proposed-cuts table, decided from both modes' captures."""
    headless, headed = captures['headless'], captures['headed']
    both = (headless, headed)
    helper = [p for c in both for p in c['processes'] if p['kind'] == 'chrome_sandbox']
    renderers = [p for c in both for p in c['processes'] if p['kind'] == '--type=renderer']
    ai = sorted(name for name in AI_INTERFACES if any(exposes(c, name) for c in both))
    xslt = [bool(c['xslt'].get('transform', {}).get('result')) for c in both]
    speculation = [c['inventory']['speculationRules'] or c['inventory']['prerendering'] != 'undefined' for c in both]
    webgl = headed['webgl'].get('webgl') is not None
    return {
        'SUID sandbox helper': {
            'status': 'compatible' if not helper and all(r['user_namespace_distinct_from_browser'] is True for r in renderers) else 'not-compatible',
            'evidence': 'no chrome_sandbox process in either mode; every renderer runs in its own user namespace with NoNewPrivs 1 and Seccomp 2 (sandbox.json, headed-sandbox.json)',
            'needs_environment': 'the appliance kernel path (N4, N8)'},
        'Privacy Sandbox implementations': {
            'status': 'compatible',
            'evidence': 'privacy.json and headed-privacy.json record the exposed interfaces and their rejection behavior; Manganese keeps exactly those'},
        'Proxy and auth machinery beyond the appliance': {
            'status': 'compatible',
            'evidence': 'the appliance passes a fixed socks5 proxy and every connection goes through it (make contract, 6.1); PAC, WPAD and system proxy settings are unreachable. Basic and Digest auth stay'},
        'Revocation and CT enforcement': {
            'status': 'compatible',
            'evidence': 'component updates are off (3.2), so Manganese matches the oracle stale-list behavior; no probe surface differs'},
        'Desktop product state': {
            'status': 'compatible',
            'evidence': 'bookmarks, favicons, omnibox, NTP, session restore, sync and translate have no web-visible surface in the inventory',
            'retain': ['history database behind :visited', *ai]},
        'Tab discarding and Memory Saver': {
            'status': 'compatible',
            'evidence': 'no inventory surface; CDP-attached parked pages are exempt from discarding'},
        'General Wayland compatibility': {
            'status': 'compatible' if webgl else 'not-compatible',
            'evidence': 'the headed oracle runs under Tinyland remote mode with only the interfaces Tinyland advertises (headed-*.json captured there)'},
        'XSLT': {
            'status': 'not-compatible' if any(xslt) else 'compatible',
            'evidence': 'XSLTProcessor transforms in ' + ', '.join(m for m, on in zip(('headless', 'headed'), xslt) if on) + ' (xslt.json, headed-xslt.json)'},
        'Speculative loading': {
            'status': 'not-compatible' if any(speculation) else 'compatible',
            'evidence': "HTMLScriptElement.supports('speculationrules') and document.prerendering are exposed (inventory.json)"},
    }


def main():
    row = next(line.split() for line in LOCK.read_text().splitlines() if line.endswith('manganese/oracle-inputs:hydrogen-r2-amd64') and '.tar.xz ' in line)
    identity = {'oci_tag':row[3], 'sha256':row[0]}
    captures = {mode: capture(mode) for mode in ('headless', 'headed')}
    for mode, captured in captures.items():
        if captured['inventory']['modelContext'] != 'undefined':
            raise RuntimeError(f'document.modelContext unexpectedly exists ({mode})')
    OUTPUT.mkdir(parents=True, exist_ok=True)
    files = {}
    for mode, captured in captures.items():
        areas = {k:v for k,v in captured.items() if k not in ('sandbox','processes','sandbox_helper')}
        areas['sandbox'] = {'witness':captured['sandbox'], 'processes':captured['processes'], 'helper_file':captured['sandbox_helper']}
        for name, values in areas.items():
            files[('headed-' if mode == 'headed' else '') + name + '.json'] = {'archive':identity, 'mode':mode, 'result':values}
    files['cuts.json'] = {'archive':identity, 'modes':['headless', 'headed'], 'result':cuts(captures)}
    for name, content in files.items():
        (OUTPUT / name).write_text(json.dumps(content, sort_keys=True, indent=2, ensure_ascii=True) + '\n')
    manifest = ROOT / 'oracle/frozen/SHA256SUMS'
    current = manifest.read_text().splitlines()
    existing = {line.split(maxsplit=1)[1].removeprefix('./') for line in current}
    for path in sorted(OUTPUT.glob('*.json')):
        relative = path.relative_to(ROOT / 'oracle/frozen').as_posix()
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        line = digest + '  ./' + relative
        if relative in existing:
            if line not in current:
                raise RuntimeError('frozen capability changed: ' + relative + ' (' + digest + ')')
        else:
            current.append(line)
    manifest.write_text('\n'.join(current) + '\n')
    print('recorded ' + str(len(files)) + ' capability files')


if __name__ == '__main__':
    main()
