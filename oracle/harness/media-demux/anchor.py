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
    'pcm-8-1-8000', 'pcm-16-2-44100', 'pcm-16-6-44100', 'pcm-24-1-16000', 'pcm-32-2-16000', 'pcm-odd-bits-12', 'pcm-odd-bits-20', 'pcm-odd-bits-4',
    'pcm-odd-bits-40', 'pcm-odd-bits-64', 'codec-f32', 'codec-f64', 'codec-alaw', 'codec-mulaw', 'codec-ul', 'codec-adpcm-ms', 'codec-unknown',
    'codec-zero', 'extensible-6-60f', 'extensible-12-2d63f', 'extensible-6-3', 'extensible-float', 'extensible-ambisonic', 'extensible-unknown-guid',
    'layout-fact-wrong', 'layout-data-size-large', 'layout-data-size-small', 'layout-data-size-zero', 'layout-rifx', 'layout-rf64', 'layout-bw64',
    'layout-no-fmt', 'layout-no-data', 'layout-rate-low', 'layout-rate-high', 'layout-channels-33', 'layout-block-align-huge', 'layout-f24-in-s24',
    'layout-id3-prefix-odd', 'layout-id3-v4-footer', 'truncated-36', 'truncated-46', 'truncated-1001', 'other-text', 'other-random',
    'other-zeros', 'other-wave-no-riff', 'long-mono', 'fixture-startup-wav', 'fixture-audio_long8mulaw-wav', 'fixture-audio_tiny44-wav',
    'fixture-empty-wav', 'fixture-keypressreturn-wav', 'fixture-irc_composite_c_r0195_t000_p000-wav',
    'flac-1ch-8-8000-576-4', 'flac-2ch-16-44100-4096-12', 'flac-6ch-16-48000-1024-15', 'flac-8ch-24-96000-2048-9', 'flac-2ch-32-192000-4096-10',
    'flac-stereo-mid-side', 'flac-variable', 'flac-frames-1', 'flac-frames-11', 'flac-rate-code-13', 'flac-layout-long-unknown-duration',
    'flac-info-samples-large', 'flac-info-bps-invalid', 'flac-meta-picture-png', 'flac-meta-picture-two', 'flac-meta-picture-type-invalid',
    'flac-meta-mask-stereo', 'flac-meta-mask-wide', 'flac-meta-cuesheet-short', 'flac-layout-trailing-junk', 'flac-layout-junk-between',
    'flac-layout-frame-crc-bad-many', 'flac-layout-mixed-rate-frames',
    'flac-layout-frame-crc-bad', 'flac-layout-long-duration-grows', 'flac-layout-two-streaminfo', 'flac-layout-long-low-rate', 'flac-layout-id3-prefix',
    'flac-truncated-1000', 'fixture-logo-flac', 'fixture-composite-flac', 'fixture-low_battery-flac', 'fixture-welcome_back-flac',
]
MIME = {'probe wav': 'audio/wav', 'probe none': 'audio/wav', 'probe flac': 'audio/flac'}


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
    mimes = {}
    for name, data in selected.items():
        output = ask(oracle, data).decode()
        probe = output.splitlines()[0]
        if probe not in MIME:
            raise SystemExit(f'anchor case {name} probes outside the box scope: {probe}')
        mimes[name] = MIME[probe]
        expected[name] = expected_of(output)
    oracle.stdin.close()
    if oracle.wait():
        raise SystemExit('oracle failed')
    run_dir = launch.new_run_dir('media-demux-anchor', 'headless')
    cases_dir = run_dir / 'cases'
    cases_dir.mkdir()
    (cases_dir / 'cases.json').write_text(json.dumps({name: {'mime': mimes[name], 'data': base64.b64encode(data).decode()} for name, data in selected.items()}))
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
