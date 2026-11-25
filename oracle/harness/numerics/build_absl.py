import os
import pathlib
import shlex
import subprocess

out = pathlib.Path('/work/out/chromium-wayland-release')
scratch = pathlib.Path('/lane/.work/tmp/numerics')
env = dict(os.environ, TMPDIR=str(scratch))
compiler = '/opt/chromium-llvm/bin/clang++'
ninja = out / 'obj/third_party/abseil-cpp/absl/strings/str_format_internal.ninja'
flags = {}
sources = []
for line in ninja.read_text().splitlines():
    for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc'):
        if line.startswith(key + ' = '): flags[key] = shlex.split(line.split(' = ', 1)[1])
    if line.startswith('build ') and ': cxx ../../src/' in line:
        sources.append(line.split(': cxx ', 1)[1].split(' | ', 1)[0])
args = sum((flags[key] for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc')), [])
args = [arg for arg in args if not arg.startswith('-fcrash-diagnostics-dir=')]
for source in sources:
    obj = scratch / ('absl-' + pathlib.Path(source).name + '.o')
    subprocess.run([compiler, *args, '-c', source, '-o', str(obj)], cwd=out, env=env, check=True)
subprocess.run([compiler, *args, '-c', '/lane/oracle/harness/numerics/css_format.cc', '-o', str(scratch / 'css_format.o')], cwd=out, env=env, check=True)
flags = {}
for line in (out / 'obj/third_party/abseil-cpp/absl/strings/strings.ninja').read_text().splitlines():
    for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc'):
        if line.startswith(key + ' = '): flags[key] = shlex.split(line.split(' = ', 1)[1])
args = sum((flags[key] for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc')), [])
args = [arg for arg in args if not arg.startswith('-fcrash-diagnostics-dir=')]
subprocess.run([compiler, *args, '-c', '/work/src/third_party/abseil-cpp/absl/strings/numbers.cc', '-o', str(scratch / 'absl-numbers.cc.o')], cwd=out, env=env, check=True)
