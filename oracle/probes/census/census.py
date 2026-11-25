import collections
import hashlib
import json
import lzma
import os
import pathlib
import shlex
import subprocess
import tarfile

ROOT = pathlib.Path(__file__).resolve().parents[3]
SCRATCH = ROOT / '.work/tmp/census-gn'
RESULT = ROOT / 'oracle/frozen/census'
IMAGE = 'manganese-oracle-builder:156.0.8078.79-amd64-9535972e6a8a'
ARCHIVE = 'chromium-156.0.8078.79-r2-linux-x86_64-musl.tar.xz'
STORE = pathlib.Path(os.environ.get('MANGANESE_STORE', pathlib.Path.home() / '.local/share/manganese/store')) / 'sha256'


def command(args, **kwargs):
    return subprocess.run(args, cwd=ROOT, check=True, **kwargs)


def read_json(path):
    return json.loads(path.read_text())


def write_json(path, value):
    path.write_text(json.dumps(value, sort_keys=True, indent=2, ensure_ascii=True) + '\n')
    if path.stat().st_size >= 1024 * 1024:
        raise RuntimeError('summary exceeds 1 MiB: ' + str(path))


def rules():
    result = []
    for line in (ROOT / 'oracle/probes/census/rules.tsv').read_text().splitlines():
        columns = line.split('\t')
        if len(columns) != 6:
            raise RuntimeError('invalid rule: ' + line)
        result.append(columns)
    return result


def classify(label, table, info=None):
    for prefix, owner, consumer, kind, disposition, obligation in table:
        if label.startswith(prefix):
            if owner == '{family}':
                owner = 'third_party/' + label[len('//third_party/'):].split('/')[0].split(':')[0]
            if owner == '{package}':
                owner = 'builder package/' + label[len('builder/'):]
            if info and info.get('testonly'):
                kind = 'test-only'
            elif info and ('host_build_tools' in info.get('toolchain', '') or info.get('type') in ('rust_proc_macro', 'action', 'action_foreach', 'copy', 'generated_file')):
                kind = 'build-tool'
                disposition = 'retain-oracle-only'
            return {'owner': owner, 'consumer': consumer, 'class': kind, 'disposition': disposition, 'obligation': obligation}
    raise RuntimeError('unclassified label: ' + label)


def store(name, value):
    STORE.mkdir(parents=True, exist_ok=True)
    temporary = SCRATCH / (name + '.xz')
    with lzma.open(temporary, 'wt', encoding='utf-8', preset=9) as stream:
        json.dump(value, stream, sort_keys=True, separators=(',', ':'), ensure_ascii=True)
        stream.write('\n')
    digest = hashlib.sha256(temporary.read_bytes()).hexdigest()
    destination = STORE / digest
    if destination.exists() and hashlib.sha256(destination.read_bytes()).hexdigest() != digest:
        raise RuntimeError('corrupt artifact: ' + str(destination))
    if not destination.exists():
        destination.write_bytes(temporary.read_bytes())
    return f'{digest} {temporary.stat().st_size} {name}.xz'


def elf_inventory(directory):
    result = {}
    for path in sorted(directory.rglob('*')):
        if not path.is_file():
            continue
        with path.open('rb') as stream:
            if stream.read(4) != b'\x7fELF':
                continue
        dynamic = command(['llvm-readelf', '-d', str(path)], capture_output=True, text=True).stdout
        program = command(['llvm-readelf', '-l', str(path)], capture_output=True, text=True).stdout
        needed = sorted(line.split('[', 1)[1].split(']', 1)[0] for line in dynamic.splitlines() if '(NEEDED)' in line)
        interp = [line.split('interpreter:', 1)[1].strip().rstrip(']') for line in program.splitlines() if 'Requesting program interpreter:' in line]
        result[str(path.relative_to(directory))] = {'needed': needed, 'interpreter': interp[0] if interp else None}
    return result


