import hashlib
import os
import pathlib
import re
import shlex
import subprocess

out = pathlib.Path('/work/out/chromium-wayland-release')
scratch = pathlib.Path('/lane/.work/tmp/url')
scratch.mkdir(parents=True, exist_ok=True)
env = dict(os.environ, TMPDIR=str(scratch))
clang = '/opt/chromium-llvm/bin/clang'
clangxx = '/opt/chromium-llvm/bin/clang++'

sources = {}
for ninja in (out / 'obj').rglob('*.ninja'):
    values = {}
    for line in ninja.read_text(errors='replace').splitlines():
        for key in ('defines', 'include_dirs', 'cflags', 'cflags_c', 'cflags_cc'):
            if line.startswith(key + ' = '):
                values[key] = shlex.split(line.split(' = ', 1)[1])
        match = re.match(r'build (obj/\S+\.o): (cc|cxx) (\S+)', line)
        if match:
            obj, language, source = match.groups()
            flags = sum((values.get(key, []) for key in ('defines', 'include_dirs', 'cflags', 'cflags_' + ('c' if language == 'cc' else 'cc'))), [])
            flags = [arg for arg in flags if not arg.startswith('-fcrash-diagnostics-dir=')]
            sources[obj] = (source, flags, language)

archives = [out / 'obj/base/libbase.a', out / 'obj/net/libnet.a',
            out / 'obj/buildtools/third_party/libc++/libc++.a']
archives += sorted((out / 'obj/base/allocator/partition_allocator/src/partition_alloc').glob('*.a'))
archives += sorted((out / 'obj/third_party/icu').glob('*.a'))
symbols = {}
for archive in archives:
    result = subprocess.run(['/opt/chromium-llvm/bin/llvm-nm', '-g', '--defined-only', '-A', str(archive)],
                            cwd=out, env=env, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, check=True)
    for line in result.stdout.splitlines():
        match = re.match(r'^(.+\.a):([^:]+):\s+[0-9a-fA-F-]+\s+[A-Za-z]\s+(\S+)$', line)
        if not match:
            continue
        path, member, symbol = match.groups()
        obj = str((pathlib.Path(path).parent / member).relative_to(out))
        if obj in sources:
            symbols.setdefault(symbol, obj)
    print('indexed', archive.name, flush=True)

selected = {obj for obj in sources if obj.startswith('obj/url/url/')}
objects = {}
def compile_source(key, source, flags, language):
    digest = hashlib.sha256((key + '\0' + '\0'.join(flags)).encode()).hexdigest()
    target = scratch / (digest + '.o')
    if not target.exists():
        print('compile', source, flush=True)
        compiler = clang if language == 'cc' else clangxx
        subprocess.run([compiler, *flags, '-c', source, '-o', str(target)], cwd=out, env=env, check=True)
    return str(target)

while True:
    for obj in sorted(selected - objects.keys()):
        source, flags, language = sources[obj]
        objects[obj] = compile_source(obj, source, flags, language)
    source, flags, language = sources['obj/url/url/gurl.o']
    harness = compile_source('harness', '/lane/oracle/harness/url/harness.cc', flags, language)
    command = [clang, '--target=x86_64-alpine-linux-musl', '-flto=thin', '-fuse-ld=lld',
               '-Wl,--gc-sections,--no-demangle,--error-limit=0', '-pthread', harness, *objects.values(),
               '/usr/lib/libstdc++.so.6', '-o', str(scratch / 'oracle')]
    result = subprocess.run(command, cwd=out, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode == 0:
        break
    missing = set(re.findall(r'undefined symbol: (\S+)', result.stderr))
    additions = {symbols[symbol] for symbol in missing if symbol in symbols} - selected
    if not additions:
        print(result.stderr, flush=True)
        raise SystemExit(result.returncode)
    print('unresolved', len(missing), 'adding sources', len(additions), flush=True)
    selected.update(additions)
