import pathlib
import sys
import tempfile
import types


class Client:
    def __init__(self, primary, disable_failure):
        self.primary = primary
        self.disable_failure = disable_failure

    def page(self):
        return 'page'

    def send(self, method, *args, session=None):
        if method == 'Fetch.disable' and self.disable_failure is not None:
            raise self.disable_failure

    def evaluate(self, page, expression):
        if self.primary is not None:
            raise self.primary

    def wait_event(self, name, session=None):
        return {'requestId': '1'}


fake_cdp = types.ModuleType('cdp')
fake_cdp.Cdp = Client
sys.modules['cdp'] = fake_cdp
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import sidecar


class Browser:
    def poll(self):
        return 0

    def kill(self):
        pass

    def wait(self, timeout=None):
        return 0


class Session:
    def __init__(self, primary, close_failure):
        self.primary = primary
        self.close_failure = close_failure
        self.profile = pathlib.Path(tempfile.mkdtemp(prefix='profile-handoff-result-'))

    def start_browser(self, mode):
        return None, Browser()

    def sidecar(self, *args, **kwargs):
        if self.primary is not None:
            raise self.primary
        return '{}'

    def probe(self, argument):
        return {'shutdown': {'ok': True}}

    def close(self):
        if self.close_failure is not None:
            raise self.close_failure


run_root = pathlib.Path(tempfile.mkdtemp(prefix='profile-handoff-run-'))
fake_launch = types.ModuleType('launch')
fake_launch.new_run_dir = lambda label, mode: run_root
fake_launch.resolve_archive = lambda *args: 'archive'
fake_launch.open_session = lambda run_dir, archive: Session(None, None)
sys.modules['launch'] = fake_launch
import session


def expect_raised(call, expected):
    try:
        call()
    except type(expected) as error:
        if error is not expected:
            raise SystemExit(f'expected the original {expected!r}, got {error!r}')
    else:
        raise SystemExit(f'{expected!r} was silently accepted')


def main():
    failure = RuntimeError('teardown failed')
    expect_raised(lambda: sidecar.page_at(Client(None, failure)), failure)
    primary = ValueError('primary failed')
    expect_raised(lambda: sidecar.page_at(Client(primary, failure)), primary)

    profile = pathlib.Path(tempfile.mkdtemp(prefix='profile-handoff-profile-'))
    original_argv = sys.argv
    try:
        sys.argv = ['session.py', 'read', str(profile)]
        session_failure = RuntimeError('session teardown failed')
        fake_launch.open_session = lambda run_dir, archive: Session(None, session_failure)
        expect_raised(session.main, session_failure)
        fake_launch.open_session = lambda run_dir, archive: Session(primary, session_failure)
        expect_raised(session.main, primary)
    finally:
        sys.argv = original_argv

    print('profile-handoff selftest: cleanup promotion and primary preservation ok')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
