import argparse
import concurrent.futures
import fnmatch
import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
FROZEN = ROOT / 'oracle/frozen'
HERE = Path(__file__).resolve().parent


def lines(path):
    return {line for line in path.read_text().splitlines() if line and not line.startswith('#')} if path.is_file() else set()


def set_differences(name, expected, actual, tolerated, result):
    """Items present on one side only, except items a side recorded as not seen in every run."""
    for item in sorted(expected - actual - tolerated):
        result.append({'field':name, 'expected':item, 'actual':'<missing>'})
    for item in sorted(actual - expected - tolerated):
        result.append({'field':name, 'expected':'<missing>', 'actual':item})


def key(line):
    return '\t'.join(line.split('\t')[:2])


def compare_abi(rebuild, run_dir, result):
    output = run_dir / 'abi'
    completed = run_python(['oracle/probes/abi/census.py', '--archive', str(rebuild), '--output', str(output)], run_dir / 'abi.log')
    if completed.returncode:
        raise RuntimeError('abi census failed: ' + str(completed.returncode))
    frozen, rebuilt = lines(FROZEN / 'abi/x86_64.txt'), lines(output / 'x86_64.txt')
    tolerated = {key(line) for line in lines(FROZEN / 'abi/x86_64-unstable.txt') | lines(output / 'x86_64-unstable.txt')}
    set_differences('abi', {key(line) for line in frozen}, {key(line) for line in rebuilt}, tolerated, result)


def shape_items(path):
    data = read(path)
    return {(kind, name, section, field) for kind in ('methods', 'events') for name, sections in data[kind].items()
            for section, fields in sections.items() for field in fields}


def compare_cdp(rebuild, run_dir, result):
    output = run_dir / 'cdp'
    completed = run_python(['oracle/probes/cdp/run.py', '--archive', str(rebuild), '--output', str(output)], run_dir / 'cdp.log')
    if completed.returncode:
        raise RuntimeError('cdp trace failed: ' + str(completed.returncode))
    unstable = {tuple(line.split('\t')) for line in lines(FROZEN / 'cdp/unstable.tsv') | lines(output / 'unstable.tsv')}
    names = {item[1] for item in unstable if item[0] in ('method', 'event')}
    for name in ('methods.tsv', 'events.tsv', 'errors.tsv'):
        expected, actual = lines(FROZEN / 'cdp' / name), lines(output / name)
        tolerated = {line for line in expected | actual if line.split('\t')[0] in names}
        set_differences('cdp.' + name, expected, actual, tolerated, result)
    shapes_tolerated = {repr(item[1:]) for item in unstable if item[0] == 'shape'}
    expected = {repr(item[1:]) for item in shape_items(FROZEN / 'cdp/shapes.json')}
    actual = {repr(item[1:]) for item in shape_items(output / 'shapes.json')}
    set_differences('cdp.shapes.json', expected, actual, shapes_tolerated, result)


INVENTORY_COMPARERS = [('abi-census', compare_abi), ('cdp-trace', compare_cdp)]


def read(path):
    return json.loads(path.read_text())


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def differences(expected, actual, path, result):
    if isinstance(expected, dict) and isinstance(actual, dict):
        for key in sorted(expected.keys() | actual.keys()):
            name = path + '.' + key
            if key not in expected or key not in actual:
                result.append({'field':name, 'expected':expected.get(key, '<missing>'), 'actual':actual.get(key, '<missing>')})
            else:
                differences(expected[key], actual[key], name, result)
    elif isinstance(expected, list) and isinstance(actual, list):
        if len(expected) != len(actual):
            result.append({'field':path + '.length', 'expected':len(expected), 'actual':len(actual)})
        for index, (a, b) in enumerate(zip(expected, actual)):
            differences(a, b, path + '.' + str(index), result)
    elif type(expected) != type(actual) or expected != actual:
        result.append({'field':path, 'expected':expected, 'actual':actual})


