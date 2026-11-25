import copy
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock
import checks
import contract


def entry(host, seconds=121):
    return {'host':host, 'version':5, 'command':1, 'address_type':3, 'seconds_after_readiness':seconds}


def evidence():
    out = []
    for enabled in (True, False):
        entries = [entry('forms.test'), entry('frame.test')]
        if not enabled:
            entries.append(entry(checks.AD))
        out.append({'enabled':enabled, 'entries':entries, 'preferences':{'contract':{'sentinel':'kept'}, 'adblock':{'other':'kept', 'enabled':enabled}}, 'local_state':{'exists':True, 'uid':1000}, 'netdev':'headers\n lo: 0', 'route':'header\n', 'fixture':{'frames':1, 'url':'http://forms.test/'}})
    return out


class NegativeControls(unittest.TestCase):
    def test_client_workloads(self):
        good = {name:{workload:{'status':'ok', 'evidence':{'observed':True}} for workload in checks.WORKLOADS} for name in checks.CLIENTS}
        self.assertTrue(checks.client_workloads(good))
        good['extension-cli']['dialogs'] = {'status':'n/a', 'reason':'no dialog API'}
        self.assertTrue(checks.client_workloads(good))
        for change in ('client', 'workload', 'failure', 'missing-evidence', 'missing-reason'):
            bad = copy.deepcopy(good)
            if change == 'client':
                del bad['browser-use']
            elif change == 'workload':
                del bad['agent-browser']['forms']
            elif change == 'failure':
                bad['agent-browser']['forms'] = {'status':'FAIL', 'evidence':{'observed':False}}
            elif change == 'missing-evidence':
                bad['agent-browser']['forms'] = {'status':'ok'}
            else:
                bad['extension-cli']['dialogs'] = {'status':'n/a'}
            self.assertFalse(checks.client_workloads(bad), change)

    def test_extension_features(self):
        good = {name:{'ok':True, 'evidence':{'observed':True}} for name in checks.EXTENSION_FEATURES}
        self.assertTrue(checks.extension_features(good))
        for change in ('missing', 'failed', 'no-evidence'):
            bad = copy.deepcopy(good)
            if change == 'missing':
                del bad['clipboardWrite']
            elif change == 'failed':
                bad['clipboardWrite']['ok'] = False
            else:
                del bad['clipboardWrite']['evidence']
            self.assertFalse(checks.extension_features(bad), change)

    def test_profile_modes(self):
        cookie = {'name':'contract_profile', 'value':'persisted', 'httpOnly':True}
        good = [{'mode':mode, 'storage':'persisted', 'cookies':[cookie]} for mode in ('headed', 'headless', 'headed')]
        good[0]['download'] = 'headed download evidence'
        self.assertTrue(checks.profile_modes(good))
        for index in range(3):
            for key, value in (('storage',None), ('cookies',[]), ('mode','wrong')):
                bad = copy.deepcopy(good)
                bad[index][key] = value
                self.assertFalse(checks.profile_modes(bad))
        bad = copy.deepcopy(good)
        bad[0]['download'] = 'wrong'
        self.assertFalse(checks.profile_modes(bad))

    def test_preferences_atomic_and_preserved(self):
        scratch = Path(__file__).resolve().parents[3] / '.work/tmp'
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            profile = Path(directory)
            with mock.patch.object(contract.os, 'replace', wraps=contract.os.replace) as replace:
                self.assertEqual(contract.preferences(profile, True), {'adblock':{'enabled':True}})
                self.assertEqual(replace.call_count, 1)
            pref = profile / 'Default' / 'Preferences'
            pref.write_text(json.dumps({'other':7, 'adblock':{'other':8, 'enabled':True}}))
            self.assertEqual(contract.preferences(profile, False), {'other':7, 'adblock':{'other':8, 'enabled':False}})
            self.assertEqual(json.loads(pref.read_text())['other'], 7)

    def test_profile_and_egress(self):
        for clause in ('5.1','5.2','6.1','6.2','6.3','6.4'):
            good = evidence()
            self.assertTrue(checks.egress(clause, good), clause)
            bad = copy.deepcopy(good)
            if clause == '5.1':
                bad[0]['preferences']['contract']['sentinel'] = 'lost'
            elif clause == '5.2':
                bad[0]['local_state']['uid'] = 0
            elif clause == '6.1':
                bad[0]['entries'][0]['address_type'] = 1
            elif clause == '6.2':
                bad[0]['entries'][0]['seconds_after_readiness'] = 1
            elif clause == '6.3':
                bad[0]['entries'].append(entry('other.test'))
            else:
                bad[0]['entries'].append(entry(checks.AD))
            self.assertFalse(checks.egress(clause, bad), clause)

    def test_endpoints(self):
        self.assertTrue(checks.endpoints([1], 'ws://localhost:9221/devtools/browser/id', True, True, True))
        for args in (([], 'ws://localhost:9221/devtools/browser/id', True, True, True), ([1], 'ws://wrong/devtools/browser/id', True, True, True), ([1], 'ws://localhost:9221/devtools/browser/id', False, True, True), ([1], 'ws://localhost:9221/devtools/browser/id', True, False, True), ([1], 'ws://localhost:9221/devtools/browser/id', True, True, False)):
            self.assertFalse(checks.endpoints(*args))

    def test_version(self):
        self.assertTrue(checks.version(checks.VERSION))
        for key in checks.VERSION:
            bad = dict(checks.VERSION)
            bad[key] = 'wrong'
            self.assertFalse(checks.version(bad))

    def test_robustness(self):
        self.assertTrue(checks.robust(33554432,33554432,2,2,1,1))
        for args in ((1,33554432,2,2,1,1),(33554432,1,2,2,1,1),(33554432,33554432,2,3,1,1),(33554432,33554432,2,2,1,2)):
            self.assertFalse(checks.robust(*args))
        self.assertTrue(checks.protocol(True, False, True))
        self.assertFalse(checks.protocol(False, False, True))
        self.assertFalse(checks.protocol(True, False, False))

    def test_native(self):
        observed = {'methods':['m'], 'events':['e'], 'stable':True, 'screenshot_bytes':1}
        crash = {'target':'x', 'crashed_target':'x', 'browser_exit':133, 'shutdown_ok':True}
        self.assertTrue(checks.native(observed, crash, {'m','e'})[0])
        self.assertFalse(checks.native(observed, crash, {'m','e','missing'})[0])
        for key in ('stable','screenshot_bytes'):
            bad = dict(observed, **{key:False})
            self.assertFalse(checks.native(bad, crash, {'m','e'})[0])
        for key, value in (('crashed_target','wrong'),('browser_exit',0),('shutdown_ok',False)):
            self.assertFalse(checks.native(observed, dict(crash, **{key:value}), {'m','e'})[0])

    def test_webmcp(self):
        self.assertTrue(checks.webmcp('undefined'))
        self.assertFalse(checks.webmcp('object'))

    def test_audio(self):
        pages = [{'state':'running','node':True,'sampleRate':48000} for _ in range(3)]
        samples = [{'listed':True, 'contexts':[{'state':'running','node':True,'time':5*i} for _ in range(3)]} for i in range(13)]
        self.assertTrue(checks.audio(pages,samples,[]))
        self.assertFalse(checks.audio(pages,samples,[{'method':'Target.targetCrashed'}]))
        bad = copy.deepcopy(samples)
        bad[3]['contexts'][0]['state'] = 'suspended'
        self.assertFalse(checks.audio(pages,bad,[]))
        bad = copy.deepcopy(samples)
        bad[4]['listed'] = False
        self.assertFalse(checks.audio(pages,bad,[]))

    def test_durability(self):
        saved = {'indexeddb':'persisted','service_worker':True,'cache':'persisted'}
        cases = {'restart':saved, 'interrupted':saved, 'full_disk':{'preserved':True}}
        self.assertTrue(checks.durability(cases))
        for case in ('restart','interrupted'):
            for key in saved:
                bad = copy.deepcopy(cases)
                bad[case][key] = None
                self.assertFalse(checks.durability(bad))
        bad = copy.deepcopy(cases)
        bad['full_disk'] = {'preserved':False}
        self.assertFalse(checks.durability(bad))

if __name__ == '__main__':
    unittest.main(verbosity=2)
