import os
import pathlib
import shlex
import subprocess

out = pathlib.Path('/work/out/chromium-wayland-release')
source = pathlib.Path('/work/src/base/third_party/double_conversion/double-conversion')
scratch = pathlib.Path('/lane/.work/tmp/numerics')
flags = {}
for line in (out / 'obj/base/third_party/double_conversion/double_conversion.ninja').read_text().splitlines():
    for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc'):
        if line.startswith(key + ' = '):
            flags[key] = shlex.split(line.split(' = ', 1)[1])
args = sum((flags[key] for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc')), [])
args = [arg for arg in args if not arg.startswith('-fcrash-diagnostics-dir=')]
env = dict(os.environ, TMPDIR=str(scratch))
compiler = '/opt/chromium-llvm/bin/clang++'
objects = []
for path in sorted(source.glob('*.cc')):
    obj = scratch / (path.name + '.o')
    subprocess.run([compiler, *args, '-c', str(path), '-o', str(obj)], cwd=out, env=env, check=True)
    objects.append(str(obj))
for target in ('libc++', 'libc++abi'):
    ninja = out / ('obj/buildtools/third_party/' + target + '/' + target + '.ninja')
    target_flags = {}
    source_paths = []
    for line in ninja.read_text().splitlines():
        for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc'):
            if line.startswith(key + ' = '):
                target_flags[key] = shlex.split(line.split(' = ', 1)[1])
        if line.startswith('build ') and ': cxx ../../src/' in line:
            source_paths.append(line.split(': cxx ', 1)[1].split(' | ', 1)[0])
    target_args = sum((target_flags[key] for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc')), [])
    target_args = [arg for arg in target_args if not arg.startswith('-fcrash-diagnostics-dir=')]
    for index, path in enumerate(source_paths):
        obj = scratch / (target + '-' + str(index) + '.o')
        subprocess.run([compiler, *target_args, '-c', path, '-o', str(obj)], cwd=out, env=env, check=True)
        objects.append(str(obj))
obj = scratch / 'harness.o'
subprocess.run([compiler, *args, '-c', '/lane/oracle/harness/numerics/harness.cc', '-o', str(obj)], cwd=out, env=env, check=True)
subprocess.run(['/opt/chromium-llvm/bin/clang', '--target=x86_64-alpine-linux-musl', '-flto=thin', '-fuse-ld=lld', '-Wl,--gc-sections', '-pthread', str(obj), *objects, '/usr/lib/libstdc++.so.6', '/usr/lib/libgcc_s.so.1', '-o', str(scratch / 'oracle')], cwd=out, env=env, check=True)
