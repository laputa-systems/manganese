import concurrent.futures
import hashlib
import os
import pathlib
import re
import shlex
import subprocess

out = pathlib.Path('/work/out/chromium-wayland-release')
scratch = pathlib.Path('/lane/.work/tmp/leveldb')
scratch.mkdir(parents=True, exist_ok=True)
clang = '/opt/chromium-llvm/bin/clang++'
env = dict(os.environ, TMPDIR=str(scratch))


def read_target(name, language='cc'):
    contents = (out / 'obj' / name).read_text()
    values = {}
    for line in contents.splitlines():
        for key in ('defines', 'include_dirs', 'cflags', 'cflags_c', 'cflags_cc'):
            if line.startswith(key + ' = '):
                values[key] = shlex.split(line.split(' = ', 1)[1])
    args = sum((values.get(key, []) for key in ('defines', 'include_dirs', 'cflags', 'cflags_' + language)), [])
    args = [arg for arg in args if not arg.startswith('-fcrash-diagnostics-dir=')]
    rule = 'cxx' if language == 'cc' else 'cc'
    sources = re.findall(r'^build \S+: ' + rule + r' (\.\./\.\./src/\S+)', contents, re.MULTILINE)
    return args, sources


targets = [
    'third_party/leveldatabase/leveldatabase.ninja',
    'third_party/snappy/snappy.ninja',
    'third_party/crc32c/crc32c.ninja',
    'third_party/crc32c/crc32c_arm64.ninja',
    'third_party/crc32c/crc32c_sse42.ninja',
    'components/services/storage/public/cpp/filesystem/filesystem.ninja',
    'buildtools/third_party/libc++/libc++.ninja',
    'buildtools/third_party/libc++abi/libc++abi.ninja',
]
jobs = []
for target in targets:
    args, sources = read_target(target)
    for source in sources:
        jobs.append((target, args, source))
base_target = 'base/base.ninja'
base_args, base_sources = read_target(base_target)
base_names = {
    'feature_list.cc', 'feature_list_internal.cc', 'file.cc', 'file_posix.cc', 'file_path.cc',
    'file_enumerator.cc', 'file_enumerator_posix.cc', 'file_util.cc', 'file_util_posix.cc',
    'location.cc', 'memory_pressure_listener.cc', 'memory_pressure_listener_registry.cc',
    'strcat.cc', 'string_number_conversions.cc', 'string_util.cc', 'stringprintf.cc',
    'lock.cc', 'lock_impl_posix.cc', 'condition_variable_posix.cc', 'platform_thread.cc',
    'platform_thread_posix.cc', 'platform_thread_linux.cc', 'platform_thread_linux_base.cc',
    'thread_restrictions.cc', 'sys_info.cc', 'sys_info_posix.cc', 'sys_info_linux.cc',
    'time.cc', 'time_override.cc', 'time_now_posix.cc', 'time_exploded_posix.cc',
    'callback_internal.cc', 'command_line.cc', 'crash_logging.cc', 'current_thread.cc',
    'features.cc', 'field_trial_params.cc', 'hash.cc', 'histogram.cc', 'histogram_base.cc',
    'histogram_functions.cc', 'interned_args_helper.cc', 'lock_metrics_recorder.cc', 'logging.cc',
    'memory_pressure_level_proto.cc', 'metrics_hashes.cc', 'observer_list_types.cc',
    'platform_thread_internal_posix.cc', 'process_metrics_posix.cc', 'scoped_blocking_call.cc',
    'scoped_file.cc', 'scoped_file_linux.cc', 'sequenced_task_runner.cc',
    'single_thread_task_runner.cc', 'sys_string_conversions_posix.cc', 'task_runner.cc',
    'thread_id_name_manager.cc', 'thread_pool.cc', 'trace_event_impl.cc', 'utf_string_conversions.cc',
    'weak_ptr.cc', 'alias.cc', 'atomic_flag.cc', 'bucket_ranges.cc', 'check.cc', 'dummy_histogram.cc',
    'dump_without_crashing.cc', 'field_trial_param_associator.cc', 'histogram_samples.cc',
    'histogram_scope.cc', 'lazy_instance_helpers.cc', 'persistent_histogram_allocator.cc', 'pickle.cc',
    'safe_strerror.cc', 'sample_vector.cc', 'scoped_blocking_call_internal.cc', 'sequence_manager_impl.cc',
    'sparse_histogram.cc', 'statistics_recorder.cc', 'thread_pool_impl.cc', 'thread_pool_instance.cc',
    'values.cc',
    'crc32.cc', 'environment_config.cc', 'message_pump.cc', 'pending_task.cc',
    'persistent_memory_allocator.cc', 'ranges_manager.cc', 'sample_map.cc',
    'scoped_set_task_priority_for_current_thread.cc', 'sequence.cc', 'task.cc', 'thread_type.cc',
    'intrusive_heap.cc', 'sequence_local_storage_map.cc', 'sequence_token.cc', 'task_source.cc', 'task_traits.cc',
}
jobs.extend((base_target, base_args, source) for source in base_sources if pathlib.Path(source).name in base_names)
for target, names in [
    ('base/allocator/partition_allocator/src/partition_alloc/allocator_core.ninja',
     {'bounds_checks.cc', 'partition_address_space.cc', 'reservation_offset_table.cc', 'slot_address_and_size.cc',
      'stack.cc', 'dangling_raw_ptr_checks.cc', 'intended_leak.cc', 'internal_allocator.cc', 'partition_page.cc',
      'partition_root.cc', 'scheduler_loop_quarantine.cc', 'spinning_mutex.cc', 'thread_cache.cc',
      'address_pool_manager.cc', 'allocator_config.cc', 'in_slot_metadata.cc', 'page_allocator.cc',
      'page_allocator_internals_posix.cc', 'partition_alloc_hooks.cc', 'partition_bucket.cc',
      'partition_dcheck_helper.cc', 'partition_freelist_entry.cc', 'pkey.cc', 'thread_isolation.cc',
      'address_space_randomization.cc', 'oom.cc', 'partition_oom.cc'}),
    ('base/allocator/partition_allocator/src/partition_alloc/allocator_base.ninja',
     {'alias.cc', 'platform_thread_posix.cc', 'rand_util.cc', 'rand_util_posix.cc', 'time.cc', 'time_now_posix.cc'}),
    ('base/allocator/partition_allocator/src/partition_alloc/raw_ptr.ninja', {'raw_ptr_backup_ref_impl.cc'}),
    ('third_party/abseil-cpp/absl/strings/str_format_internal.ninja',
     {'arg.cc', 'bind.cc', 'extension.cc', 'float_conversion.cc', 'parser.cc'}),
    ('third_party/abseil-cpp/absl/strings/internal.ninja', {'utf8.cc'}),
    ('third_party/abseil-cpp/absl/strings/strings.ninja', {'numbers.cc'}),
    ('third_party/perfetto/src/base/base.ninja', {'logging.cc', 'file_utils.cc', 'progress_reporter.cc', 'string_splitter.cc'}),
    ('third_party/perfetto/src/base/perfetto_base_default_platform.ninja', {'default_platform.cc'}),
    ('third_party/perfetto/src/protozero/protozero.ninja',
     {'message.cc', 'scattered_stream_writer.cc', 'message_arena.cc', 'scattered_heap_buffer.cc'}),
    ('third_party/perfetto/src/tracing/client_api_without_backends.ninja',
     {'data_source.cc', 'event_context.cc', 'traced_value.cc', 'tracing_muxer_impl.cc', 'track.cc',
      'track_event_internal.cc', 'platform.cc', 'tracing_muxer_fake.cc', 'track_event_interned_fields.cc'}),
    ('mojo/public/cpp/bindings/bindings.ninja', {'interface_ptr_state.cc'}),
    ('mojo/public/cpp/bindings/bindings_base.ninja', {'associated_group.cc', 'pending_remote_state.cc'}),
    ('third_party/boringssl/boringssl.ninja', {'sha1.cc', 'md5.cc'}),
]:
    args, sources = read_target(target)
    jobs.extend((target, args, source) for source in sources if pathlib.Path(source).name in names)
