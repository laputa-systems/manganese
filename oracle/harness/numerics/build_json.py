import hashlib
import os
import pathlib
import subprocess
import tarfile
import urllib.request
import tomllib

scratch = pathlib.Path('/lane/.work/tmp/numerics/json-build')
scratch.mkdir(parents=True, exist_ok=True)
(scratch / 'src').mkdir(exist_ok=True)
(scratch / '.cargo').mkdir(exist_ok=True)
manifest = '''[package]
name = "json_oracle"
version = "0.1.0"
edition = "2021"
[workspace]
[lib]
crate-type = ["staticlib"]
[dependencies]
serde = { version = "=1.0.229", features = ["derive"] }
serde_json_lenient = { version = "=0.2.4", features = ["float_roundtrip", "unbounded_depth"] }
'''
std_lock = tomllib.loads(pathlib.Path('/work/src/third_party/rust-toolchain/lib/rustlib/src/rust/library/Cargo.lock').read_text())
overlay = scratch / 'vendor'
overlay.mkdir(exist_ok=True)
chromium_packages = {}
for path in pathlib.Path('/work/src/third_party/rust/chromium_crates_io/vendor').iterdir():
    if not (path / 'Cargo.toml').is_file(): continue
    package = tomllib.loads((path / 'Cargo.toml').read_text()).get('package', {})
    if isinstance(package.get('version'), str): chromium_packages[(package.get('name'), package['version'])] = path
    dest = overlay / path.name
    if not dest.exists(): dest.symlink_to(path, target_is_directory=True)
for package in std_lock['package']:
    if not package.get('source', '').startswith('registry'): continue
    name, version = package['name'], package['version']
    path = pathlib.Path('/registry') / (name + '-' + version)
    archive = pathlib.Path('/registry-cache') / (name + '-' + version + '.crate')
    if not path.is_dir() or not archive.is_file():
        cached = scratch / 'toolchain-crates'
        cached.mkdir(exist_ok=True)
        archive = cached / (name + '-' + version + '.crate')
        if not archive.exists():
            with urllib.request.urlopen(f'https://static.crates.io/crates/{name}/{name}-{version}.crate', timeout=60) as response:
                archive.write_bytes(response.read())
        if hashlib.sha256(archive.read_bytes()).hexdigest() != package['checksum']:
            raise RuntimeError(f'toolchain crate checksum mismatch: {name} {version}')
        path = cached / (name + '-' + version)
        if not path.exists():
            with tarfile.open(archive) as source:
                source.extractall(cached, filter='data')
    if hashlib.sha256(archive.read_bytes()).hexdigest() != package['checksum']:
        raise RuntimeError(f'cached crate checksum mismatch: {name} {version}')
    chromium_path = chromium_packages.get((name, version))
    if chromium_path is not None:
        for source in (chromium_path / 'src').rglob('*'):
            if source.is_file():
                original = path / source.relative_to(chromium_path)
                if name in ('memchr', 'itoa', 'ryu', 'serde', 'serde_core', 'serde_json_lenient') and (not original.is_file() or original.read_bytes() != source.read_bytes()):
                    raise RuntimeError(f'Chromium source differs from toolchain crate: {name} {source}')
        link = overlay / chromium_path.name
        if link.is_symlink(): link.unlink()
    dest = overlay / (name + '-' + version)
    dest.mkdir(exist_ok=True)
    for child in path.iterdir():
        if child.name in ('.cargo-checksum.json', '.cargo-ok'): continue
        link = dest / child.name
        if link.is_symlink() and link.readlink() != child: link.unlink()
        if not link.exists(): link.symlink_to(child, target_is_directory=child.is_dir())
    (dest / '.cargo-checksum.json').write_text('{"files":{},"package":"' + package['checksum'] + '"}')
(scratch / 'Cargo.toml').write_text(manifest)
(scratch / 'src/lib.rs').write_text(pathlib.Path('/lane/oracle/harness/numerics/json_oracle.rs').read_text())
(scratch / '.cargo/config.toml').write_text('''[source.crates-io]
replace-with = "chromium"
[source.chromium]
directory = "/lane/.work/tmp/numerics/json-build/vendor"
''')
env = dict(os.environ, CARGO_HOME=str(scratch / 'cargo-home'), CARGO_TARGET_DIR=str(scratch / 'target'), RUSTC='/work/src/third_party/rust-toolchain/bin/rustc', RUSTC_BOOTSTRAP='1', CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER='/lane/oracle/harness/numerics/host-linker', TMPDIR='/lane/.work/tmp/numerics')
subprocess.run(['/usr/bin/cargo', 'build', '--lib', '-j', '2', '--offline', '--target', 'x86_64-unknown-linux-musl', '-Z', 'build-std=std,panic_abort'], cwd=scratch, env=env, check=True)
