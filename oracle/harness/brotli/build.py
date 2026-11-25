import os
import pathlib
import shlex
import subprocess

out = pathlib.Path('/work/out/chromium-wayland-release')
root = out / 'obj/third_party/brotli'
tmp = pathlib.Path('/lane/.work/tmp/brotli')
clang = '/opt/chromium-llvm/bin/clang'
env = dict(os.environ, TMPDIR=str(tmp))
objects = {}
flags_by_target = {}
for target in ('common', 'dec', 'enc'):
    lines = (root / (target + '.ninja')).read_text().splitlines()
    flags = {}
    for line in lines:
        for key in ('defines', 'include_dirs', 'cflags', 'cflags_c'):
            if line.startswith(key + ' = '):
                flags[key] = shlex.split(line.split(' = ', 1)[1])
    args = sum((flags[key] for key in ('defines', 'include_dirs', 'cflags', 'cflags_c')), [])
    args = [arg for arg in args if not arg.startswith('-fcrash-diagnostics-dir=')]
    flags_by_target[target] = args
    sources = []
    for line in lines:
        if not line.startswith('build ') or ': ' not in line:
            continue
        words = shlex.split(line.split(': ', 1)[1])
        sources.extend(word for word in words[1:] if word.endswith('.c'))
    if not sources:
        raise RuntimeError(f'no C sources in {target}.ninja')
    objects[target] = []
    for index, source in enumerate(sources):
        obj = tmp / f'{target}-{index}.o'
        subprocess.run([clang, *args, '-c', source, '-o', str(obj)], cwd=out, env=env, check=True)
        objects[target].append(str(obj))
for name, source, target, parts in (
    ('oracle', 'harness.c', 'dec', ('common', 'dec')),
    ('encode', 'encode.c', 'enc', ('common', 'enc')),
):
    obj = tmp / (name + '.o')
    subprocess.run([clang, *flags_by_target[target], '-c', '/lane/oracle/harness/brotli/' + source, '-o', str(obj)], cwd=out, env=env, check=True)
    libs = [obj for part in parts for obj in objects[part]]
    subprocess.run([clang, '--target=x86_64-alpine-linux-musl', '-flto=thin', '-fuse-ld=lld', '-pthread', str(obj), *libs, '-o', str(tmp / name)], cwd=out, env=env, check=True)
