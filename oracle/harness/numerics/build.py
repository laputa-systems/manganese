import os
import pathlib
import shlex
import subprocess
import sys

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
harness_flags = {}
for line in (out / 'obj/base/base.ninja').read_text().splitlines():
    for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc'):
        if line.startswith(key + ' = '):
            harness_flags[key] = shlex.split(line.split(' = ', 1)[1])
harness_args = sum((harness_flags[key] for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc')), [])
harness_args = [arg for arg in harness_args if not arg.startswith('-fcrash-diagnostics-dir=')]
obj = scratch / 'harness.o'
subprocess.run([compiler, *harness_args, '-c', '/lane/oracle/harness/numerics/harness.cc', '-o', str(obj)], cwd=out, env=env, check=True)
for path in sorted(source.glob('*.cc')):
    obj = scratch / (path.name + '.o')
    subprocess.run([compiler, *args, '-c', str(path), '-o', str(obj)], cwd=out, env=env, check=True)
    objects.append(str(obj))
for path in ('base/json/json_writer.cc', 'base/strings/string_number_conversions.cc'):
    target = scratch / (path.replace('/', '_') + '.o')
    subprocess.run([compiler, *harness_args, '-c', '/work/src/' + path, '-o', str(target)], cwd=out, env=env, check=True)
    objects.append(str(target))
subprocess.run([sys.executable, '-B', '/lane/oracle/harness/numerics/build_allocator.py'], cwd=out, env=env, check=True)
objects.extend(str(scratch / name) for name in ('allocator-bounds_checks.cc.o', 'allocator-partition_address_space.cc.o', 'allocator-reservation_offset_table.cc.o', 'allocator-slot_address_and_size.cc.o', 'allocator-partition_alloc_base_debug_alias.cc.o', 'stack_only.o'))
blink_flags = {}
for line in (out / 'obj/third_party/blink/renderer/platform/wtf/wtf.ninja').read_text().splitlines():
    for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc'):
        if line.startswith(key + ' = '):
            blink_flags[key] = shlex.split(line.split(' = ', 1)[1])
blink_args = sum((blink_flags[key] for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc')), [])
blink_args = [arg for arg in blink_args if not arg.startswith('-fcrash-diagnostics-dir=')]
blink_obj = scratch / 'blink-dtoa.o'
subprocess.run([compiler, *blink_args, '-c', '/work/src/third_party/blink/renderer/platform/wtf/dtoa.cc', '-o', str(blink_obj)], cwd=out, env=env, check=True)
objects.append(str(blink_obj))
simd_flags = {}
for line in (out / 'obj/third_party/simdutf/simdutf.ninja').read_text().splitlines():
    for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc'):
        if line.startswith(key + ' = '):
            simd_flags[key] = shlex.split(line.split(' = ', 1)[1])
simd_args = sum((simd_flags[key] for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc')), [])
simd_args = [arg for arg in simd_args if not arg.startswith('-fcrash-diagnostics-dir=')]
obj = scratch / 'simdutf.o'
subprocess.run([compiler, *simd_args, '-c', '/work/src/third_party/simdutf/simdutf.cpp', '-o', str(obj)], cwd=out, env=env, check=True)
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
subprocess.run(['/opt/chromium-llvm/bin/clang', '--target=x86_64-alpine-linux-musl', '-flto=thin', '-fuse-ld=lld', '-Wl,--gc-sections', '-pthread', str(scratch / 'harness.o'), *objects, str(scratch / 'json-build/target/x86_64-unknown-linux-musl/debug/libjson_oracle.a'), '/usr/lib/libstdc++.so.6', '/usr/lib/libgcc_s.so.1', '-o', str(scratch / 'oracle')], cwd=out, env=env, check=True)
