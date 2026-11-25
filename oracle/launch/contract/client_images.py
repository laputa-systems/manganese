import concurrent.futures
import subprocess
from pathlib import Path

import launch


PINS = {
    'browser-use': ('manganese-client-browser-use:070be981', None),
    'agent-browser': ('manganese-client-agent-browser:0.43.0', None),
    'extensions': ('manganese-client-extensions:5205b197', ('extensions', '5205b197073a61f9733a382f3ed1971d4b67e95a')),
    'shadowdriver': ('manganese-client-shadowdriver:ee03cdbd', ('shadowdriver', 'ee03cdbd33ef3d096ef41d855c8a670444baa4a5')),
}


def build(name, directory):
    image, source = PINS[name]
    context = directory / name
    context.mkdir(parents=True)
    if source:
        repository, revision = source
        exported = subprocess.run(['git', '-C', str(Path.home() / 'repos' / repository), 'archive', revision],
                                  capture_output=True, check=True)
        subprocess.run(['tar', '-x', '-C', str(context)], input=exported.stdout, check=True)
    dockerfile = launch.ROOT / 'oracle/workloads/clients' / name / 'Dockerfile'
    result = subprocess.run(['docker', 'build', '--progress=plain', '-t', image, '-f', str(dockerfile), str(context)],
                            capture_output=True, text=True)
    (directory / f'{name}-build.log').write_text(result.stdout + result.stderr)
    if result.returncode:
        raise RuntimeError(f'{name} image build failed ({result.returncode}); see {directory / (name + "-build.log")}')
    return image


def build_all(directory):
    directory.mkdir()
    with concurrent.futures.ThreadPoolExecutor(max_workers=len(PINS)) as executor:
        builds = {name:executor.submit(build, name, directory) for name in PINS}
        return {name:future.result() for name, future in builds.items()}
