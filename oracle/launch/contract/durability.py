import json
import shutil
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import launch
import checks


def start(session):
    _, browser = session.start_browser('headless')
    probe = session.probe()
    if not probe['readiness']['ok']:
        raise RuntimeError(f'browser did not become ready: {probe}')
    return browser


def storage(session, mode):
    return json.loads(session.sidecar(['python3', '-B', '/launch/contract/storage.py', mode], timeout=70))['result']


def close(session, browser):
    result = session.probe('--close')
    code = browser.wait(timeout=15)
    if not result['shutdown']['ok'] or code != 0:
        log = (session.run_dir / 'browser.log').read_text(errors='replace').splitlines()[-25:]
        raise RuntimeError(f"Browser.close failed: shutdown {result.get('shutdown')}, exit {code}, log {log}")


def expected(value):
    return checks.storage(value)


def run(run_dir, archive, preferences):
    directory = run_dir / 'durability'
    directory.mkdir()
    session = launch.open_session(directory, archive)
    cases = {}
    try:
        preferences(session.profile, True)
        session.sidecar(['python3', '-B', '/launch/contract/fixture.py'], {directory:'/evidence'}, detach=True)
        browser = start(session)
        cases['seed'] = storage(session, 'seed')
        close(session, browser)
        browser = start(session)
        cases['restart'] = storage(session, 'read')
        cases['write'] = storage(session, 'interrupt')
        session.root("for p in /proc/[0-9]*; do a=$(tr '\\0' ' ' < \"$p/cmdline\" 2>/dev/null) || continue; case \"$a\" in /opt/browser/chrome*--type=*) ;; /opt/browser/chrome*) kill -KILL \"${p##*/}\"; break;; esac; done")
        try:
            cases['kill_exit'] = browser.wait(timeout=15)
        except Exception:
            raise RuntimeError('browser did not exit after SIGKILL')
        browser = start(session)
        cases['interrupted'] = storage(session, 'read')
        close(session, browser)
        saved = directory / 'saved-profile'
        shutil.copytree(session.profile, saved, symlinks=True)
    finally:
        session.close()
    full_dir = run_dir / 'full-profile'
    full_dir.mkdir()
    full = launch.open_session(full_dir, archive)
    try:
        launch.checked(['docker','exec','--privileged',full.name,'sh','-c',
                        'mount -t tmpfs -o size=128m,mode=0755 tmpfs /profile && chown 1000:27 /profile'])
        launch.checked(['docker','cp',str(saved) + '/.',full.name + ':/profile/'])
        full.root("chown -R 1000:27 /profile; dd if=/dev/zero of=/profile/.filler bs=1M count=256 2>/dev/null || true")
        cases['disk'] = full.root('df -k /profile')
        full.sidecar(['python3','-B','/launch/contract/fixture.py'], {full_dir:'/evidence'}, detach=True)
        _, browser = full.start_browser('headless')
        for _ in range(150):
            if browser.poll() is not None:
                break
            try:
                result = storage(full, 'read')
                cases['full_disk'] = {'state':result, 'preserved':expected(result)}
                break
            except Exception as error:
                cases['full_disk_error'] = repr(error)
                log = (full_dir/'browser.log').read_text(errors='replace').splitlines()
                if 'DOMException' in repr(error) and any('disk is full' in line for line in log):
                    cases['full_disk'] = {'explicit_failure':True, 'error':repr(error), 'log':log[-40:]}
                    break
                time.sleep(0.1)
        else:
            raise RuntimeError('full disk browser neither produced state nor exited')
        if browser.poll() is not None:
            cases['full_disk'] = {'exit':browser.returncode, 'explicit_failure':browser.returncode != 0,
                                  'log':(full_dir/'browser.log').read_text(errors='replace').splitlines()[-40:]}
        else:
            close(full, browser)
    finally:
        full.close()
    cases['ok'] = checks.durability(cases)
    return cases
