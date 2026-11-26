import base64
import hashlib
import json
import pathlib
import struct
import subprocess
import sys
import time

root = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(root / 'oracle/launch'))
sys.path.insert(0, str(root / 'oracle/harness/cert-verify'))
import launch
from pki import Key, Signature, aki, basic_constraints, certificate, cn, eku, extension, key_id, key_usage, name_constraints, san, ski
from wire import ANCHOR_CONSTRAINED, ANCHOR_ENFORCED, DISTRUSTED_SPKI, HINT, Record, Trust, encode

ORACLE = root / '.work/tmp/cert-verify/oracle'
POLICY_DIR = '/etc/chromium/policies/managed'
CONSTRAINED_DOMAIN = 'constrained.cert.test'
DAY = 86400
NOW = int(time.time())


def issue(subject, key, issuer=None, ca=False, names=(), nb=NOW - 30 * DAY, na=NOW + 60 * DAY, extensions=(), signature=None, path_len=None,
          purposes=('serverAuth',)):
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


def scenario():
    root_a = issue('Anchor Root A', Key('rsa', 'anchor-root-a'), ca=True, nb=NOW - 400 * DAY, na=NOW + 3650 * DAY)
    inter_a = issue('Anchor Intermediate A', Key('rsa', 'anchor-inter-a'), root_a, ca=True, nb=NOW - 400 * DAY, na=NOW + 1000 * DAY)
    hinted = issue('Anchor Hinted Intermediate', Key('ec', 'anchor-hinted'), root_a, ca=True, nb=NOW - 400 * DAY, na=NOW + 1000 * DAY)
    distrusted = issue('Anchor Distrusted Intermediate', Key('ec', 'anchor-distrusted'), root_a, ca=True, nb=NOW - 400 * DAY, na=NOW + 1000 * DAY)
    zero = issue('Anchor Pathlen Zero', Key('ec', 'anchor-pathlen0'), root_a, ca=True, path_len=0, nb=NOW - 400 * DAY, na=NOW + 1000 * DAY)
    below_zero = issue('Anchor Below Pathlen Zero', Key('ec', 'anchor-below'), zero, ca=True, nb=NOW - 400 * DAY, na=NOW + 1000 * DAY)
    root_b = issue('Anchor Constrained Root B', Key('ec', 'anchor-root-b', curve='P-384'), ca=True, nb=NOW - 400 * DAY, na=NOW + 3650 * DAY)
    stranger = issue('Anchor Untrusted Root', Key('rsa', 'anchor-stranger'), ca=True, nb=NOW - 400 * DAY, na=NOW + 3650 * DAY)
    nc_inter = issue('Anchor Name Constrained', Key('ec', 'anchor-nc'), root_a, ca=True, nb=NOW - 400 * DAY, na=NOW + 1000 * DAY,
                     extensions=[name_constraints(permitted=[('dns', 'nc.cert.test')])])
    rsa_leaf, ec_leaf, weak_leaf = Key('rsa', 'anchor-leaf'), Key('ec', 'anchor-leaf'), Key('rsa', 'anchor-weak-leaf', bits=1024)

    def leaf(host, issuer, key=rsa_leaf, **options):
        return issue(host, key, issuer, names=options.pop('names', [('dns', host)]), **options)

    cases = []

    def case(name, host, chain, key=rsa_leaf):
        cases.append({'name': name, 'host': host, 'chain': chain, 'key': key})

    case('valid', 'valid.cert.test', [leaf('valid.cert.test', inter_a), inter_a])
    case('valid-ecdsa', 'ecdsa.cert.test', [leaf('ecdsa.cert.test', inter_a, ec_leaf), inter_a], ec_leaf)
    case('wrong-host', 'wrong-host.cert.test', [leaf('elsewhere.cert.test', inter_a), inter_a])
    case('wildcard', 'any.wild.cert.test', [leaf('any.wild.cert.test', inter_a, names=[('dns', '*.wild.cert.test')]), inter_a])
    case('ip-san', '192.0.2.7', [leaf('ip', inter_a, names=[('ip', '192.0.2.7')]), inter_a])
    case('expired', 'expired.cert.test', [leaf('expired.cert.test', inter_a, nb=NOW - 90 * DAY, na=NOW - 2 * DAY), inter_a])
    case('not-yet-valid', 'future.cert.test', [leaf('future.cert.test', inter_a, nb=NOW + 2 * DAY, na=NOW + 90 * DAY), inter_a])
    case('untrusted-root', 'untrusted.cert.test', [leaf('untrusted.cert.test', stranger), stranger])
    case('self-signed', 'self.cert.test', [issue('self.cert.test', rsa_leaf, names=[('dns', 'self.cert.test')])])
    case('hinted-intermediate', 'hinted.cert.test', [leaf('hinted.cert.test', hinted)])
    case('missing-intermediate', 'missing.cert.test', [leaf('missing.cert.test', inter_a)])
    case('distrusted-intermediate', 'distrusted.cert.test', [leaf('distrusted.cert.test', distrusted), distrusted])
    case('pathlen-violation', 'pathlen.cert.test', [leaf('pathlen.cert.test', below_zero), below_zero, zero])
    case('constrained-root-inside', CONSTRAINED_DOMAIN, [leaf(CONSTRAINED_DOMAIN, root_b)])
    case('constrained-root-outside', 'outside.cert.test', [leaf('outside.cert.test', root_b)])
    case('name-constraint-inside', 'www.nc.cert.test', [leaf('www.nc.cert.test', nc_inter), nc_inter])
    case('name-constraint-violation', 'nc-violation.cert.test', [leaf('nc-violation.cert.test', nc_inter), nc_inter])
    case('client-auth-only', 'client.cert.test', [leaf('client.cert.test', inter_a, purposes=('clientAuth',)), inter_a])
    case('sha1-leaf', 'sha1.cert.test', [leaf('sha1.cert.test', inter_a, signature=Signature('rsa', 'sha1')), inter_a])
    case('weak-leaf-key', 'weak.cert.test', [leaf('weak.cert.test', inter_a, weak_leaf), inter_a], weak_leaf)
    case('unknown-critical', 'critical.cert.test', [leaf('critical.cert.test', inter_a, extensions=[
        extension('1.3.6.1.4.1.11129.99.3', b'\x05\x00', critical=True)]), inter_a])
    policy = {
        'CACertificates': [base64.b64encode(root_a.der).decode()],
        'CACertificatesWithConstraints': [{'certificate': base64.b64encode(root_b.der).decode(), 'constraints': {'permitted_dns_names': [CONSTRAINED_DOMAIN]}}],
        'CAHintCertificates': [base64.b64encode(hinted.der).decode()],
        'CADistrustedCertificates': [base64.b64encode(distrusted.der).decode()],
    }
    trust = [Trust(ANCHOR_ENFORCED, root_a.der), Trust(ANCHOR_CONSTRAINED, root_b.der, [CONSTRAINED_DOMAIN.encode()], []),
             Trust(HINT, hinted.der), Trust(DISTRUSTED_SPKI, spki(distrusted.der))]
    return cases, policy, trust


