import json
import tempfile
import unittest
from pathlib import Path

import run


def write(directory, files):
    directory.mkdir(parents=True, exist_ok=True)
    for name, content in files.items():
        (directory / name).write_text(content if isinstance(content, str) else json.dumps(content))


def abi(syscalls, unstable=()):
    return {'x86_64.txt':'# header\n' + ''.join('syscall\t' + name + '\n' for name in syscalls),
            'x86_64-unstable.txt':''.join(line + '\n' for line in unstable)}


def cdp(methods=None, events=None, unstable=()):
    return {'methods.tsv':'method\tclients\tcount_class\n', 'events.tsv':'event\tclients\n', 'errors.tsv':'method\tcode\tmessage\n',
            'shapes.json':{'archive':{}, 'methods':methods or {}, 'events':events or {}},
            'unstable.tsv':''.join('\t'.join(item) + '\n' for item in unstable)}


EVALUATE = {'Runtime.evaluate':{'result':{'':['object'], 'result.value':['number']}}}


class NegativeControls(unittest.TestCase):
    def differences(self, compare, frozen, rebuilt):
        with tempfile.TemporaryDirectory() as root:
            write(Path(root) / 'frozen', frozen)
            write(Path(root) / 'rebuilt', rebuilt)
            result = []
            compare(Path(root) / 'frozen', Path(root) / 'rebuilt', result)
            return result

    def test_abi_identical_censuses_agree(self):
        self.assertEqual(self.differences(run.abi_differences, abi(['read', 'futex']), abi(['read', 'futex'])), [])

    def test_abi_rebuild_only_instability_does_not_excuse_a_new_syscall(self):
        result = self.differences(run.abi_differences, abi(['read', 'futex']),
                                  abi(['read', 'futex', 'io_uring_setup'], ['syscall\tio_uring_setup']))
        self.assertEqual({item['field'] for item in result}, {'abi', 'abi.unstable'})

    def test_abi_rebuild_only_instability_does_not_excuse_a_new_flag_combination(self):
        frozen = {'x86_64.txt':'flags\taccess:1=F_OK\n', 'x86_64-unstable.txt':''}
        rebuilt = {'x86_64.txt':'flags\taccess:1=F_OK\nflags\taccess:1=W_OK\n', 'x86_64-unstable.txt':'flags\taccess:1=W_OK\n'}
        result = self.differences(run.abi_differences, frozen, rebuilt)
        self.assertIn({'field':'abi', 'expected':'<missing>', 'actual':'flags\taccess:1=W_OK'}, result)

    def test_abi_frozen_instability_is_tolerated(self):
        frozen = abi(['read', 'futex', 'sched_yield'], ['syscall\tsched_yield'])
        self.assertEqual(self.differences(run.abi_differences, frozen, abi(['read', 'futex'])), [])

    def test_cdp_identical_shapes_agree(self):
        self.assertEqual(self.differences(run.cdp_differences, cdp(EVALUATE), cdp(EVALUATE)), [])

    def test_cdp_changed_field_type_fails(self):
        changed = {'Runtime.evaluate':{'result':{'':['object'], 'result.value':['string']}}}
        result = self.differences(run.cdp_differences, cdp(EVALUATE), cdp(changed))
        self.assertEqual([(item['item'][3], item['expected'], item['actual']) for item in result],
                         [('result.value', ['number'], ['string'])])

    def test_cdp_missing_and_unexpected_fields_fail(self):
        missing = {'Runtime.evaluate':{'result':{'':['object']}}}
        extra = {'Runtime.evaluate':{'result':{'':['object'], 'result.value':['number'], 'result.type':['string']}}}
        self.assertEqual(len(self.differences(run.cdp_differences, cdp(EVALUATE), cdp(missing))), 1)
        self.assertEqual(len(self.differences(run.cdp_differences, cdp(EVALUATE), cdp(extra))), 1)

    def test_cdp_method_and_event_of_one_name_do_not_collide(self):
        result = self.differences(run.cdp_differences, cdp(methods=EVALUATE), cdp(events=EVALUATE))
        self.assertEqual({item['item'][0] for item in result}, {'methods', 'events'})

    def test_cdp_rebuild_only_unstable_field_fails(self):
        extra = {'Runtime.evaluate':{'result':{'':['object'], 'result.value':['number'], 'exceptionDetails':['object']}}}
        result = self.differences(run.cdp_differences, cdp(EVALUATE),
                                  cdp(extra, unstable=[('shape', 'Runtime.evaluate', 'result', 'exceptionDetails')]))
        self.assertEqual({item['field'] for item in result}, {'cdp.shapes.json', 'cdp.unstable'})

    def test_cdp_frozen_unstable_field_is_tolerated(self):
        extra = {'Runtime.evaluate':{'result':{'':['object'], 'result.value':['number'], 'exceptionDetails':['object']}}}
        frozen = cdp(extra, unstable=[('shape', 'Runtime.evaluate', 'result', 'exceptionDetails')])
        self.assertEqual(self.differences(run.cdp_differences, frozen, cdp(EVALUATE)), [])


if __name__ == '__main__':
    unittest.main()
