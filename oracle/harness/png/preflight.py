import os
import pathlib
import shutil
import subprocess

source = pathlib.Path('/work/out/chromium-wayland-release')
root = pathlib.Path('/lane/.work/tmp/png')
build = root / 'out/build'
build.mkdir(parents=True, exist_ok=True)
if not (root / 'src').exists():
    (root / 'src').symlink_to('/work/src', target_is_directory=True)
opt = pathlib.Path('/lane/.work/tmp/opt')
if not opt.exists():
    opt.symlink_to('/opt', target_is_directory=True)
for name in ('build.ninja', 'build.ninja.stamp', 'build.ninja.d', 'toolchain.ninja', 'args.gn'):
    shutil.copy2(source / name, build / name)
empty = pathlib.Path('local_rustc_sysroot/lib/rustlib/x86_64-unknown-linux-musl/lib/.empty')
(build / empty).parent.mkdir(parents=True, exist_ok=True)
shutil.copy2(source / empty, build / empty)
if not (build / 'gen').exists():
    shutil.copytree(source / 'gen', build / 'gen', symlinks=True)
for tree in ('obj', 'default_for_rust_host_build_tools', 'default_host_with_system_allocator', 'default_with_system_allocator'):
    for directory, _, files in os.walk(source / tree):
        for filename in files:
            if filename.endswith('.ninja') or filename.endswith('_expected_outputs.txt'):
                path = pathlib.Path(directory) / filename
                target = build / path.relative_to(source)
                target.parent.mkdir(parents=True, exist_ok=True)
                if not target.exists() or path.stat().st_mtime_ns != target.stat().st_mtime_ns:
                    shutil.copy2(path, target)
targets = [
    'obj/third_party/blink/renderer/platform/image-decoders/image_decoders/png_image_decoder.o',
    'obj/skia/skia/SkPngRustCodec.o',
    'obj/skia/skia/SkPngRustEncoder.o',
    'obj/skia/librust_png_ffi_e866329c.rlib',
    'obj/ui/gfx/codec/codec/png_codec.o',
]
with (root / 'ninja-preflight.log').open('w') as log:
    subprocess.run(['ninja', '-C', str(build), '-n', *targets], stdout=log, stderr=subprocess.STDOUT, check=True)
print('ninja preflight complete')