def main():
    SCRATCH.mkdir(parents=True, exist_ok=True)
    RESULT.mkdir(parents=True, exist_ok=True)
    command(['docker', 'run', '--rm', '--network=none', '-v', 'manganese-oracle-work-amd64:/work:ro', '-v', str(SCRATCH) + ':/scratch', '-v', str(ROOT) + ':/repo:ro', '--tmpfs', '/work/src/chrome/build/pgo_profiles', '--entrypoint', 'python3', IMAGE, '/repo/oracle/probes/census/collect.py'])
    archive = command(['sh', 'scripts/artifact', 'fetch', ARCHIVE], capture_output=True, text=True).stdout.strip().splitlines()[-1]
    extraction = SCRATCH / 'archive'
    extraction.mkdir(exist_ok=True)
    with tarfile.open(archive, 'r:xz') as tar:
        tar.extractall(extraction, filter='data')
    browser = extraction / ARCHIVE.removesuffix('.tar.xz')
    elf = elf_inventory(browser)
    project = read_json(SCRATCH / 'out/project.json')
    targets = project['targets']
    table = rules()
    classification = {label: classify(label, table, target) for label, target in targets.items()}
    output_labels = collections.defaultdict(set)
    outputs = read_json(SCRATCH / 'out/ninja-outputs.json')
    for label, paths in outputs.items():
        if label in targets:
            for path in paths:
                output_labels[path.removeprefix('./')].add(label)
    for label, target in targets.items():
        for path in target.get('outputs', []):
            output_labels[path.removeprefix('/scratch/out/').removeprefix('./')].add(label)
        for paths in target.get('source_outputs', {}).values():
            for path in paths:
                output_labels[path.removeprefix('./')].add(label)
    generated = {}
    for path, owners in sorted(output_labels.items()):
        generated[path] = [{'target': owner, **(classification[owner] if 'host_build_tools' in targets[owner].get('toolchain', '') else classify(owner, table))} for owner in sorted(owners)]
    packages = read_json(SCRATCH / 'packages.json')
    manifest = read_json(ROOT / 'oracle/frozen/amd64-r2/package-manifest.json')
    staged = {}
    for entry in manifest['files']:
        path = entry['path']
        if path in packages:
            package = packages[path]
            if package['sha256'] != entry['sha256']:
                raise RuntimeError('builder file differs from frozen member: ' + path)
            record = {'source': package['path'], **classify('builder/' + package['package'], table)}
        elif path in ('chromium', 'chromium.desktop', 'fontconfig/fonts.conf'):
            record = {'source': 'oracle/build/scripts/' + ('package-runtime.py' if path != 'chromium.desktop' else 'stage-runtime.py'), **classify('oracle/packaging', table)}
        elif path == 'chromium.png':
            source = '//chrome/app/theme/chromium/linux/product_logo_256.png'
            record = {'source': source, **classify(source, table)}
            record['class'] = 'data'
        elif path in output_labels:
            owners = sorted(output_labels[path])
            record = {'source': '/work/out/chromium-wayland-release/' + path, 'producers': owners, **classify(owners[0], table)}
            if path not in elf:
                record['class'] = 'data'
        else:
            raise RuntimeError('unmapped staged member: ' + path)
        staged[path] = {**entry, **record}
    archive_prefix = ARCHIVE.removesuffix('.tar.xz') + '/'
    archive_files = set()
    for member in (ROOT / 'oracle/frozen/amd64-r2/archive-members.txt').read_text().splitlines():
        if not member.startswith(archive_prefix):
            raise RuntimeError('unexpected archive member: ' + member)
        path = member[len(archive_prefix):]
        if not path or path.endswith('/'):
            staged[path or '.'] = {'source': 'archive directory', **classify('oracle/packaging', table)}
        elif path == 'package-manifest.json':
            staged[path] = {'source': 'oracle/build/scripts/package-runtime.py', **classify('oracle/packaging', table)}
        elif path not in staged:
            raise RuntimeError('archive member absent from package manifest: ' + path)
        else:
            archive_files.add(path)
    if archive_files != {entry['path'] for entry in manifest['files']}:
        raise RuntimeError('package manifest and archive member list differ')
    edge = (SCRATCH / 'link-edge.txt').read_text().strip().split(': link ', 1)[1].split(' |', 1)[0]
    direct = shlex.split(edge)
    link_inputs = []
    for path in direct:
        owners = sorted(output_labels.get(path.removeprefix('./'), []))
        if not owners:
            raise RuntimeError('unmapped direct link input: ' + path)
        link_inputs.append({'path': path, 'owners': owners, 'classification': [classification[x] for x in owners]})
    command_line = (SCRATCH / 'commands.txt').read_text().strip()
    command_arguments = shlex.split(command_line)
    archives = {}
    for path in command_arguments:
        if not path.endswith(('.a', '.rlib')):
            continue
        owners = sorted(output_labels.get(path.removeprefix('./'), []))
        if owners:
            archives[path] = {'producers': owners, 'classification': [classification[x] for x in owners]}
        elif path.startswith('/opt/chromium-llvm/'):
            archives[path] = {'classification': [classify('toolchain/chromium-llvm', table)]}
        else:
            raise RuntimeError('unclassified link archive: ' + path)
    libraries = {flag: {**info, 'classification': classify('builder/' + info['package'], table)} for flag, info in read_json(SCRATCH / 'link-libraries.json').items()}
    transitive = {}
    for line in (SCRATCH / 'inputs.txt').read_text(errors='surrogateescape').splitlines():
        path = shlex.split(line)[0].removeprefix('./')
        owners = sorted(output_labels.get(path, []))
        if owners:
            record = {'producers': owners, 'classification': [classification[x] for x in owners]}
        elif path.startswith('../../work/src/'):
            record = {'source': '/work/src/' + path.removeprefix('../../work/src/'), 'classification': [classify('//' + path.removeprefix('../../work/src/'), table)]}
        elif path.startswith(('gen/', 'obj/')):
            record = {'source': 'GN generation', 'classification': [classify('//' + path.split('/', 1)[1], table)]}
        elif path.startswith('../../opt/chromium-llvm/'):
            record = {'source': '/' + path.removeprefix('../../'), 'classification': [classify('toolchain/chromium-llvm', table)]}
        else:
            raise RuntimeError('unclassified transitive input: ' + path)
        transitive[path] = record
    graph = {'build_settings': project['build_settings'], 'toolchains': project['toolchains'], 'targets': {label: {'classification': classification[label], **target} for label, target in sorted(targets.items())}, 'generated': generated}
    links = {'direct': link_inputs, 'command': command_line, 'flags': [argument for argument in command_arguments if argument.startswith('-')], 'archives': archives, 'system_libraries': libraries, 'transitive_inputs': transitive}
    families = collections.Counter(c['owner'] for c in classification.values())
    link_families = collections.Counter(item['classification'][0]['owner'] for item in link_inputs)
    reachable = set()
    pending = ['//chrome:chrome']
    while pending:
        label = pending.pop()
        if label not in reachable:
            if label not in targets:
                raise RuntimeError('missing reachable target: ' + label)
            reachable.add(label)
            pending.extend(targets[label].get('deps', []))
    gn_args = (SCRATCH / 'out/args.gn').read_text()
    summary = {'archive': ARCHIVE, 'archive_sha256': pathlib.Path(archive).name, 'builder_image': IMAGE, 'profile': 'chromium-wayland-release', 'architecture': 'x86_64', 'gn_args': gn_args, 'gn_args_sha256': hashlib.sha256(gn_args.encode()).hexdigest(), 'roots': read_json(SCRATCH / 'roots.json'), 'target_count': len(targets), 'chrome_reachable_target_count': len(reachable), 'rules': 'oracle/probes/census/rules.tsv', 'rules_sha256': hashlib.sha256((ROOT / 'oracle/probes/census/rules.tsv').read_bytes()).hexdigest(), 'class_counts': dict(sorted(collections.Counter(c['class'] for c in classification.values()).items())), 'disposition_counts': dict(sorted(collections.Counter(c['disposition'] for c in classification.values()).items())), 'owner_counts': dict(sorted(families.items())), 'link_input_owner_counts': dict(sorted(link_families.items())), 'third_party_families': len([x for x in families if x.startswith('third_party/')]), 'top_third_party': sorted(((k, v) for k, v in families.items() if k.startswith('third_party/')), key=lambda x: (-x[1], x[0]))[:30], 'toolchains': sorted({t['toolchain'] for t in targets.values()}), 'elf': elf}
    summary['reference_notes'] = ['The requirements document records PartitionAlloc as malloc with BackupRefPtr as the qualified security baseline.', 'Presence in the archive does not establish runtime use.']
    if 'libvk_swiftshader.so' in staged:
        summary['reference_notes'].append('The frozen amd64 archive contains libvk_swiftshader.so although the requirements reference-build asymmetry says it is excluded.')
    entries = [store('targets.json', graph), store('link.json', links)]
    write_json(RESULT / 'summary.json', summary)
    write_json(RESULT / 'staged.json', staged)
    (RESULT / 'store-manifest.txt').write_text('\n'.join(sorted(entries)) + '\n')
    print('census: targets', len(targets), 'staged', len(staged), 'ELF', len(elf), 'unclassified 0')


if __name__ == '__main__':
    main()
