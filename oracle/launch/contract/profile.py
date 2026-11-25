import json
import time

import checks
import durability
import launch


def run(run_dir, archive, preferences):
    directory = run_dir / 'profile-modes'
    directory.mkdir()
    session = launch.open_session(directory, archive)
    observations = []
    try:
        preferences(session.profile, True)
        session.sidecar(['python3', '-B', '/launch/contract/fixture.py'], {directory:'/evidence'}, detach=True)
        for index, mode in enumerate(('headed', 'headless', 'headed')):
            command, browser = session.start_browser(mode)
            readiness = session.probe()
            if not readiness['readiness']['ok']:
                raise RuntimeError(f'readiness failed: {readiness}')
            result = json.loads(session.sidecar(['python3', '-B', '/launch/contract/profile_client.py',
                                                 'seed' if index == 0 else 'read'], timeout=60))
            result.update({'mode':mode, 'command':command})
            if index == 0:
                download = session.downloads / 'contract-profile.txt'
                for _ in range(100):
                    if download.is_file():
                        break
                    time.sleep(0.1)
                result['download'] = download.read_bytes().decode() if download.is_file() else None
            durability.close(session, browser)
            observations.append(result)
    finally:
        session.close()
    return {'ok':checks.profile_modes(observations), 'observations':observations}
