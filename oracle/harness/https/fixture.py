import base64
import hashlib
import json
import pathlib
import struct
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'oracle/launch'))
sys.path.insert(0, str(ROOT / 'oracle/harness/cert-verify'))
import launch
from pki import Key, Signature, aki, basic_constraints, certificate, cn, eku, extension, key_id, key_usage, name_constraints, san, ski
from wire import ANCHOR_CONSTRAINED, ANCHOR_ENFORCED, DISTRUSTED_SPKI, HINT, Record, Trust, encode

POLICY_DIR = '/etc/chromium/policies/managed'
POLICY_FILE = 'manganese-https-fixture.json'
SOCKS_HOST = '127.0.0.1'
SOCKS_PORT = 1080
HTTPS_PORT = 443
SIDECAR = ROOT / 'oracle/harness/https/sidecar.py'
DAY = 86400
PAGE = b'HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 20\r\nConnection: close\r\n\r\n<!doctype html>ok!!!'
CT_VIEW = {'not-enough-scts': 'not-compliant', 'not-diverse-scts': 'not-compliant', 'complies': 'compliant',
           'build-not-timely': 'unknown', 'details-not-available': 'unknown'}
COMPARISON_FIELDS = ('error', 'ct', 'chain')


def now():
    return int(time.time())


def issue(subject, key, issuer=None, ca=False, names=(), nb=None, na=None, extensions=(), signature=None, path_len=None,
          purposes=('serverAuth',)):
    moment = now()
    nb = moment - 30 * DAY if nb is None else nb
    na = moment + 60 * DAY if na is None else na
    exts = [basic_constraints(True, path_len) if ca else basic_constraints(False),
            key_usage('keyCertSign', 'cRLSign') if ca else key_usage('digitalSignature', 'keyEncipherment'), ski(key_id(key))]
    if issuer is not None:
        exts.append(aki(key_id(issuer.key)))
    if not ca:
        exts.append(eku(*purposes))
    if names:
        exts.append(san(*names))
    exts += list(extensions)
    if issuer is None:
        return certificate(cn(subject), key, not_before=nb, not_after=na, extensions=exts, signature=signature)
    return certificate(cn(subject), key, issuer=issuer.subject, issuer_key=issuer.key, not_before=nb, not_after=na,
                       extensions=exts, signature=signature)


def spki(der):
    def element(offset):
        size, header = der[offset + 1], 2
        if size & 0x80:
            count = size & 0x7f
            size, header = int.from_bytes(der[offset + 2:offset + 2 + count], 'big'), 2 + count
        return offset + header, offset + header + size
    tbs_start = element(element(0)[0])[0]
    offset, fields = tbs_start, []
    while len(fields) < 7:
        start, end = element(offset)
        fields.append((der[offset], offset, end))
        offset = end
    index = 6 if fields[0][0] == 0xa0 else 5
    return der[fields[index][1]:fields[index][2]]


def pem(der):
    body = base64.b64encode(der).decode()
    return '-----BEGIN CERTIFICATE-----\n' + '\n'.join(body[i:i + 64] for i in range(0, len(body), 64)) + '\n-----END CERTIFICATE-----\n'


def b64(data):
    return base64.b64encode(data).decode()


def enterprise_policy(root_der, constrained=(), hints=(), distrusted=()):
    policy = {'CACertificates': [b64(root_der)]}
    if constrained:
        policy['CACertificatesWithConstraints'] = [
            {'certificate': b64(der), 'constraints': {'permitted_dns_names': list(names)}} for der, names in constrained]
    if hints:
        policy['CAHintCertificates'] = [b64(der) for der in hints]
    if distrusted:
        policy['CADistrustedCertificates'] = [b64(der) for der in distrusted]
    return policy


def trust_entries(root_der, constrained=(), hints=(), distrusted=()):
    entries = [Trust(ANCHOR_ENFORCED, root_der)]
    for der, names in constrained:
        entries.append(Trust(ANCHOR_CONSTRAINED, der, [name.encode() for name in names], []))
    for der in hints:
        entries.append(Trust(HINT, der))
    for der in distrusted:
        entries.append(Trust(DISTRUSTED_SPKI, spki(der)))
    return entries


