import collections
import hashlib
import shutil
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
FROZEN = ROOT / 'oracle/frozen'
OUT = FROZEN / 'abi/x86_64.txt'
UNSTABLE = FROZEN / 'abi/x86_64-unstable.txt'
WORKLOADS = (('oracle-launch-headless', ['make', 'oracle-launch', 'ARCH=x86_64', 'MODE=headless']),
             ('oracle-launch-headed', ['make', 'oracle-launch', 'ARCH=x86_64', 'MODE=headed']),
             ('contract', ['make', 'contract', 'ARCHIVE=hydrogen-r2-amd64',
                           'CLAUSES=5.1 5.2 5.4 6.1 6.2 6.3 6.4 7.1 7.2 7.3 7.5 7.6 7.7 8.1 8.4']),
             ('contract-7.4', ['make', 'contract', 'ARCHIVE=hydrogen-r2-amd64', 'CLAUSES=7.4']),
             ('capability-probes', ['make', 'capability-probes']),
             ('persona-inventory', ['make', 'persona-inventory']))
SYSCALL = re.compile(r'^([a-zA-Z_][a-zA-Z_0-9]*)\((.*)\)\s+=\s+(.*)$')
CLONE = re.compile(r'(?:clone|clone3|fork|vfork)\(.*\)\s+=\s+(\d+)(?: /\* (\d+) in strace.s PID NS \*/)?')
TYPE = re.compile(r'--type=([a-zA-Z0-9_-]+)')
SUBTYPE = re.compile(r'--utility-sub-type=([a-zA-Z0-9_.-]+)')
PATH = re.compile(r'"(/(?:proc|sys)(?:/[^"\\]*)?)"')
SYMBOL = re.compile(r'\b[A-Z][A-Z0-9_]{1,}\b')
STRINGS = re.compile(r'"(?:\\.|[^"\\])*"')
FLAG_ARGUMENTS = {
    'open': (1,), 'openat': (2,), 'access': (1,), 'faccessat': (2, 3), 'faccessat2': (2, 3),
    'mmap': (2, 3), 'mprotect': (2,), 'pkey_mprotect': (2,), 'mremap': (3,),
    'futex': (1,), 'futex_waitv': (2,), 'socket': (0, 1, 2), 'socketpair': (0, 1, 2),
    'accept4': (3,), 'epoll_create1': (0,), 'epoll_ctl': (1,), 'epoll_pwait2': (5,),
    'eventfd2': (1,), 'timerfd_create': (1,), 'timerfd_settime': (1,), 'pipe2': (1,),
    'dup3': (2,), 'fcntl': (1,), 'memfd_create': (1,), 'prctl': (0,), 'fcntl64': (1,),
    'sendto': (3,), 'recvfrom': (3,), 'sendmsg': (2,), 'recvmsg': (2,),
    'shutdown': (1,), 'renameat2': (4,), 'statx': (2, 3), 'getrandom': (2,),
    'wait4': (2,), 'waitid': (0, 3,), 'poll': (), 'ppoll': (), 'close_range': (2,),
}
PATH_ARGUMENTS = {
    'open': (0,), 'openat': (1,), 'openat2': (1,), 'creat': (0,),
    'stat': (0,), 'lstat': (0,), 'newfstatat': (1,), 'statx': (1,),
    'access': (0,), 'faccessat': (1,), 'faccessat2': (1,),
    'readlink': (0,), 'readlinkat': (1,), 'chdir': (0,), 'chroot': (0,),
    'unlink': (0,), 'unlinkat': (1,), 'mkdir': (0,), 'mkdirat': (1,),
    'rmdir': (0,), 'rename': (0, 1), 'renameat': (1, 3), 'renameat2': (1, 3),
    'link': (0, 1), 'linkat': (1, 3), 'symlink': (1,), 'symlinkat': (2,),
    'chmod': (0,), 'fchmodat': (1,), 'fchmodat2': (1,), 'chown': (0,),
    'lchown': (0,), 'fchownat': (1,), 'execve': (0,), 'execveat': (1,),
    'truncate': (0,), 'utimensat': (1,), 'mknod': (0,), 'mknodat': (1,),
    'statfs': (0,), 'getxattr': (0,), 'lgetxattr': (0,), 'listxattr': (0,),
    'llistxattr': (0,), 'setxattr': (0,), 'lsetxattr': (0,),
    'removexattr': (0,), 'lremovexattr': (0,), 'mount': (0, 1), 'umount2': (0,),
}