def pem(der):
    body = base64.b64encode(der).decode()
    return '-----BEGIN CERTIFICATE-----\n' + '\n'.join(body[i:i + 64] for i in range(0, len(body), 64)) + '\n-----END CERTIFICATE-----\n'


def harness(records):
    process = subprocess.Popen([str(ORACLE)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
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
    if process.wait():
        raise RuntimeError('harness failed')
    return outputs


CT_VIEW = {'not-enough-scts': 'not-compliant', 'not-diverse-scts': 'not-compliant', 'complies': 'compliant',
           'build-not-timely': 'unknown', 'details-not-available': 'unknown'}


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


def main():
    if not ORACLE.is_file():
        raise SystemExit(f'{ORACLE} is missing; run make cert-verify-harness')
    cases, policy, trust = scenario()
    run_dir = launch.new_run_dir('cert-verify-anchor', 'headless')
    files = run_dir / 'anchor'
    files.mkdir()
    for item in cases:
        (files / f"{item['name']}.chain.pem").write_text(''.join(pem(c.der) for c in item['chain']))
        (files / f"{item['name']}.key.pem").write_bytes(item['key'].path.read_bytes())
    archive = launch.resolve_archive(run_dir, 'x86_64', None, None)
    session = launch.open_session(run_dir, archive)
    browser = None
    try:
        encoded = base64.b64encode(json.dumps(policy).encode()).decode()
        session.root(f'mkdir -p {POLICY_DIR} && echo {encoded} | base64 -d > {POLICY_DIR}/cert-verify-anchor.json && chmod 644 {POLICY_DIR}/cert-verify-anchor.json')
        _, browser = session.start_browser('headless')
        sidecar_cases = [{'name': item['name'], 'host': item['host']} for item in cases]
        output = session.sidecar(['python3', '-B', '/cert-verify/anchor-sidecar.py', json.dumps(sidecar_cases)],
                                 {root / 'oracle/harness/cert-verify': '/cert-verify:ro', files: '/anchor:ro'}, timeout=600)
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
    records = [encode(Record(result['observed'][item['name']]['started'], item['host'].encode(), [c.der for c in item['chain']], trust)) for item in cases]
    failures = []
    for item, output_text in zip(cases, harness(records)):
        observed = observed_view(result['observed'][item['name']])
        expected = expected_view(output_text)
        if observed != expected:
            failures.append(f"{item['name']}: browser {json.dumps(observed)} harness {json.dumps(expected)} handshakes {result['handshakes'][item['name']]}")
    for failure in failures:
        print(failure, file=sys.stderr)
    if failures:
        raise SystemExit(f'anchor {len(cases) - len(failures)}/{len(cases)} agree')
    print(f'anchor {len(cases)}/{len(cases)}: TLS navigations through the SOCKS5 egress with enterprise-policy trust agree with the source harness on net error, CT compliance and verified chain')


if __name__ == '__main__':
    main()
