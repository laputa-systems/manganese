import os
import pathlib
import shlex
import subprocess

out = pathlib.Path('/work/out/chromium-wayland-release')
scratch = pathlib.Path('/lane/.work/tmp/compression-misc')
scratch.mkdir(parents=True, exist_ok=True)
env = dict(os.environ, TMPDIR=str(scratch))
clangxx = '/opt/chromium-llvm/bin/clang++'

flags = {}
for line in (out / 'obj/third_party/snappy/snappy.ninja').read_text().splitlines():
    for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc'):
        if line.startswith(key + ' = '):
            flags[key] = shlex.split(line.split(' = ', 1)[1])

drop_prefixes = (
    '-fprofile-use=', '-fcrash-diagnostics-dir=', '-flto=thin', '-fsplit-lto-unit',
    '-fwhole-program-vtables', '-fcomplete-member-pointers', '-Wno-profile-',
)
args = []
for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc'):
    args.extend(flag for flag in flags[key] if not flag.startswith(drop_prefixes))

sources = [
    pathlib.Path('/work/src/third_party/snappy/src/snappy.cc'),
    pathlib.Path('/work/src/third_party/snappy/src/snappy-sinksource.cc'),
    pathlib.Path('/work/src/third_party/snappy/src/snappy-stubs-internal.cc'),
    pathlib.Path('/lane/oracle/harness/compression-misc/harness.cc'),
    pathlib.Path('/lane/oracle/harness/compression-misc/runtime.cc'),
]
objects = []
for source in sources:
    target = scratch / (source.stem + '.o')
    subprocess.run([clangxx, *args, '-c', str(source), '-o', str(target)], cwd=out, env=env, check=True)
    objects.append(str(target))

binary = scratch / 'oracle'
subprocess.run([
    clangxx, '--target=x86_64-alpine-linux-musl', '-fuse-ld=lld', '-nostdlib++', '-pthread', '-Wl,--gc-sections',
    *objects,
    '-o', str(binary),
], cwd=out, env=env, check=True)
print('built', binary, flush=True)
