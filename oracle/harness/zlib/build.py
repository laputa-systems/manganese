import os
import pathlib
import shlex
import subprocess

out = pathlib.Path('/work/out/chromium-wayland-release')
zlib = out / 'obj/third_party/zlib'
lines = (zlib / 'zlib.ninja').read_text().splitlines()
flags = {}
for line in lines:
    for key in ('defines', 'include_dirs', 'cflags', 'cflags_c'):
        if line.startswith(key + ' = '):
            flags[key] = shlex.split(line.split(' = ', 1)[1])
args = sum((flags[key] for key in ('defines', 'include_dirs', 'cflags', 'cflags_c')), [])
args = [arg for arg in args if not arg.startswith('-fcrash-diagnostics-dir=')]
env = dict(os.environ, TMPDIR='/lane/.work/tmp/zlib')
clang = '/opt/chromium-llvm/bin/clang'
obj = '/lane/.work/tmp/zlib/harness.o'
binary = '/lane/.work/tmp/zlib/oracle'
subprocess.run([clang, *args, '-c', '/lane/oracle/harness/zlib/harness.c', '-o', obj], cwd=out, env=env, check=True)
objects = sorted(str(path) for path in zlib.glob('zlib/*.o'))
for name in ('zlib_adler32_simd', 'zlib_crc32_simd', 'zlib_data_chunk_simd'):
    objects.extend(sorted(str(path) for path in (zlib / name).glob('*.o')))
subprocess.run([clang, '--target=x86_64-alpine-linux-musl', '-flto=thin', '-fuse-ld=lld', '-pthread', obj, *objects, '-o', binary], cwd=out, env=env, check=True)