def role(text):
    match = TYPE.search(text)
    if match:
        kind = match.group(1)
        sub = SUBTYPE.search(text)
        return ('crashpad' if kind == 'crashpad-handler' else
                kind + (':' + sub.group(1) if kind == 'utility' and sub else ''))
    if 'chrome_crashpad_handler' in text:
        return 'crashpad'
    return None


def trace_roles(files):
    inherited = {}
    known = {}
    for path in files:
        pid = int(path.name.split('.')[-1])
        pending = None
        with path.open(errors='replace') as stream:
            for line in stream:
                if line.startswith('execve('):
                    found = role(line)
                    if found:
                        known[pid] = found
                    elif '"/opt/browser/chrome"' in line:
                        known[pid] = 'browser'
                if line.startswith('recvmsg(') and '--type=' in line:
                    pending = role(line)
                match = CLONE.match(line)
                if match and int(match.group(1)) > 0:
                    child = int(match.group(2) or match.group(1))
                    inherited[child] = (pid, pending)
                    pending = None
    changed = True
    while changed:
        changed = False
        for pid, (parent, hint) in inherited.items():
            if pid not in known and (hint or parent in known):
                known[pid] = hint or known[parent]
                changed = True
    return known


def normalized_path(path):
    path = re.sub(r'\d+', '<number>', path)
    path = re.sub(r'^/proc/<number>', '/proc/<pid>', path)
    path = re.sub(r'(/task/)<number>', r'\1<tid>', path)
    path = re.sub(r'(/(?:fd|fdinfo)/)<number>', r'\1<fd>', path)
    return path


def arguments(text):
    parts = []
    start = 0
    depth = 0
    quoted = False
    escaped = False
    for i, char in enumerate(text):
        if quoted:
            if escaped:
                escaped = False
            elif char == '\\':
                escaped = True
            elif char == '"':
                quoted = False
        elif char == '"':
            quoted = True
        elif char in '([{':
            depth += 1
        elif char in ')]}':
            depth -= 1
        elif char == ',' and depth == 0:
            parts.append(text[start:i].strip())
            start = i + 1
    parts.append(text[start:].strip())
    return parts


