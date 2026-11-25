import os
import pathlib
import shlex
import subprocess

out = pathlib.Path('/work/out/chromium-wayland-release')
scratch = pathlib.Path('/lane/.work/tmp/protobuf')
gen = scratch / 'gen'
gen.mkdir(parents=True, exist_ok=True)
env = dict(os.environ, TMPDIR=str(scratch))
compiler = '/opt/chromium-llvm/bin/clang++'
protos = ['components/leveldb_proto/internal/proto/shared_db_metadata.proto', 'components/origin_trials/proto/db_trial_token.proto']
subprocess.run([str(out / 'protoc'), '--proto_path=/work/src', '--cpp_out=' + str(gen), *protos], cwd='/work/src', env=env, check=True)


def flags(path):
    values = {}
    for line in path.read_text().splitlines():
        for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc'):
            if line.startswith(key + ' = '):
                values[key] = shlex.split(line.split(' = ', 1)[1])
    return [arg for key in ('defines', 'include_dirs', 'cflags', 'cflags_cc') for arg in values.get(key, []) if not arg.startswith('-fcrash-diagnostics-dir=')]


sources = []
for ninja in [out / 'obj/third_party/protobuf/protobuf_lite_library.ninja']:
    for line in ninja.read_text().splitlines():
        if line.startswith('build ') and ': cxx ' in line:
            sources.append((ninja, line.split(': cxx ', 1)[1].split()[0]))
link = next(line for line in (out / 'obj/chrome/chrome_initial.ninja').read_text().splitlines() if line.startswith('build ./chrome: link '))
wanted = {word for word in link.split() if word.startswith('obj/third_party/abseil-cpp/') and word.endswith('.o')}
for root in ('obj/third_party/abseil-cpp', 'obj/buildtools/third_party/libc++', 'obj/buildtools/third_party/libc++abi'):
    for ninja in (out / root).rglob('*.ninja'):
        for line in ninja.read_text().splitlines():
            if not line.startswith('build ') or ': cxx ' not in line:
                continue
            obj, source = line[6:].split(': cxx ', 1)
            if obj in wanted or root != 'obj/third_party/abseil-cpp' and ninja.name in ('libc++.ninja', 'libc++abi.ninja'):
                sources.append((ninja, source.split()[0]))
                wanted.discard(obj)
if wanted:
    raise SystemExit('unresolved ninja objects: ' + ' '.join(sorted(wanted)))
proto_ninja = out / 'obj/components/leveldb_proto/proto.ninja'
for proto in protos:
    sources.append((proto_ninja, str(gen / proto.replace('.proto', '.pb.cc'))))
sources.append((proto_ninja, '/lane/oracle/harness/protobuf/harness.cc'))
objects = []
for index, (ninja, source) in enumerate(sources):
    obj = scratch / f'object-{index}.o'
    print(f'compile {index + 1}/{len(sources)} {source}', flush=True)
    subprocess.run([compiler, *flags(ninja), '-I' + str(gen), '-c', source, '-o', str(obj)], cwd=out, env=env, check=True)
    objects.append(str(obj))
subprocess.run([compiler, '--target=x86_64-alpine-linux-musl', '-flto=thin', '-fuse-ld=lld', '-Wl,--gc-sections', '-pthread', *objects, '-o', str(scratch / 'oracle')], cwd=out, env=env, check=True)
