import concurrent.futures
import hashlib
import os
import pathlib
import shlex
import shutil
import subprocess

out = pathlib.Path('/work/out/chromium-wayland-release')
scratch = pathlib.Path('/lane/.work/tmp/encoding')
scratch.mkdir(parents=True, exist_ok=True)
shutil.copyfile('/work/src/third_party/icu/common/icudtl.dat', scratch / 'icudtl.dat')
subprocess.run(['/opt/chromium-llvm/bin/llvm-objcopy', '-I', 'binary', '-O', 'elf64-x86-64', '-B', 'i386:x86-64', '--set-section-alignment', '.data=16', 'icudtl.dat', 'icudtl.o'], cwd=scratch, check=True)
env = dict(os.environ, TMPDIR=str(scratch), KACHE_DISABLED='1')
compiler = '/opt/chromium-llvm/bin/clang++'


def target(path, rule='cxx'):
    lines = (out / 'obj' / path).read_text().splitlines()
    values = {}
    for line in lines:
        for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc', 'cflags_c'):
            if line.startswith(key + ' = '):
                values[key] = shlex.split(line.split(' = ', 1)[1])
    args = [arg for key in ('defines', 'include_dirs', 'cflags', 'cflags_c' if rule == 'cc' else 'cflags_cc') for arg in values.get(key, [])
            if not arg.startswith('-fcrash-diagnostics-dir=')]
    marker = ': ' + rule + ' '
    sources = [line.split(marker, 1)[1].split()[0] for line in lines if line.startswith('build ') and marker in line]
    return args, sources


def compile_one(task):
    args, source, obj = task
    if obj.exists() and obj.stat().st_mtime_ns >= (out / source).stat().st_mtime_ns:
        return
    print('compile', source, flush=True)
    tool = '/opt/chromium-llvm/bin/clang' if source.endswith('.c') else compiler
    subprocess.run([tool, *args, '-c', source, '-o', str(obj)], cwd=out, env=env, check=True)


wtf_args, wtf = target('third_party/blink/renderer/platform/wtf/wtf.ninja')
icu_args, icu = target('third_party/icu/icuuc_private.ninja')
cxx_args, cxx = target('buildtools/third_party/libc++/libc++.ninja')
base_args, base = target('base/base.ninja')
groups = [('wtf', wtf_args, wtf), ('icu', icu_args, icu), ('libcxx', cxx_args, cxx), ('base', base_args, base)]
for prefix, path in [('alloc', 'base/allocator/partition_allocator/src/partition_alloc/allocator_core.ninja'), ('allocbase', 'base/allocator/partition_allocator/src/partition_alloc/allocator_base.ninja'), ('rawptr', 'base/allocator/partition_allocator/src/partition_alloc/raw_ptr.ninja'), ('simdutf', 'third_party/simdutf/simdutf.ninja'), ('double', 'base/third_party/double_conversion/double_conversion.ninja')]:
    args, sources = target(path)
    groups.append((prefix, args, sources))
for prefix, path in [('abi', 'buildtools/third_party/libc++abi/libc++abi.ninja'), ('perfetto', 'third_party/perfetto/src/tracing/client_api_without_backends.ninja'), ('protozero', 'third_party/perfetto/src/protozero/protozero.ninja'), ('perfbase', 'third_party/perfetto/src/base/base.ninja'), ('perfplatform', 'third_party/perfetto/src/base/perfetto_base_default_platform.ninja'), ('abslformat', 'third_party/abseil-cpp/absl/strings/str_format_internal.ninja'), ('abslstrings', 'third_party/abseil-cpp/absl/strings/strings.ninja'), ('abslinternal', 'third_party/abseil-cpp/absl/strings/internal.ninja'), ('crashcommon', 'third_party/crashpad/crashpad/client/common.ninja')]:
    args, sources = target(path)
    groups.append((prefix, args, sources))
args, sources = target('third_party/zlib/zlib.ninja', 'cc')
groups.append(('zlib', args, sources))
args, sources = target('third_party/zlib/zlib_crc32_simd.ninja', 'cc')
groups.append(('zlibsimd', args, sources))
for prefix, path, wanted in [('ssl', 'third_party/boringssl/boringssl.ninja', ('sha1.cc', 'bcm.cc', 'cpu_intel.cc', 'crypto.cc', 'mem.cc', 'md5.cc', 'thread_pthread.cc'))]:
    args, sources = target(path)
    groups.append((prefix, args, [source for source in sources if pathlib.Path(source).name in wanted]))