def census(root):
    result = collections.defaultdict(set)
    for directory in sorted(p for p in root.iterdir() if p.is_dir()):
        status = directory / 'status'
        if not status.is_file() or status.read_text().strip() != '0':
            raise RuntimeError('tracer did not complete successfully in ' + str(directory))
        files = sorted(path for path in directory.glob('trace.[0-9]*') if path.stat().st_size)
        if not files:
            raise RuntimeError('no traces in ' + str(directory))
        roles = trace_roles(files)
        missing = [p.name for p in files if int(p.name.split('.')[-1]) not in roles]
        if missing:
            raise RuntimeError('unattributed trace files: ' + ', '.join(missing))
        for path in files:
            kind = roles[int(path.name.split('.')[-1])]
            with path.open(errors='replace') as stream:
                for line in stream:
                    match = SYSCALL.match(line)
                    if match:
                        name, args, value = match.groups()
                    elif ' <unfinished ...>' in line and '(' in line:
                        name, args = line.split('(', 1)
                        args = args.split(' <unfinished ...>', 1)[0]
                        value = ''
                    else:
                        continue
                    result[('syscall', name)].add(kind)
                    if ' ENOSYS ' in value:
                        result[('error', name + ':ENOSYS')].add(kind)
                    parts = arguments(args)
                    for index in PATH_ARGUMENTS.get(name, ()):
                        if index >= len(parts):
                            continue
                        item = PATH.fullmatch(parts[index])
                        if item:
                            result[('path', normalized_path(item.group(1)))].add(kind)
                        elif index > 0:
                            directory = re.fullmatch(r'\d+<(/(?:proc|sys)(?:/[^>]*)?)>', parts[index - 1])
                            relative = re.fullmatch(r'"([^"/\\][^"\\]*)"', parts[index])
                            if directory and relative:
                                item = os.path.normpath(directory.group(1) + '/' + relative.group(1))
                                result[('path', normalized_path(item))].add(kind)
                    if name == 'ioctl':
                        parts = arguments(args)
                        if len(parts) > 1:
                            result[('ioctl', parts[1])].add(kind)
                    if name in ('socket', 'socketpair'):
                        parts = arguments(args)
                        if len(parts) >= 3:
                            result[('socket', ','.join(parts[:3]))].add(kind)
                    if name == 'madvise':
                        parts = arguments(args)
                        if len(parts) == 3:
                            result[('madvise', parts[2])].add(kind)
                    flags = []
                    for index, part in enumerate(arguments(args)):
                        clean = re.sub(r'<[^>]*>', '', STRINGS.sub('""', part))
                        symbols = sorted(set(SYMBOL.findall(clean)))
                        if index in FLAG_ARGUMENTS.get(name, ()) and re.fullmatch(r'(?:0x[0-9a-fA-F]+|[0-9]+)', clean):
                            symbols.append(clean)
                        if 'flags=' in clean:
                            symbols.extend(re.findall(r'flags=(0x[0-9a-fA-F]+|[0-9]+)(?=[,}\]]|$)', clean))
                        if symbols:
                            unknown = sorted(set(re.findall(r'(?<=\|)0x[0-9a-fA-F]+', clean)))
                            flags.append(str(index) + '=' + '|'.join(symbols + unknown))
                    if flags:
                        result[('flags', name + ':' + ';'.join(flags))].add(kind)
    return result


