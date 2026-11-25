import json

import checks
import durability
import launch
import trace


def outcome(ok, evidence):
    return {'status':'ok' if ok else 'oracle-differs', 'evidence':evidence}


def run(directory, browser_root, image, preferences, endpoint="http://127.0.0.1:9221"):
    directory.mkdir(parents=True)
    session = launch.Session(directory, browser_root, f'manganese-client-sdk-{directory.parent.name}')
    workload = launch.ROOT / 'oracle/workloads/clients/shadowdriver'
    mounts = {directory:'/evidence', session.profile:'/profile', session.downloads:'/downloads', workload:'/workload:ro'}
    try:
        trace.start(session, session.run_dir, endpoint, directory.name)
        preferences(session.profile, True)
        session.sidecar(['python3', '-B', '/launch/contract/fixture.py'], {directory:'/evidence'}, detach=True)
        browser = durability.start(session)
        observed = json.loads(session.sidecar(trace.command(endpoint, ['python', '-B', '/workload/workload.py']), mounts, image=image, timeout=120))
        seed = json.loads(session.sidecar(trace.command(endpoint, ['python', '-B', '/workload/profile.py', 'seed']), mounts, image=image, timeout=60))
        durability.close(session, browser)
        browser = durability.start(session)
        read = json.loads(session.sidecar(trace.command(endpoint, ['python', '-B', '/workload/profile.py', 'read']), mounts, image=image, timeout=60))
        durability.close(session, browser)
    finally:
        session.close()
    target = observed['frame_targets']['targetInfos']
    upload = observed['uploads']
    sessions = observed['sessions']
    if (observed['download_clicks'] != 1 or observed['download_hit'] != 'blob'
            or observed['download_click_detail']['errors']):
        raise RuntimeError(f'blob download fixture failed: {observed["download_click_detail"]}')
    if not observed['cross_clicked'] or not upload:
        raise RuntimeError(f'the fixture did not record the cross-origin click or upload: {observed["frame_hit"]}')
    cookies = [seed['cookies']['cookies'], read['cookies']['cookies']]
    cookie_ok = all(any(item['name'] == 'contract_profile' and item['value'] == 'persisted' and item['httpOnly']
                        for item in items) for items in cookies)
    result = {
        'discovery':outcome(observed['version']['product'] == checks.VERSION['Browser'] and bool(observed['targets']['targetInfos']),
                            {'version':observed['version'], 'targets':observed['targets']}),
        'forms':outcome(observed['form_submission'], {'submitted':observed['form_submission']}),
        'accessibility':outcome(observed['ax_nodes'] > 0, {'ax_nodes':observed['ax_nodes'], 'snapshot':'SDK exposes no separate snapshot API'}),
        'screenshot':outcome(observed['png_signature'] == '89504e470d0a1a0a' and observed['png_bytes'] > 8,
                             {'signature':observed['png_signature'], 'bytes':observed['png_bytes']}),
        'dialogs':outcome(observed['confirm'] == 'true' and bool(observed['dialogs']),
                          {'confirm':observed['confirm'], 'events':observed['dialogs']}),
        'popups':outcome(any(item.get('url') == 'http://forms.test/popup' for item in observed['after_popup']['targetInfos']),
                         {'before':observed['before_popup'], 'after':observed['after_popup']}),
        'frames':outcome(observed['same_frame'] == 'same-origin frame' and observed['cross_clicked']
                         and any(item.get('type') == 'iframe' and item.get('url') == 'http://frame.test/cross' for item in target),
                         {'same':observed['same_frame'], 'cross_clicked':observed['cross_clicked'], 'targets':target}),
        'page-js':outcome(observed['user_agent'] == checks.VERSION['User-Agent'], {'user_agent':observed['user_agent']}),
        'sessions':outcome(sessions['cycles'] == 10 and not sessions['sockets_before'] and not sessions['sockets_after'], sessions),
        'files':outcome(any(item['length'] == 150000 and item['sha256'] == observed['upload_expected_sha256'] for item in upload)
                        and observed['download'] == 'client download',
                        {'uploads':upload, 'expected_sha256':observed['upload_expected_sha256'],
                         'download':observed['download'], 'events':observed['download_events']}),
        'profiles':outcome(cookie_ok, {'cookies':cookies}),
    }
    return result
