import hashlib
import os
import pathlib
import re
import shlex
import subprocess
import tempfile

out = pathlib.Path('/work/out/chromium-wayland-release')
scratch = pathlib.Path('/lane/.work/tmp/jpeg-decode')
sources = pathlib.Path('/lane/oracle/harness/jpeg-decode')
scratch.mkdir(parents=True, exist_ok=True)
env = dict(os.environ, TMPDIR=str(scratch))
clang = '/opt/chromium-llvm/bin/clang'
clangxx = '/opt/chromium-llvm/bin/clang++'


def compile_edges(ninja):
    values = {}
    edges = {}
    for line in (out / ninja).read_text(errors='replace').splitlines():
        for key in ('defines', 'include_dirs', 'cflags', 'cflags_c', 'cflags_cc'):
            if line.startswith(key + ' = '):
                values[key] = shlex.split(line.split(' = ', 1)[1])
        match = re.match(r'build (obj/\S+\.o): (cc|cxx) (\S+)', line)
        if match:
            obj, language, source = match.groups()
            keys = ('defines', 'include_dirs', 'cflags', 'cflags_' + ('c' if language == 'cc' else 'cc'))
            flags = [arg for key in keys for arg in values.get(key, []) if not arg.startswith('-fcrash-diagnostics-dir=')]
            edges[obj] = (source, flags, language)
    return edges


def compile_object(source, flags, language):
    content = pathlib.Path(source).read_bytes()
    digest = hashlib.sha256(('\0'.join(flags)).encode() + b'\0' + content).hexdigest()
    target = scratch / (digest + '.o')
    if not target.exists():
        print('compile', source, flush=True)
        with tempfile.NamedTemporaryFile(prefix=digest + '-', suffix='.o', dir=scratch, delete=False) as temporary:
            staged = pathlib.Path(temporary.name)
        try:
            subprocess.run([clangxx if language == 'cxx' else clang, *flags, '-c', source, '-o', str(staged)], cwd=out, env=env, check=True)
            os.replace(staged, target)
        finally:
            staged.unlink(missing_ok=True)
    return str(target)


decoders = compile_edges('obj/third_party/blink/renderer/platform/image-decoders/image_decoders.ninja')
_, harness_flags, _ = decoders['obj/third_party/blink/renderer/platform/image-decoders/image_decoders/jpeg_image_decoder.o']
harness_flags = [flag for flag in harness_flags if flag != '-DBLINK_PLATFORM_IMPLEMENTATION=1'] + ['-DINSIDE_BLINK']
harness = compile_object(str(sources / 'harness.cc'), harness_flags, 'cxx')
libjpeg = compile_edges('obj/third_party/libjpeg_turbo/libjpeg.ninja')
_, encoder_flags, _ = libjpeg['obj/third_party/libjpeg_turbo/libjpeg/jdhuff.o']

link_vars = {}
link = None
lines = (out / 'obj/chrome/chrome_initial.ninja').read_text(errors='replace').splitlines()
for index, line in enumerate(lines):
    if line.startswith('build ./chrome: link '):
        link = line
        for variable in lines[index + 1:]:
            if not variable.startswith('  '):
                break
            name, value = variable.split('=', 1)
            link_vars[name.strip()] = value.strip()
        break
if link is None:
    raise SystemExit('no chrome link edge')
explicit = link.split(' link ', 1)[1].split(' | ', 1)[0].split()
chrome_objects = [path for path in explicit if path.endswith('.o')]
archives = [path for path in explicit if path.endswith('.a')]
dropped = ('-Wl,--version-script=', '-Wl,--thinlto-cache-dir=', '-Wl,--thinlto-cache-policy=', '-Wl,--icf=', '-rdynamic', '-Wl,-mllvm,-enable-ext-tsp-block-placement')
ldflags = [flag for flag in shlex.split(link_vars['ldflags']) if not flag.startswith(dropped)]
ldflags += ['-Wl,--thinlto-cache-dir=' + str(scratch / 'thinlto-cache')]
host_absent = ('-lnspr4', '-lnss3', '-lnssutil3', '-lplc4', '-lplds4', '-lsmime3')
libs = [flag for flag in shlex.split(link_vars.get('libs', '')) if flag not in host_absent]