def harness(oracle, records):
    process = subprocess.Popen([str(oracle)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    try:
        process.stdin.write(b'MGB1')
        process.stdin.flush()
        if process.stdout.read(4) != b'MGB1':
            raise RuntimeError('harness wrote an invalid protocol header')
        outputs = []
        for record in records:
            process.stdin.write(struct.pack('>I', len(record)) + record)
            process.stdin.flush()
            outputs.append(process.stdout.read(struct.unpack('>I', process.stdout.read(4))[0]).decode())
        process.stdin.close()
        code = process.wait()
    finally:
        if process.poll() is None:
            process.kill()
        process.wait()
        if not process.stdin.closed:
            try:
                process.stdin.close()
            except OSError:
                pass
    if code:
        raise RuntimeError('harness failed')
    return outputs


def expected_view(output):
    fields = dict(line.split(' ', 1) for line in output.strip().split('\n'))
    error = fields['error'].split(' ', 1)[1]
    if error != 'OK':
        return {'error': 'net::' + error}
    return {'error': 'OK', 'ct': CT_VIEW[fields['ct']], 'chain': fields['chain'].split(' ')}


def observed_view(observed):
    if observed.get('timeout'):
        return {'timeout': True}
    if observed['error'] != 'OK':
        return {'error': observed['error']}
    return {'error': 'OK', 'ct': observed.get('ct'), 'chain': [hashlib.sha256(base64.b64decode(c)).hexdigest() for c in observed['chain']]}


def run(cases, policy, trust, *, oracle, label='https-fixture', pinned_time=None, page=PAGE, files_dir=None,
        policy_dir=POLICY_DIR, policy_file=POLICY_FILE):
    oracle = pathlib.Path(oracle)
    if not oracle.is_file():
        raise SystemExit(f'{oracle} is missing; run make cert-verify-harness')
    run_dir = launch.new_run_dir(label, 'headless')
    files = pathlib.Path(files_dir) if files_dir is not None else run_dir / 'fixture'
    files.mkdir(parents=True, exist_ok=True)
    for item in cases:
        (files / f"{item['name']}.chain.pem").write_text(''.join(pem(cert.der) for cert in item['chain']))
        (files / f"{item['name']}.key.pem").write_bytes(item['key'].path.read_bytes())
    config = {
        'cases': [{'name': item['name'], 'host': item['host'], 'path': item.get('path', '/'),
                   'chain': f"{item['name']}.chain.pem", 'key': f"{item['name']}.key.pem"} for item in cases],
        'page': b64(page),
        'pinned_time': pinned_time,
        'socks': {'host': SOCKS_HOST, 'port': SOCKS_PORT, 'https_port': HTTPS_PORT},
    }
    archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
    session = launch.open_session(run_dir, archive)
    browser = None
    try:
        encoded = b64(json.dumps(policy).encode())
        session.root(f'mkdir -p {policy_dir} && echo {encoded} | base64 -d > {policy_dir}/{policy_file} && chmod 644 {policy_dir}/{policy_file}')
        _, browser = session.start_browser('headless')
        output = session.sidecar(['python3', '-B', '/https-fixture/sidecar.py', json.dumps(config)],
                                 {ROOT / 'oracle/harness/https': '/https-fixture:ro', files: '/fixture:ro'}, timeout=600)
        (run_dir / 'observed.json').write_text(output)
        shutdown = session.probe('--close')
        if not shutdown.get('shutdown', {}).get('ok') or browser.wait(timeout=15) != 0:
            raise RuntimeError('browser shutdown failed')
    finally:
        if browser is not None and browser.poll() is None:
            browser.kill()
            browser.wait()
        session.close()
    result = json.loads(output)
    times = [pinned_time if pinned_time is not None else result['observed'][item['name']]['started'] for item in cases]
    records = [encode(Record(moment, item['host'].encode(), [cert.der for cert in item['chain']], trust))
               for item, moment in zip(cases, times)]
    outputs = harness(oracle, records)
    failures, comparisons = [], []
    for item, moment, output_text in zip(cases, times, outputs):
        observed = observed_view(result['observed'][item['name']])
        expected = expected_view(output_text)
        if observed != expected:
            failures.append(f"{item['name']}: browser {json.dumps(observed)} harness {json.dumps(expected)} handshakes "
                             f"{result['handshakes'][item['name']]}")
        comparisons.append({
            'name': item['name'], 'host': item['host'], 'pinned_time': moment,
            'browser': observed, 'harness': expected,
            'handshakes': result['handshakes'][item['name']],
            'socks_connect': result['connects'][item['name']],
        })
    evidence = {
        'fixture': 'oracle/harness/https',
        'run_dir': str(run_dir.relative_to(ROOT)),
        'policy_path': f'{policy_dir}/{policy_file}',
        'trust_policy': policy,
        'socks_bind': f'{SOCKS_HOST}:{SOCKS_PORT}',
        'https_port': HTTPS_PORT,
        'comparison_fields': list(COMPARISON_FIELDS),
        'chains': {item['name']: {'host': item['host'], 'pem': ''.join(pem(cert.der) for cert in item['chain']),
                                  'sha256': [hashlib.sha256(cert.der).hexdigest() for cert in item['chain']]}
                   for item in cases},
        'comparisons': comparisons,
        'failures': failures,
    }
    (run_dir / 'evidence.json').write_text(json.dumps(evidence, indent=2, sort_keys=True))
    return {'failures': failures, 'evidence': evidence, 'run_dir': run_dir, 'output': output}


def sample():
    moment = now()
    root = issue('HTTPS Fixture Root', Key('rsa', 'anchor-root-a'), ca=True, nb=moment - 400 * DAY, na=moment + 3650 * DAY)
    intermediate = issue('HTTPS Fixture Intermediate', Key('rsa', 'anchor-inter-a'), root, ca=True,
                         nb=moment - 400 * DAY, na=moment + 1000 * DAY)
    host = 'verified.https.test'
    leaf = issue(host, Key('rsa', 'anchor-leaf'), intermediate, names=[('dns', host)], nb=moment - 30 * DAY, na=moment + 60 * DAY)
    case = {'name': 'verified', 'host': host, 'chain': [leaf, intermediate], 'key': Key('rsa', 'anchor-leaf')}
    policy = enterprise_policy(root.der)
    trust = trust_entries(root.der)
    return [case], policy, trust, moment


def build():
    cases, policy, trust, moment = sample()
    target = ROOT / '.work/tmp/https/fixture'
    target.mkdir(parents=True, exist_ok=True)
    for item in cases:
        (target / f"{item['name']}.chain.pem").write_text(''.join(pem(cert.der) for cert in item['chain']))
        (target / f"{item['name']}.key.pem").write_bytes(item['key'].path.read_bytes())
    manifest = {
        'fixture': 'oracle/harness/https',
        'host': cases[0]['host'],
        'pinned_time': moment,
        'policy_path': f'{POLICY_DIR}/{POLICY_FILE}',
        'trust_policy': policy,
        'trust_kinds': [entry.kind for entry in trust],
        'chain_sha256': [hashlib.sha256(cert.der).hexdigest() for cert in cases[0]['chain']],
        'root_sha256': hashlib.sha256(trust[0].der).hexdigest(),
        'socks_bind': f'{SOCKS_HOST}:{SOCKS_PORT}',
        'https_port': HTTPS_PORT,
        'comparison_fields': list(COMPARISON_FIELDS),
    }
    (target / 'fixture.json').write_text(json.dumps(manifest, indent=2, sort_keys=True))
    print(target.relative_to(ROOT))


def inspect():
    manifest = json.loads((ROOT / '.work/tmp/https/fixture/fixture.json').read_text())
    print(f"host {manifest['host']}")
    print(f"pinned_time {manifest['pinned_time']}")
    print(f"trust_policy {manifest['policy_path']} {','.join(sorted(manifest['trust_policy']))} kinds {manifest['trust_kinds']}")
    print(f"root {manifest['root_sha256']}")
    for digest in manifest['chain_sha256']:
        print(f"chain {digest}")
    print(f"socks {manifest['socks_bind']} -> host:{manifest['https_port']}")
    print(f"compare {','.join(manifest['comparison_fields'])}")


if __name__ == '__main__':
    if sys.argv[1] == 'build':
        build()
    elif sys.argv[1] == 'inspect':
        inspect()
    else:
        raise SystemExit('usage: fixture.py build|inspect')
