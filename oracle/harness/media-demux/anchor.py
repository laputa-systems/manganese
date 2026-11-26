import base64
import json
import pathlib
import sys

import corpus

root = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(root / 'oracle/launch'))
sys.path.insert(0, str(root / 'oracle/harness/zlib'))
import launch
from compare import ask, peer

STATUS_NAMES = {12: 'DEMUXER_ERROR_COULD_NOT_OPEN', 13: 'DEMUXER_ERROR_COULD_NOT_PARSE', 14: 'DEMUXER_ERROR_NO_SUPPORTED_STREAMS'}
CASES = [
    'pcm-8-1-8000', 'pcm-16-2-44100', 'pcm-16-6-48000', 'pcm-24-1-16000', 'pcm-32-2-8000', 'pcm-odd-bits-12', 'pcm-odd-bits-20', 'pcm-odd-bits-4',
    'pcm-odd-bits-40', 'pcm-odd-bits-64', 'codec-f32', 'codec-f64', 'codec-alaw', 'codec-mulaw', 'codec-ul', 'codec-adpcm-ms', 'codec-unknown',
    'codec-zero', 'extensible-6-60f', 'extensible-12-2d63f', 'extensible-6-3', 'extensible-float', 'extensible-ambisonic', 'extensible-unknown-guid',
    'layout-fact-wrong', 'layout-data-size-large', 'layout-data-size-small', 'layout-data-size-zero', 'layout-rifx', 'layout-rf64', 'layout-bw64',
    'layout-no-fmt', 'layout-no-data', 'layout-rate-low', 'layout-rate-high', 'layout-channels-33', 'layout-block-align-huge', 'layout-f24-in-s24',
    'layout-id3-prefix-odd', 'layout-id3-v4-footer', 'truncated-36', 'truncated-45', 'truncated-46', 'truncated-1001', 'other-text', 'other-random',
    'other-zeros', 'other-wave-no-riff', 'long-mono', 'fixture-startup-wav', 'fixture-audio_long8mulaw-wav', 'fixture-audio_tiny44-wav',
    'fixture-empty-wav', 'fixture-keypressreturn-wav', 'fixture-irc_composite_c_r0195_t000_p000-wav',
]


def expected_of(text):
    lines = text.splitlines()
    fields = dict(line.split(' ', 1) for line in lines[1:] if line.startswith(('open ', 'duration ', 'final ')))
    status = int(fields['open'])
    errors = [int(line.split()[1]) for line in lines if line.startswith('error ')]
    seconds = lambda value: 'inf' if value == 'inf' else int(value) / 1e6
    if status:
        return {'status': status}
    return {'status': 0, 'duration': seconds(fields['duration']), 'final': seconds(fields['final']), 'errors': errors}


def main():
    items = dict(corpus.lane_items('seen'))
    items.update((name, data) for name, data in corpus.lane_items('holdout') if name not in items)
    selected = {}
    for prefix in CASES:
        matches = sorted(name for name in items if name == prefix or name.startswith(prefix + '-'))
        if not matches:
            raise SystemExit(f'anchor case {prefix} is not in the corpus')
        selected[prefix] = items[matches[0]]
    oracle = peer(str(root / '.work/tmp/media-demux/oracle'))
    expected = {}
    for name, data in selected.items():
        output = ask(oracle, data).decode()
        if not output.startswith('probe wav\n') and not output.startswith('probe none\n'):
            raise SystemExit(f'anchor case {name} probes outside the box scope: {output.splitlines()[0]}')
        expected[name] = expected_of(output)
    oracle.stdin.close()
    if oracle.wait():
        raise SystemExit('oracle failed')
    run_dir = launch.new_run_dir('media-demux-anchor', 'headless')
    cases_dir = run_dir / 'cases'
    cases_dir.mkdir()
    (cases_dir / 'cases.json').write_text(json.dumps({name: base64.b64encode(data).decode() for name, data in selected.items()}))
    archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
    session = launch.open_session(run_dir, archive)
    browser = None
    try:
        _, browser = session.start_browser('headless')
        observed = json.loads(session.sidecar(['python3', '-B', '/media/anchor-sidecar.py'],
                                              {root / 'oracle/harness/media-demux': '/media:ro', cases_dir: '/cases:ro'}, timeout=900))
        (run_dir / 'observed.json').write_text(json.dumps(observed, indent=1, sort_keys=True))
        failures = []
        for name, want in expected.items():
            got = observed[name]
            if want['status']:
                agrees = got.get('code') == 4 and str(got.get('message', '')).startswith('PipelineStatus::' + STATUS_NAMES[want['status']] + ':')
            else:
                agrees = got.get('code') is None and got.get('duration') == want['duration'] and got.get('final') == want['final']
                if want['errors']:
                    agrees = got.get('duration') == want['duration'] and got.get('ended_code') == 4
                elif got.get('ended_code') is not None:
                    agrees = False
            if not agrees:
                failures.append(f'{name}: browser {json.dumps(got)} harness {json.dumps(want)}')
        if failures:
            raise SystemExit('anchor mismatches:\n' + '\n'.join(failures))
        print(f'anchor {len(expected)} cases agree: {sum(1 for w in expected.values() if w["status"] == 0)} opened, '
              f'{sum(1 for w in expected.values() if w["status"])} rejected, {sum(1 for w in expected.values() if w.get("errors"))} with demuxer errors')
        shutdown = session.probe('--close')
        if not shutdown.get('shutdown', {}).get('ok') or browser.wait(timeout=15) != 0:
            raise SystemExit('browser shutdown failed')
    finally:
        if browser is not None and browser.poll() is None:
            browser.kill()
            browser.wait()
        session.close()


main()
