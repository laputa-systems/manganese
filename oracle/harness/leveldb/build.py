import concurrent.futures
import hashlib
import os
import pathlib
import re
import shlex
import subprocess

out = pathlib.Path('/work/out/chromium-wayland-release')
harness = pathlib.Path('/lane/oracle/harness/leveldb')
scratch = pathlib.Path('/lane/.work/tmp/leveldb')
scratch.mkdir(parents=True, exist_ok=True)
clang = '/opt/chromium-llvm/bin/clang'
env = dict(os.environ, TMPDIR=str(scratch))
ALL = None
RULES = {'cc': 'cxx', 'c': 'cc', 'asm': 'asm'}


def read_target(name, language='cc'):
    contents = (out / 'obj' / name).read_text()
    values = {}
    for line in contents.splitlines():
        for key in ('defines', 'include_dirs', 'cflags', 'cflags_c', 'cflags_cc', 'asmflags'):
            if line.startswith(key + ' = '):
                values[key] = shlex.split(line.split(' = ', 1)[1])
    keys = ('defines', 'include_dirs', 'asmflags') if language == 'asm' else ('defines', 'include_dirs', 'cflags', 'cflags_' + language)
    args = [arg for arg in sum((values.get(key, []) for key in keys), []) if not arg.startswith('-fcrash-diagnostics-dir=')]
    sources = re.findall(r'^build \S+: ' + RULES[language] + r' (\.\./\.\./src/\S+)', contents, re.MULTILINE)
    return args, sources


PA = 'base/allocator/partition_allocator/src/partition_alloc/'
targets = [
    ('third_party/leveldatabase/leveldatabase.ninja', 'cc', ALL),
    ('components/services/storage/public/cpp/filesystem/filesystem.ninja', 'cc', ALL),
    ('third_party/snappy/snappy.ninja', 'cc', ALL),
    ('third_party/crc32c/crc32c.ninja', 'cc', ALL),
    ('third_party/crc32c/crc32c_arm64.ninja', 'cc', ALL),
    ('third_party/crc32c/crc32c_sse42.ninja', 'cc', ALL),
    ('buildtools/third_party/libc++/libc++.ninja', 'cc', ALL),
    ('buildtools/third_party/libc++abi/libc++abi.ninja', 'cc', ALL),
    ('base/base.ninja', 'cc', lambda name: name != 'message_pump_glib.cc'),
    (PA + 'allocator_core.ninja', 'cc', ALL),
    (PA + 'allocator_base.ninja', 'cc', ALL),
    (PA + 'raw_ptr.ninja', 'cc', ALL),
    ('third_party/abseil-cpp/absl/strings/str_format_internal.ninja', 'cc', ALL),
    ('third_party/abseil-cpp/absl/strings/internal.ninja', 'cc', ALL),
    ('third_party/abseil-cpp/absl/strings/strings.ninja', 'cc', ALL),
    ('third_party/abseil-cpp/absl/base/raw_logging_internal.ninja', 'cc', ALL),
    ('third_party/abseil-cpp/absl/base/throw_delegate.ninja', 'cc', ALL),
    ('third_party/abseil-cpp/absl/container/raw_hash_set.ninja', 'cc', ALL),
    ('third_party/abseil-cpp/absl/hash/hash.ninja', 'cc', ALL),
    ('third_party/abseil-cpp/absl/container/hashtablez_sampler.ninja', 'cc', ALL),
    ('base/third_party/double_conversion/double_conversion.ninja', 'cc', ALL),
    ('third_party/perfetto/src/base/base.ninja', 'cc', {'logging.cc', 'file_utils.cc', 'progress_reporter.cc', 'string_splitter.cc'}),
    ('third_party/perfetto/src/base/perfetto_base_default_platform.ninja', 'cc', ALL),
    ('third_party/perfetto/src/protozero/protozero.ninja', 'cc',
     {'message.cc', 'scattered_stream_writer.cc', 'message_arena.cc', 'scattered_heap_buffer.cc'}),
    ('third_party/perfetto/src/tracing/client_api_without_backends.ninja', 'cc',
     {'data_source.cc', 'event_context.cc', 'traced_value.cc', 'tracing_muxer_impl.cc', 'track.cc',
      'track_event_internal.cc', 'platform.cc', 'tracing_muxer_fake.cc', 'track_event_interned_fields.cc',
      'debug_annotation.cc', 'track_event_category_registry.cc'}),
    ('mojo/public/cpp/bindings/bindings.ninja', 'cc', {'interface_ptr_state.cc'}),
    ('mojo/public/cpp/bindings/bindings_base.ninja', 'cc', {'associated_group.cc', 'pending_remote_state.cc'}),
    ('third_party/boringssl/boringssl.ninja', 'cc',
     {'sha1.cc', 'sha256.cc', 'md5.cc', 'bcm.cc', 'mem.cc', 'cpu_intel.cc', 'crypto.cc', 'thread_pthread.cc'}),
    ('third_party/boringssl/boringssl_asm.ninja', 'asm',
     {'md5-x86_64-linux.S', 'sha1-x86_64-linux.S', 'sha256-x86_64-linux.S'}),
    ('third_party/zlib/zlib.ninja', 'c', {'crc32.c', 'cpu_features.c'}),
    ('third_party/zlib/zlib_crc32_simd.ninja', 'c', ALL),
]
jobs = []
for target, language, select in targets:
    args, sources = read_target(target, language)
    for source in sources:
        name = pathlib.Path(source).name
        if select is ALL or (callable(select) and select(name)) or (isinstance(select, set) and name in select):
            jobs.append((target, args, source))
jobs.append(('stubs', read_target('base/base.ninja', 'c')[0], str(harness / 'unreachable.c')))
jobs.append(('harness', read_target(targets[0][0])[0], str(harness / 'harness.cc')))


def compile_one(job):
    target, args, source = job
    key = hashlib.sha256((target + '\0' + source).encode()).hexdigest()[:20]
    obj = scratch / (key + '.o')
    stamp = scratch / (key + '.stamp')
    signature = hashlib.sha256(('\0'.join(args) + '\0' + source).encode()).hexdigest()
    source_path = pathlib.Path(source) if source.startswith('/') else out / source
    if not obj.exists() or not stamp.exists() or stamp.read_text() != signature or obj.stat().st_mtime_ns < source_path.stat().st_mtime_ns:
        print('compile', target, source, flush=True)
        compiler = clang if source.endswith(('.c', '.S')) else clang + '++'
        subprocess.run([compiler, *args, '-c', source, '-o', str(obj)], cwd=out, env=env, check=True)
        stamp.write_text(signature)
    return str(obj)


with concurrent.futures.ThreadPoolExecutor(max_workers=3) as executor:
    objects = list(executor.map(compile_one, jobs))
archive = scratch / 'chromium.a'
archive.unlink(missing_ok=True)
subprocess.run(['/opt/chromium-llvm/bin/llvm-ar', 'rcs', str(archive), *objects[:-1]], check=True)
subprocess.run([clang + '++', '--target=x86_64-alpine-linux-musl', '-flto=thin', '-fuse-ld=lld', '-nostdlib++',
                '-Wl,--gc-sections,--error-limit=0,--no-demangle', '-pthread', objects[-1], str(archive),
                '/usr/lib/libgcc_s.so.1', '-o', str(scratch / 'oracle')],
               cwd=out, env=env, check=True)
