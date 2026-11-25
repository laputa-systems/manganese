import os
import pathlib
import shlex
import subprocess

out = pathlib.Path('/work/out/chromium-wayland-release')
source = pathlib.Path('/work/src')
scratch = pathlib.Path('/lane/.work/tmp/checksums')
clang = '/opt/chromium-llvm/bin/clang++'
env = dict(os.environ, TMPDIR=str(scratch))

def flags(path, language='cc'):
    path = out / 'obj' / path
    values = {}
    for line in path.read_text().splitlines():
        for key in ('defines', 'include_dirs', 'cflags', 'cflags_c', 'cflags_cc'):
            if line.startswith(key + ' = '): values[key] = shlex.split(line.split(' = ', 1)[1])
    args = sum((values.get(key, []) for key in ('defines', 'include_dirs', 'cflags', 'cflags_' + language)), [])
    return [arg for arg in args if not arg.startswith('-fcrash-diagnostics-dir=')]

objects = []
for target, sources in [
    ('crc32c', ['crc32c.cc', 'crc32c_portable.cc']),
    ('crc32c_arm64', ['crc32c_arm64.cc']),
    ('crc32c_sse42', ['crc32c_sse42.cc']),
]:
    args = flags('third_party/crc32c/' + target + '.ninja')
    for name in sources:
        obj = scratch / (name + '.o')
        path = source / 'third_party/crc32c/src/src' / name
        subprocess.run([clang, *args, '-c', str(path), '-o', str(obj)], cwd=out, env=env, check=True)
        objects.append(str(obj))
for ninja, name, language in [
    ('base/base.ninja', 'base/hash/hash.cc', 'cc'),
    ('base/allocator/partition_allocator/src/partition_alloc/allocator_core.ninja', 'base/allocator/partition_allocator/src/partition_alloc/bounds_checks.cc', 'cc'),
    ('base/allocator/partition_allocator/src/partition_alloc/allocator_core.ninja', 'base/allocator/partition_allocator/src/partition_alloc/partition_address_space.cc', 'cc'),
    ('base/allocator/partition_allocator/src/partition_alloc/allocator_core.ninja', 'base/allocator/partition_allocator/src/partition_alloc/reservation_offset_table.cc', 'cc'),
    ('base/allocator/partition_allocator/src/partition_alloc/allocator_core.ninja', 'base/allocator/partition_allocator/src/partition_alloc/slot_address_and_size.cc', 'cc'),
    ('base/allocator/partition_allocator/src/partition_alloc/allocator_base.ninja', 'base/allocator/partition_allocator/src/partition_alloc/partition_alloc_base/debug/alias.cc', 'cc'),
    ('base/third_party/cityhash/cityhash.ninja', 'base/third_party/cityhash/city.cc', 'cc'),
    ('base/third_party/superfasthash/superfasthash.ninja', 'base/third_party/superfasthash/superfasthash.c', 'c'),
    ('third_party/leveldatabase/leveldatabase.ninja', 'third_party/leveldatabase/src/util/hash.cc', 'cc'),
]:
    obj = scratch / (name.replace('/', '_') + '.o')
    compiler = clang if language == 'cc' else '/opt/chromium-llvm/bin/clang'
    subprocess.run([compiler, *flags(ninja, language), '-c', str(source / name), '-o', str(obj)], cwd=out, env=env, check=True)
    objects.append(str(obj))
obj = scratch / 'hash-wrapper.o'
subprocess.run([clang, *flags('base/base.ninja'), '-I/work/src', '-c', '/lane/oracle/harness/checksums/hash.cc', '-o', str(obj)], cwd=out, env=env, check=True)
objects.append(str(obj))
obj = scratch / 'harness.o'
subprocess.run(['/opt/chromium-llvm/bin/clang', *flags('third_party/crc32c/crc32c.ninja', 'c'), '-I/work/src', '-c', '/lane/oracle/harness/checksums/harness.c', '-o', str(obj)], cwd=out, env=env, check=True)
subprocess.run(['/opt/chromium-llvm/bin/clang', '--target=x86_64-alpine-linux-musl', '-flto=thin', '-fuse-ld=lld', '-Wl,--gc-sections', '-pthread', str(obj), *objects, '/usr/lib/libstdc++.so.6', '-o', str(scratch / 'oracle')], cwd=out, env=env, check=True)
