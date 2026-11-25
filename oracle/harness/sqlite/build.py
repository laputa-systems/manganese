import hashlib
import os
import pathlib
import re
import shlex
import subprocess

out = pathlib.Path('/work/out/chromium-wayland-release')
scratch = pathlib.Path('/lane/.work/tmp/sqlite')
objects_dir = scratch / 'objects'
objects_dir.mkdir(parents=True, exist_ok=True)
env = dict(os.environ, TMPDIR=str(scratch))
clang = '/opt/chromium-llvm/bin/clang'
clangxx = '/opt/chromium-llvm/bin/clang++'

sources = {}
roots = ('obj/sql', 'obj/third_party/sqlite', 'obj/base', 'obj/third_party/icu', 'obj/buildtools/third_party/libc++',
         'obj/buildtools/third_party/libc++abi', 'obj/third_party/perfetto', 'obj/third_party/abseil-cpp',
         'obj/third_party/boringssl', 'obj/third_party/protobuf', 'obj/third_party/simdutf', 'obj/third_party/crashpad',
         'obj/third_party/zlib')
for ninja in (path for root in roots for path in (out / root).rglob('*.ninja')):
    values = {}
    for line in ninja.read_text(errors='replace').splitlines():
        for key in ('defines', 'include_dirs', 'cflags', 'cflags_c', 'cflags_cc', 'asmflags'):
            if line.startswith(key + ' = '):
                values[key] = shlex.split(line.split(' = ', 1)[1])
        match = re.match(r'build (obj/\S+\.o): (cc|cxx|asm) (\S+)', line)
        if match:
            obj, language, source = match.groups()
            keys = ('defines', 'include_dirs', 'asmflags') if language == 'asm' else ('defines', 'include_dirs', 'cflags', 'cflags_' + ('c' if language == 'cc' else 'cc'))
            flags = sum((values.get(key, []) for key in keys), [])
            flags = [arg for arg in flags if not arg.startswith('-fcrash-diagnostics-dir=')]
            sources[obj] = (source, flags, language)

archives = [out / 'obj/sql/libsql.a', out / 'obj/third_party/sqlite/libchromium_sqlite3.a', out / 'obj/base/libbase.a',
            out / 'obj/buildtools/third_party/libc++/libc++.a', out / 'obj/buildtools/third_party/libc++abi/libc++abi.a',
            out / 'obj/third_party/boringssl/libboringssl.a', out / 'obj/third_party/zlib/libchrome_zlib.a']
archives += sorted((out / 'obj/base/allocator/partition_allocator/src/partition_alloc').glob('*.a'))
archives += sorted((out / 'obj/third_party/icu').glob('*.a'))
archives += sorted((out / 'obj/third_party/protobuf').glob('*.a'))
symbols = {}
for archive in archives:
    if not archive.is_file():
        continue
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

source_objects = [obj for obj in sources if obj.startswith(('obj/third_party/perfetto/', 'obj/third_party/abseil-cpp/', 'obj/third_party/simdutf/', 'obj/third_party/crashpad/', 'obj/base/third_party/double_conversion/', 'obj/third_party/protobuf/')) and (out / obj).is_file()]
for start in range(0, len(source_objects), 100):
    batch = source_objects[start:start + 100]
    result = subprocess.run(['/opt/chromium-llvm/bin/llvm-nm', '-g', '--defined-only', '-A', *batch], cwd=out, env=env, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, check=True)
    for line in result.stdout.splitlines():
        match = re.match(r'^(obj/\S+\.o):\s+[0-9a-fA-F-]+\s+[A-Za-z]\s+(\S+)$', line)
        if match:
            obj, symbol = match.groups()
            symbols.setdefault(symbol, obj)
print('indexed source-set objects', len(source_objects), flush=True)

selected = {obj for obj in sources if obj.startswith(('obj/sql/sql/', 'obj/third_party/sqlite/chromium_sqlite3/'))}
objects = {}


def compile_source(key, source, flags, language):
    source_digest = pathlib.Path(source).read_bytes() if key == 'harness' else b''
    digest = hashlib.sha256((key + '\0' + '\0'.join(flags)).encode() + source_digest).hexdigest()
    target = objects_dir / (digest + '.o')
    if not target.exists():
        print('compile', source, flush=True)
        compiler = clangxx if language == 'cxx' else clang
        partial = target.with_suffix('.partial')
        subprocess.run([compiler, *flags, '-c', source, '-o', str(partial)], cwd=out, env=env, check=True)
        partial.rename(target)
    return str(target)


source, flags, language = sources['obj/sql/sql/database.o']
harness = compile_source('harness', '/lane/oracle/harness/sqlite/harness.cc', flags, language)
while True:
    for obj in sorted(selected - objects.keys()):
        source, flags, language = sources[obj]
        objects[obj] = compile_source(obj, source, flags, language)
    command = [clang, '--target=x86_64-alpine-linux-musl', '-flto=thin', '-fuse-ld=lld',
               '-Wl,--gc-sections,--no-demangle,--error-limit=0,--thinlto-jobs=2', '-pthread', harness,
               '-Wl,--start-lib', *objects.values(), '-Wl,--end-lib',
               '/usr/lib/libstdc++.so.6', '/usr/lib/libgcc_s.so.1', '-o', str(scratch / 'oracle.partial')]
    result = subprocess.run(command, cwd=out, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode == 0:
        (scratch / 'oracle.partial').rename(scratch / 'oracle')
        break
    missing = set(re.findall(r'undefined symbol: (\S+)', result.stderr))
    additions = {symbols[symbol] for symbol in missing if symbol in symbols} - selected
    if not additions:
        print(result.stderr, flush=True)
        raise SystemExit(result.returncode)
    print('unresolved', len(missing), 'adding sources', len(additions), flush=True)
    selected.update(additions)
print('built', scratch / 'oracle', 'from', len(objects), 'objects', flush=True)
