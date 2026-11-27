import os
import pathlib
import shlex
import subprocess

out = pathlib.Path('/work/out/chromium-wayland-release')
scratch = pathlib.Path('/lane/.work/tmp/regex')
scratch.mkdir(parents=True, exist_ok=True)
env = dict(os.environ, TMPDIR=str(scratch))
clangxx = '/opt/chromium-llvm/bin/clang++'

source = pathlib.Path('/lane/oracle/harness/regex/engine.js').read_text()
if ')ENGINE"' in source:
    raise SystemExit('engine source contains the raw-string terminator')
(scratch / 'engine.h').write_text('static const char kEngineSource[] = R"ENGINE(\n' + source + '\n)ENGINE";\n')
blob = (out / 'snapshot_blob.bin').read_bytes()
with (scratch / 'snapshot.h').open('w') as header:
    header.write('#include <stddef.h>\nstatic const unsigned char kSnapshotBlob[] = {\n')
    for start in range(0, len(blob), 24):
        header.write(','.join(str(byte) for byte in blob[start:start + 24]) + ',\n')
    header.write('};\nstatic const size_t kSnapshotBlobSize = sizeof(kSnapshotBlob);\n')

values = {}
for line in (out / 'obj/v8/regexp_fuzzer.ninja').read_text(errors='replace').splitlines():
    for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc'):
        if line.startswith(key + ' = '):
            values[key] = shlex.split(line.split(' = ', 1)[1])
args = sum((values.get(key, []) for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc')), [])
args = [arg for arg in args if not arg.startswith('-fcrash-diagnostics-dir=')]
subprocess.run([clangxx, *args, '-I' + str(scratch), '-c', '/lane/oracle/harness/regex/harness.cc',
                '-o', str(scratch / 'harness.o')], cwd=out, env=env, check=True)

link = None
link_vars = {}
lines = (out / 'obj/chrome/chrome_initial.ninja').read_text(errors='replace').splitlines()
for index, line in enumerate(lines):
    if line.startswith('build ./chrome: link '):
        link = line
        for variable in lines[index + 1:]:
            if not variable.startswith('  '):
                break
            name, value = variable.split('=', 1)
            link_vars[name.strip()] = value.strip()
        break
if link is None:
    raise SystemExit('no chrome link edge')
explicit = link.split(' link ', 1)[1].split(' | ', 1)[0].split()
objects = [path for path in explicit if path.endswith('.o')]
archives = [path for path in explicit if path.endswith('.a')]
dropped = ('-Wl,--version-script=', '-Wl,--thinlto-cache-dir=', '-Wl,--thinlto-cache-policy=',
           '-Wl,--icf=', '-rdynamic', '-Wl,-mllvm,-enable-ext-tsp-block-placement')
ldflags = [flag for flag in shlex.split(link_vars['ldflags']) if not flag.startswith(dropped)]
ldflags += ['-Wl,--thinlto-cache-dir=' + str(scratch / 'thinlto-cache')]
libs = [flag for flag in shlex.split(link_vars.get('libs', ''))]
command = [clangxx, *ldflags, '-Wl,--error-limit=0', '-o', str(scratch / 'oracle'),
           str(scratch / 'harness.o'),
           '-Wl,--start-lib', *objects, '-Wl,--end-lib',
           '-Wl,--start-group', *archives, '-Wl,--end-group',
           '-Wl,--start-group', *shlex.split(link_vars.get('rlibs', '')), '-Wl,--end-group', *libs]
print('link', len(objects), 'lazy objects and', len(archives), 'archives', flush=True)
result = subprocess.run(command, cwd=out, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
if result.returncode:
    print(result.stderr[-20000:], flush=True)
    raise SystemExit(result.returncode)
print('built', scratch / 'oracle', flush=True)