def run_contract(command, environment):
    result = subprocess.run(command, cwd=ROOT, env=environment, text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT)
    print(result.stdout, end='', flush=True)
    reports = re.findall(r'^report (.+/report\.json)$', result.stdout, re.MULTILINE)
    if len(reports) != 1:
        raise RuntimeError('contract did not identify exactly one report')
    report = json.loads(Path(reports[0]).read_text())
    if result.returncode == 0 and report.get('ok') is True:
        return
    clauses = report['clauses']
    failures = {name for name, value in clauses.items() if value.get('ok') is False}
    error = report.get('error', '')
    if (result.returncode == 0 or failures != {'7.4'} or
            '/launch/contract/crash.py' not in error or
            'Target.targetCrashed' not in error or
            'ConnectionError: CDP connection closed' not in error):
        raise RuntimeError('traced contract failed outside the expected crash case')
    run_dir = Path(report['run_dir'])
    crash_log = (run_dir / 'crash/browser.log').read_text(errors='replace')
    if 'scoped_ptrace_attach.cc' not in crash_log or 'ptrace: Operation not permitted' not in crash_log:
        raise RuntimeError('crash case lacks the expected ptrace conflict')
    observed = clauses['7.4'].get('observed', {})
    inventory = set((ROOT / 'oracle/workloads/cdp/appliance-native.txt').read_text().splitlines())
    covered = set(observed.get('methods', ())) | set(observed.get('events', ())) | {
        'Browser.close', 'Browser.crash', 'Target.targetCrashed'}
    shutdown = json.loads((run_dir / 'cdp/abi-shutdown.json').read_text())
    if (inventory - covered or observed.get('stable') is not True or
            observed.get('screenshot_bytes', 0) <= 0 or shutdown.get('shutdown', {}).get('ok') is not True):
        raise RuntimeError('traced contract 7.4 has a non-crash failure')
    if any(value.get('ok') is not True and value.get('status') not in ('pending', 'excluded')
           for name, value in clauses.items() if name not in ('7.3', '7.4')):
        raise RuntimeError('traced contract has another failing clause')
    retry = subprocess.run(['make', 'contract', 'ARCHIVE=hydrogen-r2-amd64', 'CLAUSES=7.1 7.3'], cwd=ROOT,
                           env=environment, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    print(retry.stdout, end='', flush=True)
    retry_reports = re.findall(r'^report (.+/report\.json)$', retry.stdout, re.MULTILINE)
    if retry.returncode or len(retry_reports) != 1:
        raise RuntimeError('traced contract could not complete clauses 7.1 and 7.3')
    retry_report = json.loads(Path(retry_reports[0]).read_text())
    if retry_report.get('ok') is not True or any(retry_report['clauses'][key].get('ok') is not True
                                                for key in ('7.1', '7.3')):
        raise RuntimeError('traced contract clauses 7.1 and 7.3 did not pass')
    print('abi-census: accepted traced 7.4 crash ptrace conflict; other assertions passed', flush=True)


def main():
    if os.uname().machine != 'x86_64':
        raise RuntimeError('native x86_64 required')
    run_dir = ROOT / '.work/tmp/abi' / str(time.time_ns())
    run_dir.mkdir(parents=True)
    capability = {p: hashlib.sha256(p.read_bytes()).digest() for d in ('capability', 'persona') for p in (FROZEN / d).iterdir() if p.is_file()}
    runs = int(os.environ.get('ABI_RUNS') or 2)
    union = collections.defaultdict(set)
    seen = collections.Counter()
    for run in range(runs):
        observed = set()
        for label, command in WORKLOADS:
            directory = run_dir / f'{run}-{label}'
            directory.mkdir()
            environment = {**os.environ, 'MANGANESE_ABI_TRACE': str(directory)}
            if label == 'contract-7.4':
                run_contract(command, environment)
            else:
                subprocess.run(command, cwd=ROOT, env=environment, check=True)
            if not any(directory.iterdir()):
                raise RuntimeError('no browser traces for ' + label)
            for key, roles in census(directory).items():
                union[key].update(roles)
                observed.add(key)
            shutil.rmtree(directory)
        seen.update(observed)
    after = {p: hashlib.sha256(p.read_bytes()).digest() for d in ('capability', 'persona') for p in (FROZEN / d).iterdir() if p.is_file()}
    if capability != after:
        raise RuntimeError('frozen capability or persona files changed')
    archive = next(line.split() for line in (FROZEN / 'artifacts.lock').read_text().splitlines()
                   if line.endswith('manganese/oracle-inputs:hydrogen-r2-amd64') and '.tar.xz ' in line)
    header = ['# archive: ' + archive[2] + ' sha256:' + archive[0],
              '# workloads: ' + ', '.join(label for label, _ in WORKLOADS),
              '# strace symbolic decoding; flags are sets of symbolic arguments per syscall',
              '# numeric path components are placeholders; return values are omitted except ENOSYS',
              '# which process types use an item depends on scheduling, so it is reported per run, not frozen;',
              '# crashpad-only marks items only the excluded crash handler used;',
              f'# the union of {runs} runs; abi/x86_64-unstable.txt lists the items not seen in every run']
    lines = [category + '\t' + item + ('\tcrashpad-only' if types == {'crashpad'} else '')
             for (category, item), types in sorted(union.items())]
    roles = run_dir.with_name(run_dir.name + '-process-types.txt')
    roles.write_text(''.join(category + '\t' + item + '\t' + ','.join(sorted(types)) + '\n'
                             for (category, item), types in sorted(union.items())))
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text('\n'.join(header + lines) + '\n')
    unstable = [category + '\t' + item for (category, item) in sorted(union) if seen[(category, item)] < runs]
    UNSTABLE.write_text(''.join(line + '\n' for line in unstable))
    manifest = FROZEN / 'SHA256SUMS'
    rows = [line for line in manifest.read_text().splitlines() if not line.endswith(('  ./abi/x86_64.txt', '  ./abi/x86_64-unstable.txt'))]
    rows.append(hashlib.sha256(OUT.read_bytes()).hexdigest() + '  ./abi/x86_64.txt')
    rows.append(hashlib.sha256(UNSTABLE.read_bytes()).hexdigest() + '  ./abi/x86_64-unstable.txt')
    manifest.write_text('\n'.join(rows) + '\n')
    print('abi-census: ' + ', '.join(f'{category}={count}' for category, count in sorted(collections.Counter(k[0] for k in union).items())))
    shutil.rmtree(run_dir)
    print('abi-census: process types ' + str(roles))


if __name__ == '__main__':
    main()
