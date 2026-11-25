import argparse
import json
import os
import platform
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import launch
import durability
import checks
import profile

HERE = Path(__file__).resolve().parent
ALL = ('5.1', '5.2', '5.4', '6.1', '6.2', '6.3', '6.4', '7.1', '7.2', '7.3', '7.4', '7.5', '7.6', '7.7', '8.1', '8.2', '8.3', '8.4')
PENDING = {'7.5', '7.6'}
EXCLUDED = {'8.2', '8.3'}


def preferences(profile, enabled):
    target = profile / 'Default' / 'Preferences'
    target.parent.mkdir(parents=True, exist_ok=True)
    data = json.loads(target.read_text()) if target.exists() else {}
    data.setdefault('adblock', {})['enabled'] = enabled
    temporary = target.with_name('Preferences.new')
    temporary.write_text(json.dumps(data))
    os.replace(temporary, target)
    return data


def egress(run_dir, archive):
    observations = []
    for enabled in (True, False):
        directory = run_dir / ('enabled' if enabled else 'disabled')
        directory.mkdir()
        session = launch.open_session(directory, archive)
        try:
            target = session.profile / 'Default' / 'Preferences'
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(json.dumps({'contract': {'sentinel': 'kept'}, 'adblock': {'other': 'kept', 'enabled': not enabled}}))
            written = preferences(session.profile, enabled)
            session.sidecar(['python3', '-B', '/launch/contract/fixture.py'], {directory: '/evidence'}, detach=True)
            _, browser = session.start_browser('headless')
            probe = session.probe()
            if not probe['readiness']['ok']:
                raise RuntimeError(f'readiness failed: {probe}')
            (directory / 'readiness').write_text(str(time.monotonic()))
            if enabled:
                time.sleep(120)
            fixture = json.loads(session.sidecar(['python3', '-B', '/launch/contract/egress.py']))
            time.sleep(1)
            log = directory / 'connects.jsonl'
            entries = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
            route = session.root('cat /proc/net/route')
            netdev = session.root('cat /proc/net/dev')
            shutdown = session.probe('--close')
            browser.wait(timeout=15)
            local_state = session.profile / 'Local State'
            observations.append({'enabled': enabled, 'entries': entries, 'fixture': fixture, 'route': route, 'netdev': netdev,
                                 'preferences': written, 'shutdown': shutdown, 'local_state': {'exists': local_state.exists(),
                                 'uid': local_state.stat().st_uid if local_state.exists() else None}})
        finally:
            session.close()
    return observations


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--archive', default='hydrogen-r2-amd64')
    parser.add_argument('--clauses', default='')
    args = parser.parse_args()
    selected = set(args.clauses.replace(',', ' ').split()) or set(ALL)
    unknown = selected - set(ALL)
    if unknown:
        parser.error(f'unknown clauses: {sorted(unknown)}')
    run_dir = launch.new_run_dir('contract', platform.machine())
    results = {clause: {'status': 'pending' if clause in PENDING else 'excluded' if clause in EXCLUDED else 'FAIL',
                        'ok': None if clause in PENDING | EXCLUDED else False} for clause in ALL if clause in selected}
    report = {'run_dir': str(run_dir), 'archive': args.archive, 'clauses': results}
    try:
        path = Path(args.archive)
        archive = launch.resolve_archive(run_dir, platform.machine(), path if path.exists() else None,
                                         None if path.exists() else args.archive)
        if '5.4' in selected:
            results['5.4'] = profile.run(run_dir, archive, preferences)
        if selected & {'5.1', '5.2', '6.1', '6.2', '6.3', '6.4'}:
            observed = egress(run_dir, archive)
            for clause in ('5.1','5.2','6.1','6.2','6.3','6.4'):
                if clause in results:
                    results[clause] = {'ok': checks.egress(clause, observed), 'evidence': observed}
        if '8.4' in selected:
            results['8.4'] = durability.run(run_dir, archive, preferences)
        if selected & {'7.1', '7.2', '7.3', '7.4', '7.7', '8.1'}:
            directory = run_dir / 'cdp'
            directory.mkdir()
            session = launch.open_session(directory, archive)
            try:
                preferences(session.profile, True)
                _, browser = session.start_browser('headless')
                readiness = session.probe()
                if not readiness['readiness']['ok']:
                    raise RuntimeError(f'readiness failed: {readiness}')
                if selected & {'7.1', '7.2', '7.7'}:
                    cdp_results = json.loads(session.sidecar(['python3', '-B', '/launch/contract/cdp_checks.py']))
                    for clause, value in cdp_results.items():
                        if clause in results:
                            results[clause] = value
                if '7.3' in selected:
                    observed = json.loads(session.sidecar(['python3', '-B', '/launch/contract/robustness.py'], timeout=300))
                    results['7.3'] = {'ok': False, 'robustness': observed}
                if '8.1' in selected:
                    results['8.1'] = json.loads(session.sidecar(['python3', '-B', '/launch/contract/audio.py'], timeout=150))
                if '7.4' in selected:
                    observed = json.loads(session.sidecar(['python3', '-B', '/launch/contract/native.py'], timeout=100))
                    results['7.4'] = {'ok': False, 'observed': observed}
                shutdown = session.probe('--close')
                shutdown_ok = shutdown['shutdown']['ok']
                browser.wait(timeout=15)
            finally:
                session.close()
        if '7.4' in selected:
            directory = run_dir / 'crash'
            directory.mkdir()
            session = launch.open_session(directory, archive)
            try:
                preferences(session.profile, True)
                _, browser = session.start_browser('headless')
                session.probe()
                crash = json.loads(session.sidecar(['python3', '-B', '/launch/contract/crash.py'], timeout=60))
                crash['browser_exit'] = browser.wait(timeout=20)
                crash['shutdown_ok'] = shutdown_ok
                observed = results['7.4']['observed']
                inventory = set((HERE.parent.parent / 'workloads/cdp/appliance-native.txt').read_text().splitlines())
                ok, missing = checks.native(observed, crash, inventory)
                results['7.4'] = {'ok': ok,
                                  'observed': observed, 'crash': crash, 'missing': missing}
            finally:
                session.close()
        if selected & {'7.1', '7.3'}:
            directory = run_dir / 'protocol'
            directory.mkdir()
            session = launch.open_session(directory, archive)
            try:
                preferences(session.profile, True)
                _, browser = session.start_browser('headless')
                session.probe()
                protocol = json.loads(session.sidecar(['python3', '-B', '/launch/contract/protocol.py'], timeout=180))
                if not protocol['browser_answers']:
                    try:
                        protocol['exit_status'] = browser.wait(timeout=15)
                    except Exception:
                        protocol['exit_status'] = None
                else:
                    session.probe('--close')
                    protocol['exit_status'] = browser.wait(timeout=15)
                protocol['browser_log_tail'] = (directory / 'browser.log').read_text(errors='replace').splitlines()[-40:]
                oracle = checks.hydrogen_revision(archive.path.name)
                defect = not protocol['browser_answers'] and oracle
                if '7.1' in results:
                    results['7.1']['protocol'] = protocol
                    if defect:
                        results['7.1']['oracle_defect'] = 'json-protocol-crash'
                if '7.3' in results:
                    hard_ok = results['7.3']['robustness']['checks_ok']
                    results['7.3'].update({'ok': checks.protocol(hard_ok, protocol['browser_answers'], oracle), 'protocol':protocol})
                    if defect and hard_ok:
                        results['7.3']['status'] = 'expected-defect'
            finally:
                session.close()
    except Exception as error:
        report['error'] = repr(error)
    report['ok'] = all(value['ok'] is not False for value in results.values()) and 'error' not in report
    (run_dir / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    for clause, value in results.items():
        status = value.get('status') or ('ok  ' if value['ok'] else 'FAIL')
        if status == 'expected-defect':
            status = 'defect'
        print(f'{status} {clause}')
    if 'error' in report:
        print(report['error'], file=sys.stderr)
    print(f'report {run_dir / "report.json"}')
    return 0 if report['ok'] else 1

if __name__ == '__main__':
    raise SystemExit(main())
