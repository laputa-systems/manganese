import os
import pathlib
import shlex
import subprocess

out = pathlib.Path('/work/out/chromium-wayland-release')
lane = pathlib.Path('/lane/.work/tmp/png')
lane.mkdir(parents=True, exist_ok=True)
flags = {}
for line in (out / 'obj/third_party/blink/renderer/platform/image-decoders/image_decoders.ninja').read_text().splitlines():
    for name in ('defines', 'include_dirs', 'cflags', 'cflags_cc'):
        if line.startswith(name + ' = '):
            flags[name] = shlex.split(line.split(' = ', 1)[1])
args = sum((flags[name] for name in ('defines', 'include_dirs', 'cflags', 'cflags_cc')), [])
args = [arg for arg in args if not arg.startswith('-fcrash-diagnostics-dir=')]
env = dict(os.environ, TMPDIR=str(lane), KACHE_DISABLED='1')
subprocess.run(['/opt/chromium-llvm/bin/clang++', *args, '-isystem', '../../src/build/config/musl-compat/include', '-c', '/lane/oracle/harness/png/harness.cc', '-o', str(lane / 'harness.o')], cwd=out, env=env, check=True)