for target in ('third_party/zlib/zlib.ninja', 'third_party/zlib/zlib_crc32_simd.ninja'):
    args, sources = read_target(target, 'c')
    jobs.extend((target, args, source) for source in sources if pathlib.Path(source).name == 'crc32.c' or target.endswith('_simd.ninja'))
args, _ = read_target(targets[0])
jobs.append(('harness', args, '/lane/oracle/harness/leveldb/harness.cc'))


def compile_one(job):
    target, args, source = job
    key = hashlib.sha256((target + '\0' + source).encode()).hexdigest()[:20]
    obj = scratch / (key + '.o')
    stamp = scratch / (key + '.stamp')
    signature = hashlib.sha256(('\0'.join(args) + '\0' + source).encode()).hexdigest()
    source_path = pathlib.Path(source) if source.startswith('/') else out / source
    if not obj.exists() or not stamp.exists() or stamp.read_text() != signature or obj.stat().st_mtime_ns < source_path.stat().st_mtime_ns:
        print('compile', target, source, flush=True)
        compiler = '/opt/chromium-llvm/bin/clang' if source.endswith('.c') else clang
        subprocess.run([compiler, *args, '-c', source, '-o', str(obj)], cwd=out, env=env, check=True)
        stamp.write_text(signature)
    return str(obj)


with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
    objects = list(executor.map(compile_one, jobs))
archive = scratch / 'chromium.a'
subprocess.run(['/opt/chromium-llvm/bin/llvm-ar', 'rcs', str(archive), *objects[:-1]], check=True)
subprocess.run([clang, '--target=x86_64-alpine-linux-musl', '-flto=thin', '-fuse-ld=lld', '-nostdlib++',
                '-Wl,--gc-sections,--error-limit=0,--no-demangle', '-pthread', objects[-1], str(archive),
                '/usr/lib/libgcc_s.so.1', '-o', str(scratch / 'oracle')],
               cwd=out, env=env, check=True)
