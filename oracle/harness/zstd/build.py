import os
import pathlib
import shlex
import subprocess

out = pathlib.Path('/work/out/chromium-wayland-release')
target = out / 'obj/third_party/zstd'
scratch = pathlib.Path('/lane/.work/tmp/zstd')
env = dict(os.environ, TMPDIR=str(scratch))
clang = '/opt/chromium-llvm/bin/clang'
objects = []
for name in ('common', 'decompress', 'compress'):
    lines = (target / (name + '.ninja')).read_text().splitlines()
    flags = {}
    for line in lines:
        for key in ('defines', 'include_dirs', 'cflags', 'cflags_c', 'asmflags'):
            if line.startswith(key + ' = '):
                flags[key] = shlex.split(line.split(' = ', 1)[1])
    args = sum((flags[key] for key in ('defines', 'include_dirs', 'cflags', 'cflags_c')), [])
    args = [arg for arg in args if not arg.startswith('-fcrash-diagnostics-dir=')]
    sources = [line.split(': cc ', 1)[1].split(' | ', 1)[0] for line in lines if ': cc ' in line]
    for index, source in enumerate(sources):
        obj = scratch / (name + '-' + str(index) + '.o')
        subprocess.run([clang, *args, '-c', source, '-o', str(obj)], cwd=out, env=env, check=True)
        objects.append(str(obj))
    for index, line in enumerate(line for line in lines if ': asm ' in line):
        source = line.split(': asm ', 1)[1].split(' | ', 1)[0]
        asm_args = flags['defines'] + flags['include_dirs'] + flags['asmflags']
        asm_args = [arg for arg in asm_args if not arg.startswith('-fcrash-diagnostics-dir=')]
        obj = scratch / (name + '-asm-' + str(index) + '.o')
        subprocess.run([clang, *asm_args, '-c', source, '-o', str(obj)], cwd=out, env=env, check=True)
        objects.append(str(obj))
    if name == 'decompress':
        harness_flags = args
obj = scratch / 'harness.o'
subprocess.run([clang, *harness_flags, '-c', '/lane/oracle/harness/zstd/harness.c', '-o', str(obj)], cwd=out, env=env, check=True)
subprocess.run([clang, '--target=x86_64-alpine-linux-musl', '-flto=thin', '-fuse-ld=lld', '-pthread', str(obj), *objects, '-o', str(scratch / 'oracle')], cwd=out, env=env, check=True)