def run_python(arguments, log):
    completed = subprocess.run(['python3', '-B', *arguments], cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    log.write_text(completed.stdout)
    print(completed.stdout, end='', flush=True)
    return completed


def run_make(target, variables, log):
    command = ['make', target] + [key + '=' + value for key, value in variables.items()]
    completed = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    log.write_text(completed.stdout)
    print(completed.stdout, end='', flush=True)
    return completed


def contract(archive, log, cache=None):
    if cache and cache.is_file():
        return read(cache)
    completed = run_make('contract', {'ARCHIVE':str(archive)}, log)
    paths = [line[7:] for line in completed.stdout.splitlines() if line.startswith('report ')]
    if len(paths) != 1:
        raise RuntimeError('contract did not produce exactly one report')
    report = read(Path(paths[0]))
    if 'error' in report:
        raise RuntimeError('contract failed: ' + report['error'])
    if completed.returncode not in (0, 2):
        raise RuntimeError('contract command failed: ' + str(completed.returncode))
    if cache:
        cache.parent.mkdir(parents=True, exist_ok=True)
        temporary = cache.with_name(cache.name + '.' + str(os.getpid()))
        temporary.write_text(json.dumps(report, indent=2) + '\n')
        os.replace(temporary, cache)
    return report


def compare_inventory(name, actual_dir, identity, result):
    expected_dir = FROZEN / name
    expected_names = {p.name for p in expected_dir.glob('*.json')}
    actual_names = {p.name for p in actual_dir.glob('*.json')}
    differences(sorted(expected_names), sorted(actual_names), name + '.files', result)
    for filename in sorted(expected_names & actual_names):
        expected = read(expected_dir / filename)
        actual = read(actual_dir / filename)
        differences(identity, actual.get('archive', {}).get('sha256'), name + '.' + filename + '.archive.sha256', result)
        expected.pop('archive')
        actual.pop('archive', None)
        if name == 'persona':
            comparator = expected.get('comparator', {}).get('class')
            if comparator != 'exact':
                result.append({'field':name + '.' + filename + '.comparator', 'error':'unsupported comparator ' + str(comparator)})
                continue
            for evidence in (expected, actual):
                value = evidence['result']
                reference = value if filename.endswith('-global-inventory.json') else value.get('capabilityReference') if filename.endswith('-webgl.json') else None
                if reference is not None:
                    capability_dir = FROZEN / 'capability' if evidence is expected else actual_dir.parent / 'capability'
                    ref_file = capability_dir / Path(reference['path']).name
                    wanted_path = 'oracle/frozen/capability/' + ref_file.name if evidence is expected else str(ref_file.resolve())
                    differences(wanted_path, reference['path'], name + '.' + filename + '.capabilityReference.path', result)
                    differences(digest(ref_file), reference['sha256'], name + '.' + filename + '.capabilityReference.sha256', result)
                    reference['path'] = '<verified>'
                    reference['sha256'] = '<verified>'
        differences(expected, actual, name + '.' + filename, result)


def strip_ignored(value, patterns, substitutions, path=''):
    if isinstance(value, dict):
        return {key:strip_ignored(item, patterns, substitutions, path + '.' + key if path else key) for key, item in value.items()
                if not any(fnmatch.fnmatchcase(path + '.' + key if path else key, pattern) for pattern in patterns)}
    if isinstance(value, list):
        return [strip_ignored(item, patterns, substitutions, path + '.' + str(index)) for index, item in enumerate(value)]
    if isinstance(value, str):
        for rule in substitutions:
            if fnmatch.fnmatchcase(path, rule['path']):
                value = re.sub(rule['pattern'], rule['replacement'], value)
    return value


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--rebuild', required=True, type=Path)
    parser.add_argument('--inject', type=Path)
    args = parser.parse_args()
    rebuild = args.rebuild.resolve()
    run_dir = ROOT / '.work/tmp/equivalence' / (time.strftime('%Y%m%dT%H%M%SZ', time.gmtime()) + '-' + str(os.getpid()))
    run_dir.mkdir(parents=True)
    report = {'archive':{'path':str(rebuild)}, 'inventories':{name:'pending' for name in ('abi-census', 'cdp-trace', 'capability', 'persona', 'contract', 'test-fast')}, 'discrepancies':[]}
    result = report['discrepancies']
    try:
        identity = digest(rebuild)
        report['archive']['sha256'] = identity
        for name, comparer in INVENTORY_COMPARERS:
            if comparer is not None:
                before = len(result)
                comparer(rebuild, run_dir, result)
                report['inventories'][name] = 'pass' if len(result) == before else 'fail'
        series = rebuild.parent / 'capture-series'
        if series.is_file():
            report['archive']['capture_series_sha256'] = series.read_text().strip()
        for target, name in [('capability-probes', 'capability'), ('persona-inventory', 'persona')]:
            output = run_dir / name
            arguments = '--archive ' + shlex.quote(str(rebuild)) + ' --output ' + shlex.quote(str(output))
            if name == 'persona':
                arguments += ' --capability ' + shlex.quote(str(run_dir / 'capability'))
            completed = run_make(target, {'PROBE_ARGS':arguments}, run_dir / (name + '.log'))
            if completed.returncode:
                raise RuntimeError(target + ' failed: ' + str(completed.returncode))
        if args.inject:
            injected = read(args.inject)
            destination = run_dir / injected['inventory'] / injected['file']
            if injected['inventory'] not in ('capability', 'persona') or destination.parent != run_dir / injected['inventory']:
                raise ValueError('invalid injection destination')
            destination.write_text(json.dumps(injected['content'], indent=2) + '\n')
        for name in ('capability', 'persona'):
            before = len(result)
            compare_inventory(name, run_dir / name, identity, result)
            report['inventories'][name] = 'pass' if len(result) == before else 'fail'
        revision = subprocess.check_output(['git', 'rev-parse', 'HEAD:oracle/launch'], cwd=ROOT, text=True).strip()
        cache = ROOT / '.work/tmp/equivalence-cache' / (revision + '.json')
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
            frozen_run = executor.submit(contract, 'hydrogen-r2-amd64', run_dir / 'contract-frozen.log', cache)
            rebuild_run = executor.submit(contract, rebuild, run_dir / 'contract-rebuild.log')
            baseline = frozen_run.result()
            candidate = rebuild_run.result()
        (run_dir / 'contract-frozen.json').write_text(json.dumps(baseline, indent=2) + '\n')
        (run_dir / 'contract-rebuild.json').write_text(json.dumps(candidate, indent=2) + '\n')
        before = len(result)
        differences(sorted(baseline['clauses']), sorted(candidate['clauses']), 'contract.clauses', result)
        for clause in sorted(baseline['clauses'].keys() | candidate['clauses'].keys()):
            a = baseline['clauses'].get(clause, {})
            b = candidate['clauses'].get(clause, {})
            for key in ('status', 'ok'):
                differences(a.get(key), b.get(key), 'contract.' + clause + '.' + key, result)
            evidence = {'7.1':('custom_target_host',), '7.2':('expected',), '7.4':('missing',), '7.7':('value',), '8.1':('started',)}
            for key in evidence.get(clause, ()):
                differences(a.get(key), b.get(key), 'contract.' + clause + '.' + key, result)
            if clause == '7.2' and 'actual' in a and 'actual' in b:
                differences({k:v for k,v in a['actual'].items() if k != 'webSocketDebuggerUrl'},
                            {k:v for k,v in b['actual'].items() if k != 'webSocketDebuggerUrl'},
                            'contract.7.2.actual', result)
            if clause == '7.4' and 'observed' in a and 'observed' in b:
                for key in ('methods', 'events', 'evaluation', 'stable'):
                    differences(a['observed'].get(key), b['observed'].get(key), 'contract.7.4.observed.' + key, result)
        report['inventories']['contract'] = 'pass' if len(result) == before else 'fail'
        before = len(result)
        ignored = read(HERE / 'test-fast-ignored.json')
        substitutions = read(HERE / 'test-fast-normalize.json')
        differences(strip_ignored(read(FROZEN / 'amd64-r2/test-fast.json'), ignored, substitutions), strip_ignored(read(rebuild.parent / 'test-fast.json'), ignored, substitutions), 'test-fast', result)
        report['inventories']['test-fast'] = 'pass' if len(result) == before else 'fail'
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
        result.append({'field':'harness', 'error':str(error)})
    (run_dir / 'report.json').write_text(json.dumps(report, sort_keys=True, indent=2) + '\n')
    for name, status in report['inventories'].items():
        print(name + ': ' + status)
    print('discrepancies: ' + str(len(result)))
    print('report ' + str(run_dir / 'report.json'))
    return 1 if result else 0


if __name__ == '__main__':
    sys.exit(main())
