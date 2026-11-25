import os
import pathlib
import shlex
import subprocess
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait

out = pathlib.Path('/work/out/chromium-wayland-release')
scratch = pathlib.Path('/lane/.work/tmp/protobuf')
gen = scratch / 'gen'
gen.mkdir(parents=True, exist_ok=True)
env = dict(os.environ, TMPDIR=str(scratch))
protos = ['components/leveldb_proto/internal/proto/shared_db_metadata.proto', 'components/origin_trials/proto/db_trial_token.proto', 'content/browser/background_sync/background_sync.proto', 'content/browser/payments/payment_app.proto']
subprocess.run([str(out / 'protoc'), '--proto_path=/work/src', '--cpp_out=' + str(gen), *protos], cwd='/work/src', env=env, check=True)


def flags(path: pathlib.Path, kind: str) -> list[str]:
    values = {}
    for line in path.read_text().splitlines():
        for key in ('defines', 'include_dirs', 'cflags', 'cflags_c', 'cflags_cc', 'asmflags'):
            if line.startswith(key + ' = '):
                values[key] = shlex.split(line.split(' = ', 1)[1])
    keys = ('defines', 'include_dirs', 'asmflags') if kind == 'asm' else ('defines', 'include_dirs', 'cflags', 'cflags_cc' if kind == 'cxx' else 'cflags_c')
    return [arg for key in keys for arg in values.get(key, []) if not arg.startswith('-fcrash-diagnostics-dir=')]


sources = []
for ninja in [out / 'obj/third_party/protobuf/protobuf_lite_library.ninja', out / 'obj/third_party/protobuf/utf8_range.ninja', out / 'obj/buildtools/third_party/libunwind/libunwind.ninja']:
    for line in ninja.read_text().splitlines():
        for kind in ('cxx', 'cc', 'asm'):
            separator = f': {kind} '
            if line.startswith('build ') and separator in line:
                sources.append((ninja, line.split(separator, 1)[1].split()[0], kind))
link = next(line for line in (out / 'obj/chrome/chrome_initial.ninja').read_text().splitlines() if line.startswith('build ./chrome: link '))
wanted = {word for word in link.split() if word.startswith('obj/third_party/abseil-cpp/') and word.endswith('.o')}
for root in ('obj/third_party/abseil-cpp', 'obj/buildtools/third_party/libc++', 'obj/buildtools/third_party/libc++abi'):
    for ninja in (out / root).rglob('*.ninja'):
        for line in ninja.read_text().splitlines():
            if not line.startswith('build ') or ': cxx ' not in line:
                continue
            obj, source = line[6:].split(': cxx ', 1)
            if obj in wanted or root != 'obj/third_party/abseil-cpp' and ninja.name in ('libc++.ninja', 'libc++abi.ninja'):
                sources.append((ninja, source.split()[0], 'cxx'))
                wanted.discard(obj)
if wanted:
    raise SystemExit('unresolved ninja objects: ' + ' '.join(sorted(wanted)))
proto_ninjas = [out / 'obj/components/leveldb_proto/proto.ninja', out / 'obj/components/origin_trials/db_trial_token.ninja', out / 'obj/content/browser/background_sync/background_sync_proto.ninja', out / 'obj/content/browser/payments/payment_app_proto.ninja']
for ninja, proto in zip(proto_ninjas, protos):
    sources.append((ninja, str(gen / proto.replace('.proto', '.pb.cc')), 'cxx'))
sources.append((proto_ninjas[0], '/lane/oracle/harness/protobuf/harness.cc', 'cxx'))


def compile_source(item: tuple[int, tuple[pathlib.Path, str, str]]) -> str:
    index, (ninja, source, kind) = item
    obj = scratch / f'object-{index}.o'
    print(f'compile {index + 1}/{len(sources)} {source}', flush=True)
    compiler = '/opt/chromium-llvm/bin/clang++' if kind == 'cxx' else '/opt/chromium-llvm/bin/clang'
    subprocess.run([compiler, '-I' + str(gen), *flags(ninja, kind), '-c', source, '-o', str(obj)], cwd=out, env=env, check=True)
    return str(obj)


objects = [''] * len(sources)
remaining = iter(enumerate(sources))
with ThreadPoolExecutor(max_workers=2) as pool:
    pending = {}
    for _ in range(min(2, len(sources))):
        item = next(remaining)
        pending[pool.submit(compile_source, item)] = item[0]
    while pending:
        completed, _ = wait(pending, return_when=FIRST_COMPLETED)
        for future in completed:
            objects[pending.pop(future)] = future.result()
        for _ in completed:
            item = next(remaining, None)
            if item is not None:
                pending[pool.submit(compile_source, item)] = item[0]

partial = scratch / 'oracle.partial'
try:
    subprocess.run(['/opt/chromium-llvm/bin/clang++', '--target=x86_64-alpine-linux-musl', '-flto=thin', '-fuse-ld=lld', '-nostdlib++', '-Wl,--gc-sections', '-Wl,--threads=2', '-Wl,--thinlto-jobs=2', '-pthread', *objects, '-o', str(partial)], cwd=out, env=env, check=True)
    partial.replace(scratch / 'oracle')
finally:
    partial.unlink(missing_ok=True)
