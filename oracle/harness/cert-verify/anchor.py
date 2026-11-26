import pathlib
import sys

root = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(root / 'oracle/harness/https'))
import fixture
from pki import Key, Signature, extension, name_constraints

ORACLE = root / '.work/tmp/cert-verify/oracle'
CONSTRAINED_DOMAIN = 'constrained.cert.test'
DAY = 86400


def scenario():
    now = fixture.now()
    issue = fixture.issue
    root_a = issue('Anchor Root A', Key('rsa', 'anchor-root-a'), ca=True, nb=now - 400 * DAY, na=now + 3650 * DAY)
    inter_a = issue('Anchor Intermediate A', Key('rsa', 'anchor-inter-a'), root_a, ca=True, nb=now - 400 * DAY, na=now + 1000 * DAY)
    hinted = issue('Anchor Hinted Intermediate', Key('ec', 'anchor-hinted'), root_a, ca=True, nb=now - 400 * DAY, na=now + 1000 * DAY)
    distrusted = issue('Anchor Distrusted Intermediate', Key('ec', 'anchor-distrusted'), root_a, ca=True, nb=now - 400 * DAY, na=now + 1000 * DAY)
    zero = issue('Anchor Pathlen Zero', Key('ec', 'anchor-pathlen0'), root_a, ca=True, path_len=0, nb=now - 400 * DAY, na=now + 1000 * DAY)
    below_zero = issue('Anchor Below Pathlen Zero', Key('ec', 'anchor-below'), zero, ca=True, nb=now - 400 * DAY, na=now + 1000 * DAY)
    root_b = issue('Anchor Constrained Root B', Key('ec', 'anchor-root-b', curve='P-384'), ca=True, nb=now - 400 * DAY, na=now + 3650 * DAY)
    stranger = issue('Anchor Untrusted Root', Key('rsa', 'anchor-stranger'), ca=True, nb=now - 400 * DAY, na=now + 3650 * DAY)
    nc_inter = issue('Anchor Name Constrained', Key('ec', 'anchor-nc'), root_a, ca=True, nb=now - 400 * DAY, na=now + 1000 * DAY,
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
    case('expired', 'expired.cert.test', [leaf('expired.cert.test', inter_a, nb=now - 90 * DAY, na=now - 2 * DAY), inter_a])
    case('not-yet-valid', 'future.cert.test', [leaf('future.cert.test', inter_a, nb=now + 2 * DAY, na=now + 90 * DAY), inter_a])
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
    policy = fixture.enterprise_policy(root_a.der, constrained=[(root_b.der, [CONSTRAINED_DOMAIN])],
                                       hints=[hinted.der], distrusted=[distrusted.der])
    trust = fixture.trust_entries(root_a.der, constrained=[(root_b.der, [CONSTRAINED_DOMAIN])],
                                  hints=[hinted.der], distrusted=[distrusted.der])
    return cases, policy, trust


def main():
    cases, policy, trust = scenario()
    result = fixture.run(cases, policy, trust, oracle=ORACLE, label='cert-verify-anchor')
    for failure in result['failures']:
        print(failure, file=sys.stderr)
    if result['failures']:
        raise SystemExit(f'anchor {len(cases) - len(result["failures"])}/{len(cases)} agree')
    print(f'anchor {len(cases)}/{len(cases)}: TLS navigations through the SOCKS5 egress with enterprise-policy trust agree with the source harness on net error, CT compliance and verified chain')


if __name__ == '__main__':
    main()
