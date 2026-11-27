import importlib.util
import json
import pathlib
import struct
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(ROOT / 'oracle/harness/https'))
import ch
import fixture

_spec = importlib.util.spec_from_file_location('tls_wire', str(HERE / 'wire.py'))
wire = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(wire)

ORACLE = ROOT / '.work/tmp/tls-handshake/oracle'
CANDIDATE = ROOT / '.work/tmp/target/x86_64-unknown-linux-musl/debug/tls-handshake'
CERT_ORACLE = ROOT / '.work/tmp/cert-verify/oracle'
HOST = 'verified.https.test'
ALPN = [b'h2', b'http/1.1']
DROPPED = {0x44CD, 0xCA34}
FIXED = {0x0000, 0x0005, 0x000A, 0x000B, 0x000D, 0x0010, 0x0012, 0x0017, 0x001B, 0x0023, 0x002B, 0x002D}
VARIABLE = {0x0033, 0xFE0D, 0xFF01, ch.GREASE}


def parse(data):
    offset = 1
    count = struct.unpack('>H', data[offset:offset + 2])[0]
    offset += 2
    ciphers = [struct.unpack('>H', data[offset + 2 * i:offset + 2 * i + 2])[0] for i in range(count)]
    offset += 2 * count
    methods = data[offset + 1:offset + 1 + data[offset]]
    offset += 1 + data[offset]
    count = struct.unpack('>H', data[offset:offset + 2])[0]
    offset += 2
    extensions = {}
    for _ in range(count):
        kind = struct.unpack('>H', data[offset:offset + 2])[0]
        size = struct.unpack('>H', data[offset + 2:offset + 4])[0]
        value = data[offset + 4:offset + 4 + size]
        offset += 4 + size
        extensions.setdefault(kind, []).append(value)
    if offset != len(data):
        raise SystemExit('anchor: trailing bytes in a canonical ClientHello')
    return ciphers, methods, extensions


def project(data):
    ciphers, methods, extensions = parse(data)
    fixed = {kind: values for kind, values in extensions.items() if kind in FIXED}
    variable = {kind: len(values) for kind, values in extensions.items() if kind in VARIABLE}
    unknown = set(extensions) - FIXED - VARIABLE - DROPPED
    if unknown:
        raise SystemExit('anchor: unclassified extensions ' + ','.join(f'{kind:04x}' for kind in sorted(unknown)))
    return ciphers, methods, fixed, variable


def peer(path, record):
    process = subprocess.Popen([str(path)], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    try:
        process.stdin.write(b'MGB1')
        process.stdin.flush()
        if process.stdout.read(4) != b'MGB1':
            raise SystemExit(f'anchor: {path} wrote an invalid protocol header')
        process.stdin.write(struct.pack('>I', len(record)) + record)
        process.stdin.flush()
        size = struct.unpack('>I', process.stdout.read(4))[0]
        value = process.stdout.read(size)
        process.stdin.close()
        if process.wait() != 0:
            raise SystemExit(f'anchor: {path} failed')
        return value
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()


def main():
    for path in (ORACLE, CANDIDATE, CERT_ORACLE):
        if not path.is_file():
            raise SystemExit(f'anchor: {path.relative_to(ROOT)} is missing')
    cases, policy, trust, moment = fixture.sample()
    result = fixture.run(cases, policy, trust, oracle=CERT_ORACLE, label='tls-handshake-anchor',
                         pinned_time=moment, capture_clienthello=True)
    evidence = result['evidence']
    if evidence['failures']:
        for failure in evidence['failures']:
            print(failure, file=sys.stderr)
        raise SystemExit('anchor: the HTTPS fixture did not verify')
    raw = bytes.fromhex(evidence['clienthellos'][cases[0]['name']])
    browser = project(ch.canonicalize(raw))
    record = wire.encode(HOST, ALPN)
    harness = project(peer(ORACLE, record))
    candidate = project(peer(CANDIDATE, record))
    if browser != harness or browser != candidate:
        print('anchor: browser', json.dumps(browser, sort_keys=True, default=list), file=sys.stderr)
        print('anchor: harness', json.dumps(harness, sort_keys=True, default=list), file=sys.stderr)
        print('anchor: candidate', json.dumps(candidate, sort_keys=True, default=list), file=sys.stderr)
        raise SystemExit('anchor: ClientHello models disagree')
    comparison = evidence['comparisons'][0]
    print(f"anchor 1/1: browser, source harness and candidate agree on the modeled ClientHello subset")
    print(f"anchor ciphers {len(browser[0])} fixed-extensions {len(browser[2])} variable {json.dumps(browser[3], sort_keys=True)}")
    print(f"anchor trust {evidence['policy_path']} root {evidence['chains'][cases[0]['name']]['sha256'][-1]}")
    print(f"anchor host {HOST} alpn {','.join(p.decode() for p in ALPN)} pinned_time {comparison['pinned_time']}")
    print(f"anchor browser chain {','.join(comparison['browser']['chain'])}")
    print(f"anchor handshakes {json.dumps(comparison['handshakes'])}")
    print(f"anchor evidence {evidence['run_dir']}/evidence.json")


if __name__ == '__main__':
    main()