gen_args, generated = target('third_party/perfetto/protos/perfetto/trace/track_event/cpp.ninja')
generated_root = scratch / 'generated'
generated_root.mkdir(exist_ok=True)
protos = ['protos/perfetto/trace/track_event/' + pathlib.Path(source).name.replace('.gen.cc', '.proto') for source in generated]
if any(not (generated_root / proto.replace('.proto', '.gen.cc')).exists() or (generated_root / proto.replace('.proto', '.gen.cc')).stat().st_mtime_ns < (out / '../../src/third_party/perfetto' / proto).stat().st_mtime_ns for proto in protos):
    subprocess.run(['python3', '../../src/tools/protoc_wrapper/protoc_wrapper.py', *protos, '--protoc', './protoc', '--proto-in-dir', '../../src/third_party/perfetto/', '--plugin', 'cppgen_plugin', '--plugin-out-dir', str(generated_root), '--plugin-options', 'wrapper_namespace=gen', '--import-dir=../../src/third_party/protobuf/src'], cwd=out, env=env, check=True)
groups.append(('trackgen', ['-I' + str(generated_root), *gen_args], [str(generated_root / proto.replace('.proto', '.gen.cc')) for proto in protos]))
tasks = [(wtf_args, '/lane/oracle/harness/encoding/harness.cc', scratch / 'harness.o')]
asm_lines = (out / 'obj/third_party/boringssl/boringssl_asm.ninja').read_text().splitlines()
asm_values = {line.split(' = ', 1)[0]: shlex.split(line.split(' = ', 1)[1]) for line in asm_lines if line.startswith(('defines = ', 'include_dirs = ', 'asmflags = '))}
asm_args = [arg for key in ('defines', 'include_dirs', 'asmflags') for arg in asm_values[key] if not arg.startswith('-fcrash-diagnostics-dir=')]
asm_source = next(line.split(': asm ', 1)[1].split()[0] for line in asm_lines if line.startswith('build ') and ': asm ' in line and 'sha1-x86_64-linux.S' in line)
tasks.append((asm_args, asm_source, scratch / 'sha1-asm.o'))
md5_source = next(line.split(': asm ', 1)[1].split()[0] for line in asm_lines if line.startswith('build ') and ': asm ' in line and 'md5-x86_64-linux.S' in line)
tasks.append((asm_args, md5_source, scratch / 'md5-asm.o'))
crash_lines = (out / 'obj/third_party/crashpad/crashpad/client/common.ninja').read_text().splitlines()
crash_values = {line.split(' = ', 1)[0]: shlex.split(line.split(' = ', 1)[1]) for line in crash_lines if line.startswith(('defines = ', 'include_dirs = ', 'asmflags = '))}
crash_args = [arg for key in ('defines', 'include_dirs', 'asmflags') for arg in crash_values.get(key, []) if not arg.startswith('-fcrash-diagnostics-dir=')]
crash_source = next(line.split(': asm ', 1)[1].split()[0] for line in crash_lines if line.startswith('build ') and ': asm ' in line and 'crashpad_info_note.S' in line)
tasks.append((crash_args, crash_source, scratch / 'crashpad-note.o'))
for prefix, args, sources in groups:
    for source in sources:
        flag_digest = hashlib.sha256('\0'.join(args).encode()).hexdigest()[:16]
        obj = scratch / (prefix + '-' + flag_digest + '-' + source.replace('/', '_') + '.o')
        tasks.append((args, source, obj))
with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
    list(pool.map(compile_one, tasks))
objects = [str(task[2]) for task in tasks]
subprocess.run(['/opt/chromium-llvm/bin/clang', '--target=x86_64-alpine-linux-musl', '-flto=thin', '-fuse-ld=lld', '-Wl,--gc-sections', '-Wl,--error-limit=0', '-Wl,--thinlto-jobs=2', '-pthread', objects[0], str(scratch / 'icudtl.o'), '-Wl,--start-lib', *objects[1:], '-Wl,--end-lib', '/usr/lib/libstdc++.so.6', '/usr/lib/libgcc_s.so.1', '-o', str(scratch / 'oracle')], cwd=out, env=env, check=True)