def link_harness(extra):
    command = [clangxx, *ldflags, '-Wl,--error-limit=0', '-o', str(scratch / 'oracle'), harness, *extra,
               '-Wl,--start-lib', *chrome_objects, '-Wl,--end-lib',
               '-Wl,--start-group', *archives, '-Wl,--end-group',
               '-Wl,--start-group', *shlex.split(link_vars.get('rlibs', '')), '-Wl,--end-group', *libs]
    return subprocess.run(command, cwd=out, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


print('link harness with', len(chrome_objects), 'lazy chrome objects and', len(archives), 'archives', flush=True)
result = link_harness([])
missing = sorted(set(re.findall(r'undefined symbol: (\S+)', result.stderr)))
if result.returncode and missing and all(re.match(r'(CERT|PK11|SECITEM|SECMOD|NSS|PR|PORT|SEC)_', symbol) for symbol in missing):
    stubs = scratch / 'nss-stubs.c'
    stubs.write_text('#include <stdlib.h>\n' + ''.join(f'void {symbol}(void) {{ abort(); }}\n' for symbol in missing))
    subprocess.run([clang, '--target=x86_64-alpine-linux-musl', '-fPIC', '-O2', '-c', str(stubs), '-o', str(scratch / 'nss-stubs.o')], cwd=out, env=env, check=True)
    print('stubbed', len(missing), 'NSS symbols that abort when called', flush=True)
    result = link_harness([str(scratch / 'nss-stubs.o')])
if result.returncode:
    print(result.stderr[-20000:], flush=True)
    raise SystemExit(result.returncode)
print('built', scratch / 'oracle', flush=True)

plain_dropped = ('-fsanitize', '-flto', '-fsplit-lto-unit', '-fwhole-program-vtables', '-fprofile-use=', '-DWITH_SIMD')
plain = []
for ninja in ('libjpeg', 'libjpeg12', 'libjpeg16'):
    for source, flags, language in compile_edges(f'obj/third_party/libjpeg_turbo/{ninja}.ninja').values():
        flags = [flag for flag in flags if not flag.startswith(plain_dropped)]
        plain.append(compile_object(str(out / source), flags, language))
encoder_flags = [flag for flag in encoder_flags if not flag.startswith(plain_dropped)]
encoder = compile_object(str(sources / 'encoder.c'), encoder_flags, 'cc')
subprocess.run([clang, '--target=x86_64-alpine-linux-musl', '-fuse-ld=lld', '-pie', '-o', str(scratch / 'encoder'), encoder, *plain],
               cwd=out, env=env, check=True)
print('built', scratch / 'encoder', flush=True)

fixture_groups = {
    'libjpeg-turbo': 'third_party/libjpeg_turbo/testimages',
    'blink-decoders': 'third_party/blink/renderer/platform/image-decoders/testing',
    'blink-data': 'third_party/blink/renderer/platform/testing/data',
    'blink-core': 'third_party/blink/renderer/core/testing/data',
    'skia': 'third_party/skia/resources/images',
    'skia-orientation': 'third_party/skia/resources/images/orientation',
    'skia-invalid': 'third_party/skia/resources/invalid_images',
    'skia-empty': 'third_party/skia/resources/empty_images',
    'go': 'third_party/webpagereplay/third_party/golang/linux/x64/src/image/testdata',
    'tflite': 'third_party/tflite/src/tensorflow/core/lib/jpeg/testdata',
    'libyuv': 'third_party/libyuv/unit_test/testdata',
    'content': 'content/test/data',
    'crabbyavif': 'third_party/crabbyavif/src/tests/data',
}
fixtures = scratch / 'fixtures'
for group, directory in fixture_groups.items():
    target = fixtures / group
    target.mkdir(parents=True, exist_ok=True)
    for path in sorted((pathlib.Path('/work/src') / directory).iterdir()):
        if path.is_file() and path.suffix.lower() in ('.jpg', '.jpeg'):
            (target / path.name).write_bytes(path.read_bytes())
print('copied', sum(1 for _ in fixtures.rglob('*.*')), 'fixtures', flush=True)
