import os
import pathlib
import shlex
import subprocess

out = pathlib.Path('/work/out/chromium-wayland-release')
scratch = pathlib.Path('/lane/.work/tmp/numerics')
env = dict(os.environ, TMPDIR=str(scratch))
compiler = '/opt/chromium-llvm/bin/clang++'
base = 'base/allocator/partition_allocator/src/partition_alloc/'
for target, sources in [
    ('allocator_core', ['bounds_checks.cc', 'partition_address_space.cc', 'reservation_offset_table.cc', 'slot_address_and_size.cc']),
    ('allocator_base', ['partition_alloc_base/debug/alias.cc']),
]:
    flags = {}
    for line in (out / 'obj' / base / (target + '.ninja')).read_text().splitlines():
        for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc'):
            if line.startswith(key + ' = '): flags[key] = shlex.split(line.split(' = ', 1)[1])
    args = sum((flags[key] for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc')), [])
    args = [arg for arg in args if not arg.startswith('-fcrash-diagnostics-dir=')]
    for source in sources:
        output = scratch / ('allocator-' + source.replace('/', '_') + '.o')
        subprocess.run([compiler, *args, '-c', '/work/src/' + base + source, '-o', str(output)], cwd=out, env=env, check=True)

subprocess.run([compiler, *args, '-c', '/lane/oracle/harness/numerics/stack_only.cc', '-o', str(scratch / 'stack_only.o')], cwd=out, env=env, check=True)
