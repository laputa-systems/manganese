import os
import pathlib
import re
import shlex
import subprocess

out = pathlib.Path('/work/out/chromium-wayland-release')
lane = pathlib.Path('/lane/.work/tmp/png')
lane.mkdir(parents=True, exist_ok=True)
env = dict(os.environ, TMPDIR=str(lane), KACHE_DISABLED='1')

flags = {}
for line in (out / 'obj/skia/skia.ninja').read_text().splitlines():
    for name in ('defines', 'include_dirs', 'cflags', 'cflags_cc'):
        if line.startswith(name + ' = '):
            flags[name] = shlex.split(line.split(' = ', 1)[1])
args = sum((flags[name] for name in ('defines', 'include_dirs', 'cflags', 'cflags_cc')), [])
args = [arg for arg in args if not arg.startswith('-fcrash-diagnostics-dir=')]
subprocess.run(['/opt/chromium-llvm/bin/clang++', *args, '-isystem', '../../src/build/config/musl-compat/include',
                '-c', '/lane/oracle/harness/png/harness.cc', '-o', str(lane / 'harness.o')], cwd=out, env=env, check=True)

reference = (out / 'obj/skia/image_operations_bench.ninja').read_text().splitlines()
edge = next(line for line in reference if line.startswith('build ./image_operations_bench'))
variables = {}
for line in reference:
    match = re.match(r'  (ldflags|libs|rlibs) = (.*)', line)
    if match:
        variables[match.group(1)] = match.group(2).replace('$:', ':')
inputs = edge.split(':', 1)[1].split()[1:]
inputs = inputs[:inputs.index('|')]
objects = [item for item in inputs if item.endswith('.o') and 'image_operations_bench' not in item]
archives = [item for item in inputs if not item.endswith('.o')]
ldflags = variables['ldflags'].replace('-Wl,--thinlto-cache-dir=thinlto-cache', '-Wl,--thinlto-cache-dir=' + str(lane / 'thinlto'))
command = ['/opt/chromium-llvm/bin/clang++', *shlex.split(ldflags), '-Wl,--thinlto-jobs=4',
           '-o', str(lane / 'oracle'), str(lane / 'harness.o'),
           '-Wl,--start-lib', *objects, '-Wl,--end-lib', *archives,
           *shlex.split(variables['libs']), '-Wl,--start-group', *shlex.split(variables['rlibs']), '-Wl,--end-group']
subprocess.run(command, cwd=out, env=env, check=True)
