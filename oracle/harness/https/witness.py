import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'oracle/harness/https'))
import fixture

ORACLE = ROOT / '.work/tmp/cert-verify/oracle'


def main():
    cases, policy, trust, moment = fixture.sample()
    result = fixture.run(cases, policy, trust, oracle=ORACLE, label='https-fixture-witness', pinned_time=moment)
    evidence = result['evidence']
    comparison = evidence['comparisons'][0]
    if evidence['failures']:
        for failure in evidence['failures']:
            print(failure, file=sys.stderr)
        raise SystemExit('witness: browser and source harness disagree')
    if comparison['browser']['error'] != 'OK' or comparison['harness']['error'] != 'OK':
        raise SystemExit('witness: no verified HTTPS navigation')
    if len(comparison['browser']['chain']) != len(cases[0]['chain']) + 1:
        raise SystemExit('witness: verified chain is incomplete')
    connect = comparison['socks_connect'][0]
    print(f"witness trust {evidence['policy_path']} {','.join(sorted(policy))} root {evidence['chains']['verified']['sha256'][-1]}")
    print(f"witness chain {','.join(evidence['chains']['verified']['sha256'])}")
    print(f"witness pinned_time {comparison['pinned_time']}")
    print(f"witness socks {evidence['socks_bind']} -> {connect['host']}:{connect['port']} reply {connect['reply']}")
    print(f"witness browser {json.dumps(comparison['browser'], sort_keys=True)}")
    print(f"witness harness {json.dumps(comparison['harness'], sort_keys=True)}")
    print(f"witness handshakes {json.dumps(comparison['handshakes'])}")
    print(f"witness 1/1: verified HTTPS navigation through the SOCKS5 egress agrees with the source harness; evidence {evidence['run_dir']}/evidence.json")


if __name__ == '__main__':
    main()
