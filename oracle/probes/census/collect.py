import hashlib
import json
import os
import pathlib
import shlex
import subprocess

ROOT = pathlib.Path('/work/src')
OUT = pathlib.Path('/scratch/out')
GN = str(ROOT / 'buildtools/linux64/gn')


def run(args, **kwargs):
    return subprocess.run(args, cwd=ROOT, check=True, **kwargs)


def package_for(path):
    result = run(['apk', 'info', '-W', str(path)], text=True, capture_output=True)
    marker = ' is owned by '
    if marker not in result.stdout:
        raise RuntimeError('no package owner: ' + str(path))
    return result.stdout.strip().split(marker, 1)[1]


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / 'args.gn').write_bytes(pathlib.Path('/work/out/chromium-wayland-release/args.gn').read_bytes())
    profile = pathlib.Path('/work/src/chrome/build/pgo_profiles')
    for name in pathlib.Path('/work/src/chrome/build/linux.pgo.txt').read_text().splitlines():
        if name.strip():
            (profile / name.strip()).touch()
    common = [GN, 'gen', str(OUT), '--root=/work/src', '--ide=json', '--filter-with-data', '--ninja-outputs-file=/scratch/out/ninja-outputs.json']
    run(common + ['--filters=//chrome:chrome;//sandbox/linux:chrome_sandbox'])
    outputs = json.loads((OUT / 'ninja-outputs.json').read_text())
    manifest = json.loads(pathlib.Path('/repo/oracle/frozen/amd64-r2/package-manifest.json').read_text())
    members = {f['path'] for f in manifest['files']}
    extra = sorted(label for label, paths in outputs.items() if any(p.removeprefix('./') in members for p in paths))
    roots = sorted(set(extra + ['//chrome:chrome', '//sandbox/linux:chrome_sandbox']))
    pathlib.Path('/scratch/roots.json').write_text(json.dumps(roots) + '\n')
    run(common + ['--filters=' + ';'.join(roots)])
    with subprocess.Popen(['ninja', '-C', str(OUT), '-t', 'commands', 'chrome'], cwd=ROOT, stdout=subprocess.PIPE, text=True, errors='replace') as process:
        last = None
        for last in process.stdout:
            pass
        if process.wait() or last is None:
            raise RuntimeError('ninja commands failed')
    pathlib.Path('/scratch/commands.txt').write_text(last)
    libraries = {}
    for flag in sorted({arg for arg in shlex.split(last) if arg.startswith('-l') and len(arg) > 2}):
        candidates = [pathlib.Path(directory) / ('lib' + flag[2:] + extension) for directory in ('/usr/lib', '/lib', '/opt/chromium-llvm/lib') for extension in ('.so', '.a')]
        path = next((candidate for candidate in candidates if candidate.is_file()), None)
        if path is None:
            raise RuntimeError('cannot resolve link library: ' + flag)
        libraries[flag] = {'path': str(path), 'package': package_for(path.resolve())}
    pathlib.Path('/scratch/link-libraries.json').write_text(json.dumps(libraries, sort_keys=True) + '\n')
    with open('/scratch/inputs.txt', 'w') as stream:
        run(['ninja', '-C', str(OUT), '-t', 'inputs', 'chrome'], stdout=stream)
    link = (OUT / 'obj/chrome/chrome_initial.ninja').read_text()
    lines = [line for line in link.splitlines() if line.startswith('build ./chrome: link ')]
    if len(lines) != 1:
        raise RuntimeError('cannot identify chrome link edge')
    pathlib.Path('/scratch/link-edge.txt').write_text(lines[0] + '\n')
    packages = {}
    for member in sorted(members):
        if member.startswith('lib/'):
            relative = member.removeprefix('lib/')
            candidates = [pathlib.Path('/usr/lib') / relative, pathlib.Path('/lib') / relative, pathlib.Path('/usr/lib/dri') / relative]
        elif member.startswith('fonts/noto/'):
            candidates = [pathlib.Path('/usr/share/fonts/noto') / member.removeprefix('fonts/noto/')]
        elif member.startswith('fontconfig/conf.d/'):
            candidates = [pathlib.Path('/etc/fonts/conf.d') / member.removeprefix('fontconfig/conf.d/')]
        else:
            continue
        path = next((p for p in candidates if p.is_file()), None)
        if path is None:
            raise RuntimeError('no builder file for ' + member)
        packages[member] = {'path': str(path), 'package': package_for(path), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
    pathlib.Path('/scratch/packages.json').write_text(json.dumps(packages, sort_keys=True) + '\n')


if __name__ == '__main__':
    main()
