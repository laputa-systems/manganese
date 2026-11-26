import pathlib
import sys
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


fake = types.ModuleType('cdp')
fake.Cdp = Client
sys.modules['cdp'] = fake
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import sidecar


def main():
    failure = RuntimeError('teardown failed')
    try:
        sidecar.page_at(Client(None, failure))
    except RuntimeError as error:
        assert error is failure, 'a teardown-only failure must be promoted'
    else:
        raise SystemExit('a teardown-only failure was silently accepted')

    primary = ValueError('primary failed')
    try:
        sidecar.page_at(Client(primary, failure))
    except ValueError as error:
        assert error is primary, 'the primary failure must win'
    else:
        raise SystemExit('the primary failure was masked')

    print('profile-handoff selftest: cleanup promotion and primary preservation ok')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
