import concurrent.futures
from collections.abc import Callable
from pathlib import Path

import checks
import client_agent
import client_browser_use
import client_capture
import client_images
import client_sdk
import extensions
import launch


def run(run_dir: Path, archive: launch.Archive, preferences: Callable[[Path, bool], object],
        selected: set[str]) -> dict[str, dict[str, object]]:
    directory = run_dir / 'clients'
    directory.mkdir()
    browser_root, verified = launch.verify(archive, directory)
    if not verified['ok']:
        raise RuntimeError(f'client archive verification failed: {verified}')
    launch.ensure_probe_image()
    images_dir = directory / 'images'
    if '7.5' in selected:
        images = client_images.build_all(images_dir)
    else:
        images_dir.mkdir()
        images = {'extensions':client_images.build('extensions', images_dir)}
    jobs = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=7) as executor:
        def submit(name: str, function: Callable[..., object], *args: object) -> None:
            jobs[name] = executor.submit(function, directory / name, browser_root, *args)
        if '7.5' in selected:
            submit('browser-use-cli', client_browser_use.run, images['browser-use'], preferences, 'cli')
            submit('browser-use-library', client_browser_use.run, images['browser-use'], preferences, 'library')
            submit('agent-browser', client_agent.run, images['agent-browser'], preferences)
            submit('shadowdriver-sdk', client_sdk.run, images['shadowdriver'], preferences)
            submit('extension-cli', client_capture.run, images['extensions'], preferences, 'extension-cli')
            submit('shadowdriver-crawler', client_capture.run, images['shadowdriver'], preferences, 'shadowdriver-crawler')
        if '7.6' in selected:
            submit('extension-features', extensions.run, images['extensions'], preferences)
        observed = {}
        failures = {}
        for name, future in jobs.items():
            try:
                observed[name] = future.result()
            except Exception as error:
                failures[name] = repr(error)
        if failures:
            raise RuntimeError(f'client harness failures: {failures}')
    results = {}
    if '7.5' in selected:
        table = {name:observed[name] for name in checks.CLIENTS}
        results['7.5'] = {'ok':checks.client_workloads(table), 'clients':table}
    if '7.6' in selected:
        features = observed['extension-features']
        results['7.6'] = {'ok':checks.extension_features(features), 'features':features}
    return results
