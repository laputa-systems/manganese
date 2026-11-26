import base64
import hashlib
import pathlib
import re
import sys

from pki import (EKU, Key, Signature, aki, basic_constraints, bitstring, certificate, cn, ecdsa_algorithm, eku, epoch, explicit,
                 extension, generalized_time, ia5, implicit, inhibit_any_policy, integer, ip_bytes, key_id, key_usage, name,
                 name_constraints, null, octets, oid, policies, policy_constraints, policy_mappings, printable, pss_algorithm,
                 raw_integer, rsa_algorithm, san, seq, set_of, ski, tlv, utc_time, utf8)
from wire import ANCHOR, ANCHOR_CONSTRAINED, ANCHOR_ENFORCED, ANCHOR_OR_LEAF, DISTRUSTED_SPKI, HINT, LEAF, Record, Trust

T = 1790000000
T_LATE = 1800000000
T_EARLY = 1700000000
DAY = 86400
HOST = 'www.example.test'
SOURCE = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else '/work/src')

cases = []
def serial():
    return None


def der(item):
    return item if isinstance(item, bytes) else item.der


def add(label, chain, trust=(), time=T, host=HOST):
    label = re.sub(r'[^a-z0-9_-]', '_', label.lower())
    hostname = host.encode() if isinstance(host, str) else host
    cases.append((label, Record(time, hostname, [der(c) for c in chain], [t if isinstance(t, Trust) else Trust(t[0], der(t[1])) for t in trust])))


def spki_of(cert):
    return cert.key.spki()


KEYS = {}


def key(label, kind='rsa', bits=2048, curve='P-256', exponent=65537):
    token = (label, kind, bits, curve, exponent)
    if token not in KEYS:
        KEYS[token] = Key(kind, label, bits, curve, exponent)
    return KEYS[token]


def ca(subject, k, issuer=None, nb=T - 400 * DAY, na=T + 3650 * DAY, bc=True, ku=True, extra=(), signature=None,
       subject_name=None, issuer_name=None, keyid=True, **options):
    exts = []
    if bc is True:
        exts.append(basic_constraints(True))
    elif bc:
        exts.append(bc)
    if ku is True:
        exts.append(key_usage('keyCertSign', 'cRLSign'))
    elif ku:
        exts.append(ku)
    if keyid:
        exts.append(ski(key_id(k)))
        if issuer is not None:
            exts.append(aki(key_id(issuer.key)))
    exts += list(extra)
    subject_der = subject_name if subject_name is not None else cn(subject)
    if issuer is None:
        return certificate(subject_der, k, serial=options.pop('serial', None) if 'serial' in options else serial(), not_before=nb, not_after=na, extensions=exts, signature=signature, **options)
    return certificate(subject_der, k, issuer=issuer_name if issuer_name is not None else issuer.subject, issuer_key=issuer.key,
                       serial=options.pop('serial', None) if 'serial' in options else serial(), not_before=nb, not_after=na, extensions=exts, signature=signature, **options)


def leaf(issuer, names=(('dns', HOST),), k=None, nb=T - 30 * DAY, na=T + 60 * DAY, subject=None, ku=True, ext_key_usage=True,
         extra=(), signature=None, issuer_name=None, keyid=True, **options):
    k = k or key('leaf')
    exts = []
    if ku is True:
        exts.append(key_usage('digitalSignature', 'keyEncipherment'))
    elif ku:
        exts.append(ku)
    if ext_key_usage is True:
        exts.append(eku('serverAuth'))
    elif ext_key_usage:
        exts.append(ext_key_usage)
    if names:
        exts.append(san(*names))
    if keyid:
        exts.append(ski(key_id(k)))
        if issuer is not None:
            exts.append(aki(key_id(issuer.key)))
    exts += list(extra)
    subject_der = subject if subject is not None else cn(names[0][1] if names and names[0][0] == 'dns' and isinstance(names[0][1], str) else 'Leaf')
    if issuer is None:
        return certificate(subject_der, k, serial=options.pop('serial', None) if 'serial' in options else serial(), not_before=nb, not_after=na, extensions=exts, signature=signature, **options)
    return certificate(subject_der, k, issuer=issuer_name if issuer_name is not None else issuer.subject, issuer_key=issuer.key,
                       serial=options.pop('serial', None) if 'serial' in options else serial(), not_before=nb, not_after=na, extensions=exts, signature=signature, **options)


ROOT = ca('Root', key('root'))
INTER = ca('Intermediate', key('inter'), ROOT)
LEAF_CERT = leaf(INTER)
STANDARD = [LEAF_CERT, INTER]
TRUST_ROOT = [(ANCHOR, ROOT)]


def hostname_cases():
    groups = [
        ('exact', [('dns', HOST)], [HOST, 'example.test', 'a.www.example.test', 'www.example.test.', 'other.test', 'www.example.tes', 'ww.example.test']),
        ('wildcard', [('dns', '*.example.test')], ['foo.example.test', 'example.test', 'a.b.example.test', 'www.example.test.', '-.example.test', '123.example.test']),
        ('wildcard-unknown-tld', [('dns', '*.test')], ['foo.test', 'test']),
        ('wildcard-icann', [('dns', '*.co.uk')], ['foo.co.uk']),
        ('wildcard-private', [('dns', '*.appspot.com')], ['foo.appspot.com']),
        ('wildcard-com', [('dns', '*.com')], ['example.com']),
        ('wildcard-registrable', [('dns', '*.example.co.uk')], ['www.example.co.uk', 'example.co.uk']),
        ('partial-wildcard', [('dns', 'f*.example.test')], ['foo.example.test', 'f*.example.test']),
        ('inner-wildcard', [('dns', 'www.*.test')], ['www.example.test']),
        ('double-wildcard', [('dns', '*.*.example.test')], ['a.b.example.test']),
        ('uppercase-san', [('dns', 'WWW.EXAMPLE.TEST')], [HOST]),
        ('trailing-dot-san', [('dns', 'www.example.test.')], [HOST, 'www.example.test.']),
        ('empty-san', [('dns', '')], [HOST]),
        ('nul-san', [('dns', b'www.example.test\0.evil.test')], [HOST, 'www.example.test']),
        ('ipv4', [('ip', '192.0.2.1')], ['192.0.2.1', '192.0.2.2', '1.2.0.192']),
        ('ipv4-as-dns', [('dns', '192.0.2.1')], ['192.0.2.1']),
        ('ipv6', [('ip', '2001:db8::1')], ['2001:db8::1', '2001:db8::2', '[2001:db8::1]']),
        ('ipv4-mapped', [('ip', '::ffff:192.0.2.1')], ['192.0.2.1', '::ffff:c000:201']),
        ('ip-wildcard', [('dns', '*.0.2.1')], ['192.0.2.1']),
        ('single-label', [('dns', 'intranet')], ['intranet', 'localhost']),
        ('localhost', [('dns', 'localhost')], ['localhost']),
        ('wildcard-single-label', [('dns', '*.intranet')], ['host.intranet']),
        ('underscore', [('dns', 'under_score.example.test')], ['under_score.example.test']),
        ('idn', [('dns', 'xn--bcher-kva.example.test')], ['xn--bcher-kva.example.test']),
        ('multi', [('dns', 'a.example.test'), ('dns', 'b.example.test'), ('ip', '198.51.100.7')], ['a.example.test', 'b.example.test', '198.51.100.7', 'c.example.test']),
        ('email-only', [('email', 'admin@example.test')], [HOST]),
        ('uri-only', [('uri', 'https://www.example.test/')], [HOST]),
        ('long-label', [('dns', 'a' * 63 + '.example.test')], ['a' * 63 + '.example.test']),
        ('too-long-label', [('dns', 'a' * 64 + '.example.test')], ['a' * 64 + '.example.test']),
        ('dash-label', [('dns', '-dash.example.test')], ['-dash.example.test']),
        ('space-san', [('dns', 'www .example.test')], [HOST]),
    ]
    for label, names, hosts in groups:
        cert = leaf(INTER, names=names)
        for index, host in enumerate(hosts):
            add(f'host-{label}-{index}', [cert, INTER], TRUST_ROOT, host=host)
    no_san = leaf(INTER, names=(), subject=cn(HOST))
    add('host-cn-only', [no_san, INTER], TRUST_ROOT)
    add('host-cn-only-other', [no_san, INTER], TRUST_ROOT, host='other.test')
    for host in ('www.example.test', '127.0.0.1', '10.0.0.1', 'localhost', 'intranet', 'example.local'):
        add(f'host-reserved-{host}', [leaf(INTER, names=(('dns', host) if not host[0].isdigit() else ('ip', host),)), INTER], TRUST_ROOT, host=host)


def validity_cases():
    for label, nb, na in [('expired', T - 90 * DAY, T - 1), ('not-yet-valid', T + 1, T + 90 * DAY), ('starts-now', T, T + DAY),
                          ('ends-now', T - DAY, T), ('inverted', T + DAY, T - DAY), ('long-lived', T - 2000 * DAY, T + 2000 * DAY),
                          ('year-2050', T - DAY, epoch(2050, 1, 1)), ('year-9999', T - DAY, epoch(9999, 12, 31, 23, 59, 59)),
                          ('year-1950', epoch(1950, 1, 1), T + DAY)]:
        cert = leaf(INTER, nb=nb, na=na)
        add(f'validity-leaf-{label}', [cert, INTER], TRUST_ROOT)
        add(f'validity-leaf-{label}-untrusted', [cert, INTER])
        add(f'validity-leaf-{label}-host', [cert, INTER], TRUST_ROOT, host='other.test')
    expired_inter = ca('Intermediate', key('inter'), ROOT, nb=T - 400 * DAY, na=T - DAY)
    future_inter = ca('Intermediate', key('inter'), ROOT, nb=T + DAY, na=T + 400 * DAY)
    for label, inter in (('expired', expired_inter), ('not-yet-valid', future_inter)):
        add(f'validity-intermediate-{label}', [leaf(inter), inter], TRUST_ROOT)
        add(f'validity-intermediate-{label}-and-leaf', [leaf(inter, na=T - 1), inter], TRUST_ROOT)
    expired_root = ca('Root', key('root'), nb=T - 4000 * DAY, na=T - DAY)
    root_inter = ca('Intermediate', key('inter'), expired_root)
    root_chain = [leaf(root_inter), root_inter]
    for kind in (ANCHOR, ANCHOR_ENFORCED, ANCHOR_OR_LEAF, ANCHOR_CONSTRAINED):
        add(f'validity-root-expired-kind{kind}', root_chain, [(kind, expired_root)])
    future_root = ca('Root', key('root'), nb=T + DAY, na=T + 4000 * DAY)
    future_inter = ca('Intermediate', key('inter'), future_root)
    for kind in (ANCHOR, ANCHOR_ENFORCED):
        add(f'validity-root-future-kind{kind}', [leaf(future_inter), future_inter], [(kind, future_root)])
    for offset in (-1, 0, 1):
        add(f'validity-boundary-before{offset}', [LEAF_CERT, INTER], TRUST_ROOT, time=T - 30 * DAY + offset)
        add(f'validity-boundary-after{offset}', [LEAF_CERT, INTER], TRUST_ROOT, time=T + 60 * DAY + offset)
    raw_times = [('generalized-2049', generalized_time(T - DAY), generalized_time(epoch(2049, 1, 1))),
                 ('utc-no-seconds', tlv(0x17, b'2609010000Z'), tlv(0x17, b'2701010000Z')),
                 ('utc-offset', tlv(0x17, b'260901000000+0000'), tlv(0x17, b'270101000000Z')),
                 ('generalized-fraction', tlv(0x18, b'20260901000000.5Z'), tlv(0x18, b'20270101000000Z')),
                 ('utc-no-z', tlv(0x17, b'260901000000'), tlv(0x17, b'270101000000Z')),
                 ('generalized-local', tlv(0x18, b'20260901000000'), tlv(0x18, b'20270101000000Z')),
                 ('utc-feb-30', tlv(0x17, b'260230000000Z'), tlv(0x17, b'270101000000Z')),
                 ('utc-leap-second', tlv(0x17, b'260901235960Z'), tlv(0x17, b'270101000000Z')),
                 ('wrong-tag', tlv(0x04, b'260901000000Z'), tlv(0x17, b'270101000000Z'))]
    for label, nb, na in raw_times:
        cert = leaf(INTER, not_before_tlv=nb, not_after_tlv=na)
        add(f'validity-encoding-{label}', [cert, INTER], TRUST_ROOT)
        inter = ca('Intermediate', key('inter'), ROOT, not_before_tlv=nb, not_after_tlv=na)
        add(f'validity-encoding-intermediate-{label}', [leaf(inter), inter], TRUST_ROOT)
    for label, time in (('epoch', 0), ('negative', -86400 * 365), ('far-future', epoch(9999, 6, 1)), ('pre-1950', epoch(1940, 1, 1)),
                        ('late', T_LATE), ('early', T_EARLY)):
        add(f'validity-time-{label}', [LEAF_CERT, INTER], TRUST_ROOT, time=time)


def trust_cases():
    for kind in (ANCHOR, ANCHOR_ENFORCED, ANCHOR_CONSTRAINED, LEAF, ANCHOR_OR_LEAF, HINT):
        add(f'trust-root-kind{kind}', STANDARD, [(kind, ROOT)])
        add(f'trust-intermediate-kind{kind}', [LEAF_CERT], [(kind, INTER)])
        add(f'trust-intermediate-presented-kind{kind}', STANDARD, [(kind, INTER)])
        add(f'trust-leaf-kind{kind}', [LEAF_CERT], [(kind, LEAF_CERT)])
        add(f'trust-leaf-with-chain-kind{kind}', STANDARD, [(kind, LEAF_CERT)])
        add(f'trust-root-target-kind{kind}', [ROOT], [(kind, ROOT)])
    add('trust-none', STANDARD)
    add('trust-root-in-chain', [LEAF_CERT, INTER, ROOT])
    add('trust-root-in-chain-trusted', [LEAF_CERT, INTER, ROOT], TRUST_ROOT)
    add('trust-hint-intermediate', [LEAF_CERT], [(HINT, INTER), (ANCHOR, ROOT)])
    add('trust-missing-intermediate', [LEAF_CERT], TRUST_ROOT)
    add('trust-shuffled', [LEAF_CERT, ROOT, INTER], TRUST_ROOT)
    add('trust-duplicate-intermediate', [LEAF_CERT, INTER, INTER], TRUST_ROOT)
    add('trust-unrelated-extra', [LEAF_CERT, ca('Other', key('other')), INTER], TRUST_ROOT)
    add('trust-leaf-twice', [LEAF_CERT, LEAF_CERT, INTER], TRUST_ROOT)
    for label, entries in [('distrust-root', [(ANCHOR, ROOT), (DISTRUSTED_SPKI, spki_of(ROOT))]),
                           ('distrust-intermediate', [(ANCHOR, ROOT), (DISTRUSTED_SPKI, spki_of(INTER))]),
                           ('distrust-leaf', [(ANCHOR, ROOT), (DISTRUSTED_SPKI, spki_of(LEAF_CERT))]),
                           ('distrust-only', [(DISTRUSTED_SPKI, spki_of(ROOT))]),
                           ('distrust-garbage', [(ANCHOR, ROOT), (DISTRUSTED_SPKI, b'\x30\x00')]),
                           ('anchor-and-enforced', [(ANCHOR, ROOT), (ANCHOR_ENFORCED, ROOT)]),
                           ('enforced-and-anchor', [(ANCHOR_ENFORCED, ROOT), (ANCHOR, ROOT)]),
                           ('hint-and-anchor', [(HINT, ROOT), (ANCHOR, ROOT)]),
                           ('anchor-and-hint', [(ANCHOR, ROOT), (HINT, ROOT)]),
                           ('leaf-and-anchor', [(LEAF, ROOT), (ANCHOR, ROOT)]),
                           ('anchor-or-leaf-and-leaf', [(ANCHOR_OR_LEAF, LEAF_CERT), (LEAF, LEAF_CERT)]),
                           ('garbage-anchor', [(ANCHOR, b'\x30\x03\x02\x01\x01'), (ANCHOR, ROOT)]),
                           ('intermediate-and-root', [(ANCHOR, INTER), (ANCHOR, ROOT)])]:
        add(f'trust-{label}', STANDARD, entries)
    for label, dns, cidrs, host, names in [
        ('dns-permitted', [b'example.test'], [], HOST, [('dns', HOST)]),
        ('dns-other', [b'other.test'], [], HOST, [('dns', HOST)]),
        ('dns-exact', [b'www.example.test'], [], HOST, [('dns', HOST)]),
        ('dns-dot', [b'.example.test'], [], HOST, [('dns', HOST)]),
        ('dns-two', [b'other.test', b'example.test'], [], HOST, [('dns', HOST)]),
        ('dns-ip-san', [b'example.test'], [], '192.0.2.1', [('ip', '192.0.2.1')]),
        ('cidr-permitted', [], [(ip_bytes('192.0.2.0'), ip_bytes('255.255.255.0'))], '192.0.2.1', [('ip', '192.0.2.1')]),
        ('cidr-other', [], [(ip_bytes('198.51.100.0'), ip_bytes('255.255.255.0'))], '192.0.2.1', [('ip', '192.0.2.1')]),
        ('cidr-dns-san', [], [(ip_bytes('192.0.2.0'), ip_bytes('255.255.255.0'))], HOST, [('dns', HOST)]),
        ('cidr-v6', [], [(ip_bytes('2001:db8::'), ip_bytes('ffff:ffff::'))], '2001:db8::1', [('ip', '2001:db8::1')]),
        ('both', [b'example.test'], [(ip_bytes('192.0.2.0'), ip_bytes('255.255.255.0'))], HOST, [('dns', HOST), ('ip', '192.0.2.1')]),
        ('none', [], [], HOST, [('dns', HOST)])]:
        cert = leaf(INTER, names=names)
        add(f'trust-constrained-{label}', [cert, INTER], [Trust(ANCHOR_CONSTRAINED, ROOT.der, dns, cidrs)], host=host)
        self_signed = leaf(None, names=names, k=key('selfsigned'))
        add(f'trust-leaf-constrained-{label}', [self_signed], [Trust(LEAF, self_signed.der, dns, cidrs)], host=host)
        add(f'trust-anchor-or-leaf-constrained-{label}', [cert, INTER], [Trust(ANCHOR_OR_LEAF, ROOT.der, dns, cidrs)], host=host)
    self_signed = leaf(None, k=key('selfsigned'))
    for kind in (None, ANCHOR, LEAF, ANCHOR_OR_LEAF, ANCHOR_ENFORCED, HINT):
        add(f'self-signed-leaf-kind{kind}', [self_signed], [] if kind is None else [(kind, self_signed)])
    self_signed_ca = ca(HOST, key('selfsigned'), extra=[san(('dns', HOST))])
    for kind in (None, ANCHOR, LEAF, ANCHOR_OR_LEAF):
        add(f'self-signed-ca-kind{kind}', [self_signed_ca], [] if kind is None else [(kind, self_signed_ca)])
    issued_by_other = leaf(INTER, k=key('selfsigned'))
    add('trusted-leaf-not-self-signed', [issued_by_other], [(LEAF, issued_by_other)])
    add('self-signed-expired-trusted-leaf', [leaf(None, k=key('selfsigned'), na=T - 1)], [(LEAF, leaf(None, k=key('selfsigned'), na=T - 1))])
    expired_self = leaf(None, k=key('selfsigned'), nb=T - 90 * DAY, na=T - 1)
    add('self-signed-expired-trusted', [expired_self], [(LEAF, expired_self)])
    add('self-signed-local', [self_signed], [], host='printer.local')
    add('self-signed-private-ip', [leaf(None, names=(('ip', '192.168.1.1'),), k=key('selfsigned'))], [], host='192.168.1.1')


def cross_sign_cases():
    root_a = ca('Root A', key('roota'))
    root_b = ca('Root B', key('rootb', 'ec'))
    inter_a = ca('Cross Intermediate', key('cross'), root_a)
    inter_b = ca('Cross Intermediate', key('cross'), root_b)
    inter_a_expired = ca('Cross Intermediate', key('cross'), root_a, na=T - DAY)
    target = leaf(inter_a)
    configurations = [('a', [(ANCHOR, root_a)]), ('b', [(ANCHOR, root_b)]), ('both', [(ANCHOR, root_a), (ANCHOR, root_b)]),
                      ('both-reversed', [(ANCHOR, root_b), (ANCHOR, root_a)]),
                      ('a-distrusted', [(ANCHOR, root_a), (ANCHOR, root_b), (DISTRUSTED_SPKI, spki_of(root_a))]),
                      ('b-distrusted', [(ANCHOR, root_a), (ANCHOR, root_b), (DISTRUSTED_SPKI, spki_of(root_b))]),
                      ('none', [])]
    for chain_label, chain in [('ab', [target, inter_a, inter_b]), ('ba', [target, inter_b, inter_a]), ('a', [target, inter_a]),
                               ('b', [target, inter_b]), ('expired-a-b', [target, inter_a_expired, inter_b]),
                               ('b-expired-a', [target, inter_b, inter_a_expired])]:
        for trust_label, trust in configurations:
            add(f'cross-{chain_label}-{trust_label}', chain, trust)
    old_root = ca('Root Old', key('rootold'), nb=T - 6000 * DAY, na=T - DAY)
    new_root = ca('Root New', key('rootnew'))
    new_by_old = ca('Root New', key('rootnew'), old_root, nb=T - 3000 * DAY, na=T + 3000 * DAY)
    old_by_new = ca('Root Old', key('rootold'), new_root)
    inter = ca('Rollover Intermediate', key('inter'), new_root)
    chain = [leaf(inter), inter, new_by_old]
    for label, trust in [('old', [(ANCHOR, old_root)]), ('old-enforced', [(ANCHOR_ENFORCED, old_root)]), ('new', [(ANCHOR, new_root)]),
                         ('both', [(ANCHOR, old_root), (ANCHOR, new_root)]), ('new-hint-old', [(HINT, old_by_new), (ANCHOR, new_root)])]:
        add(f'rollover-{label}', chain, trust)
    kid_root = ca('Keyid Root', key('kidroot'))
    good = ca('Keyid Intermediate', key('kidgood'), kid_root)
    wrong_key = ca('Keyid Intermediate', key('kidbad'), kid_root)
    no_keyid = ca('Keyid Intermediate', key('kidgood'), kid_root, keyid=False)
    target = leaf(good)
    for label, chain in [('wrong-first', [target, wrong_key, good]), ('good-first', [target, good, wrong_key]),
                         ('no-keyid-first', [target, no_keyid, wrong_key]), ('only-wrong', [target, wrong_key])]:
        add(f'keyid-{label}', chain, [(ANCHOR, kid_root)])
    newer = ca('Validity Intermediate', key('kidgood'), kid_root, nb=T - 10 * DAY, na=T + 100 * DAY)
    older = ca('Validity Intermediate', key('kidgood'), kid_root, nb=T - 900 * DAY, na=T + 50 * DAY)
    older_expired = ca('Validity Intermediate', key('kidgood'), kid_root, nb=T - 900 * DAY, na=T - DAY)
    target = leaf(newer)
    for label, chain in [('newer-older', [target, newer, older]), ('older-newer', [target, older, newer]),
                         ('expired-newer', [target, older_expired, newer]), ('newer-expired', [target, newer, older_expired])]:
        add(f'prioritize-{label}', chain, [(ANCHOR, kid_root)])
    mesh_root = ca('Mesh Root', key('meshroot', 'ec'))
    mesh_good = ca('Mesh CA', key('meshgood', 'ec'), mesh_root)
    decoys = [ca('Mesh CA', key(f'mesh{i}', 'ec'), ca('Mesh Decoy Root', key(f'meshdecoy{i}', 'ec')), keyid=False) for i in range(24)]
    target = leaf(mesh_good, keyid=False)
    for count in (5, 18, 19, 20, 21, 24):
        add(f'iteration-decoys{count}-good-last', [target, *decoys[:count], mesh_good], [(ANCHOR, mesh_root)])
        add(f'iteration-decoys{count}-good-first', [target, mesh_good, *decoys[:count]], [(ANCHOR, mesh_root)])
    loop_a = ca('Loop A', key('loopa', 'ec'), ca('Loop B', key('loopb', 'ec')))
    loop_b = ca('Loop B', key('loopb', 'ec'), loop_a)
    add('loop', [leaf(loop_a), loop_a, loop_b])
    add('loop-trusted-b', [leaf(loop_a), loop_a, loop_b], [(ANCHOR, ca('Loop B', key('loopb', 'ec')))])
    chain = [ROOT]
    parent = ROOT
    for depth in range(1, 11):
        parent = ca(f'Depth {depth}', key(f'depth{depth}', 'ec'), parent)
        chain.insert(0, parent)
    deep_leaf = leaf(parent)
    add('depth-10', [deep_leaf, *chain[:-1]], TRUST_ROOT)
    add('depth-10-reversed', [deep_leaf, *reversed(chain[:-1])], TRUST_ROOT)
    add('depth-10-missing-middle', [deep_leaf, *chain[:4], *chain[5:-1]], TRUST_ROOT)


def constraint_cases():
    k = key('inter')
    variants = {
        'bc-missing': dict(bc=None), 'bc-not-ca': dict(bc=basic_constraints(False)), 'bc-noncritical': dict(bc=basic_constraints(True, critical=False)),
        'bc-pathlen0': dict(bc=basic_constraints(True, 0)), 'ku-missing': dict(ku=None), 'ku-no-certsign': dict(ku=key_usage('digitalSignature', 'cRLSign')),
        'ku-noncritical': dict(ku=key_usage('keyCertSign', critical=False)), 'ku-empty': dict(ku=key_usage()),
        'eku-server': dict(extra=[eku('serverAuth')]), 'eku-client': dict(extra=[eku('clientAuth')]), 'eku-any': dict(extra=[eku('any')]),
        'eku-any-client': dict(extra=[eku('any', 'clientAuth')]), 'eku-code': dict(extra=[eku('codeSigning')]),
        'eku-server-code': dict(extra=[eku('serverAuth', 'codeSigning')]), 'eku-server-timestamp': dict(extra=[eku('serverAuth', 'timeStamping')]),
        'eku-server-ocsp': dict(extra=[eku('serverAuth', 'OCSPSigning')]), 'eku-sgc': dict(extra=[eku('serverGatedCrypto')]),
        'eku-ms-sgc': dict(extra=[eku('msSGC')]), 'eku-email': dict(extra=[eku('emailProtection')]), 'eku-empty': dict(extra=[extension('2.5.29.37', seq())]),
        'unknown-critical': dict(extra=[extension('1.3.6.1.4.1.11129.99.1', null(), critical=True)]),
        'unknown-noncritical': dict(extra=[extension('1.3.6.1.4.1.11129.99.1', null())]),
        'duplicate-extension': dict(extra=[basic_constraints(True)]),
        'ct-poison': dict(extra=[extension('1.3.6.1.4.1.11129.2.4.3', null(), critical=True)]),
        'ms-app-policies-critical': dict(extra=[extension('1.3.6.1.4.1.311.21.10', seq(seq(oid('1.3.6.1.5.5.7.3.1'))), critical=True)]),
        'ms-app-policies-critical-eku': dict(extra=[eku('serverAuth'), extension('1.3.6.1.4.1.311.21.10', seq(seq(oid('1.3.6.1.5.5.7.3.1'))), critical=True)]),
        'policies-critical-cps': dict(extra=[policies('2.23.140.1.2.1', critical=True, qualifier=seq(oid('1.3.6.1.5.5.7.2.1'), ia5('https://cps.example.test/')))]),
        'policies-critical-unknown-qualifier': dict(extra=[policies('2.23.140.1.2.1', critical=True, qualifier=seq(oid('1.2.3.4'), null()))]),
        'policies-unknown-qualifier': dict(extra=[policies('2.23.140.1.2.1', qualifier=seq(oid('1.2.3.4'), null()))]),
        'aia-and-crl': dict(extra=[extension('1.3.6.1.5.5.7.1.1', seq(seq(oid('1.3.6.1.5.5.7.48.2'), implicit(6, b'http://aia.example.test/ca.cer')))),
                                   extension('2.5.29.31', seq(seq(explicit(0, explicit(0, implicit(6, b'http://crl.example.test/ca.crl'))))))]),
    }
    for label, options in variants.items():
        options = dict(options)
        inter = ca('Intermediate', k, ROOT, bc=options.pop('bc', True), ku=options.pop('ku', True), **options)
        add(f'intermediate-{label}', [leaf(inter), inter], TRUST_ROOT)
        root = ca('Constrained Root', key('croot'), bc=basic_constraints(True) if 'bc' not in variants[label] else variants[label]['bc'],
                  ku=variants[label].get('ku', True), extra=variants[label].get('extra', ()))
        root_inter = ca('Intermediate', k, root)
        for kind in (ANCHOR, ANCHOR_ENFORCED):
            add(f'root-{label}-kind{kind}', [leaf(root_inter), root_inter], [(kind, root)])
    pathlen_root = ca('Pathlen Root', key('plroot'))
    for root_len, chain_lengths in ((0, (1, 2)), (1, (1, 2, 3))):
        limited_root = ca('Pathlen Root', key('plroot'), bc=basic_constraints(True, root_len))
        for length in chain_lengths:
            for issuer_root, trust_kinds in ((limited_root, (ANCHOR, ANCHOR_ENFORCED)), (pathlen_root, (ANCHOR,))):
                parent = issuer_root
                chain = []
                for depth in range(length):
                    parent = ca(f'Pathlen {depth}', key(f'pl{depth}', 'ec'), parent, bc=basic_constraints(True, root_len if issuer_root is pathlen_root and depth == 0 else None))
                    chain.insert(0, parent)
                for kind in trust_kinds:
                    add(f'pathlen-root{root_len}-{"limited" if issuer_root is limited_root else "plain"}-chain{length}-kind{kind}', [leaf(parent), *chain], [(kind, issuer_root)])
    self_issued_root = ca('Self Issued', key('si1', 'ec'))
    self_issued = ca('Self Issued', key('si2', 'ec'), self_issued_root, bc=basic_constraints(True, 0))
    add('pathlen0-self-issued', [leaf(self_issued), self_issued], [(ANCHOR, self_issued_root)])
    rollover = ca('Pathlen Zero', key('pz1', 'ec'), ROOT, bc=basic_constraints(True, 0))
    self_rollover = ca('Pathlen Zero', key('pz2', 'ec'), rollover)
    add('pathlen0-then-self-issued', [leaf(self_rollover), self_rollover, rollover], TRUST_ROOT)


def leaf_cases():
    variants = {
        'eku-none': dict(ext_key_usage=None), 'eku-client': dict(ext_key_usage=eku('clientAuth')), 'eku-any': dict(ext_key_usage=eku('any')),
        'eku-server-client': dict(ext_key_usage=eku('serverAuth', 'clientAuth')), 'eku-code': dict(ext_key_usage=eku('codeSigning')),
        'eku-server-code': dict(ext_key_usage=eku('serverAuth', 'codeSigning')), 'eku-server-ocsp': dict(ext_key_usage=eku('serverAuth', 'OCSPSigning')),
        'eku-server-timestamp': dict(ext_key_usage=eku('serverAuth', 'timeStamping')), 'eku-server-email': dict(ext_key_usage=eku('serverAuth', 'emailProtection')),
        'eku-critical': dict(ext_key_usage=eku('serverAuth', critical=True)), 'eku-sgc': dict(ext_key_usage=eku('serverGatedCrypto')),
        'eku-any-server': dict(ext_key_usage=eku('any', 'serverAuth')), 'eku-unknown': dict(ext_key_usage=eku('1.2.3.4')),
        'ku-none': dict(ku=None), 'ku-certsign': dict(ku=key_usage('digitalSignature', 'keyCertSign')), 'ku-keyagreement': dict(ku=key_usage('keyAgreement')),
        'ku-encipher-only': dict(ku=key_usage('keyAgreement', 'encipherOnly')), 'ku-decipher-only': dict(ku=key_usage('keyAgreement', 'decipherOnly')),
        'ku-empty': dict(ku=key_usage()), 'ku-noncritical': dict(ku=key_usage('digitalSignature', critical=False)),
        'is-ca': dict(extra=[basic_constraints(True)]), 'pathlen-not-ca': dict(extra=[basic_constraints(False, 1)]),
        'not-ca-explicit': dict(extra=[basic_constraints(False)]),
        'unknown-critical': dict(extra=[extension('1.3.6.1.4.1.11129.99.2', null(), critical=True)]),
        'unknown-noncritical': dict(extra=[extension('1.3.6.1.4.1.11129.99.2', null())]),
        'ct-poison': dict(extra=[extension('1.3.6.1.4.1.11129.2.4.3', null(), critical=True)]),
        'sct-list-garbage': dict(extra=[extension('1.3.6.1.4.1.11129.2.4.2', octets(b'\x00\x05\x00\x03abc'))]),
        'ms-app-policies-critical': dict(extra=[extension('1.3.6.1.4.1.311.21.10', seq(seq(oid('1.3.6.1.5.5.7.3.1'))), critical=True)]),
        'name-constraints': dict(extra=[name_constraints(permitted=[('dns', 'example.test')])]),
        'duplicate-san': dict(extra=[san(('dns', HOST))]),
        'san-critical': dict(extra=[], names=None, subject=seq(), san_critical=True),
        'policy': dict(extra=[policies('2.23.140.1.2.1')]), 'policy-any': dict(extra=[policies('2.5.29.32.0')]),
        'policy-ev': dict(extra=[policies('2.23.140.1.1')]), 'policy-duplicate': dict(extra=[policies('1.2.3', '1.2.3')]),
        'policy-mapping': dict(extra=[policy_mappings(('1.2.3', '1.2.4'))]), 'inhibit-any': dict(extra=[inhibit_any_policy(0)]),
        'require-explicit': dict(extra=[policy_constraints(require=0)]),
        'subject-empty': dict(subject=seq()), 'subject-printable': dict(subject=name(('CN', printable(HOST)))),
        'san-othername': dict(names=[('raw', implicit(0, oid('1.3.6.1.4.1.311.20.2.3') + explicit(0, utf8('user')), constructed=True)), ('dns', HOST)]),
        'san-registered-id': dict(names=[('raw', implicit(8, b'\x2a\x03')), ('dns', HOST)]),
        'san-bad-ip-length': dict(names=[('ip', b'\xc0\x00\x02'), ('dns', HOST)]),
        'san-ip-with-mask': dict(names=[('ip', bytes(8)), ('dns', HOST)]),
        'san-dirname': dict(names=[('dir', cn('Directory')), ('dns', HOST)]),
        'san-empty-sequence': dict(names=[], raw_san=True),
        'san-x400': dict(names=[('raw', implicit(3, b'', constructed=True)), ('dns', HOST)]),
        'subject-unique-id': dict(subject_unique_id=b'\x01\x02'),
        'issuer-unique-id': dict(issuer_unique_id=b'\x01\x02'),
    }
    for label, options in variants.items():
        options = dict(options)
        names = options.pop('names', (('dns', HOST),))
        if options.pop('san_critical', False):
            options['extra'] = [san(('dns', HOST), critical=True)]
            names = ()
        if options.pop('raw_san', False):
            options['extra'] = [extension('2.5.29.17', seq())]
            names = ()
        cert = leaf(INTER, names=names, **options)
        add(f'leaf-{label}', [cert, INTER], TRUST_ROOT)
    for version in (1, 2, 4):
        add(f'leaf-version{version}-extensions', [leaf(INTER, version=version), INTER], TRUST_ROOT)
    v1 = certificate(cn(HOST), key('leaf'), issuer=INTER.subject, issuer_key=INTER.key, serial=serial(), not_before=T - DAY, not_after=T + DAY, version=1)
    add('leaf-version1-plain', [v1, INTER], TRUST_ROOT)
    v1_inter = certificate(cn('V1 Intermediate'), key('inter'), issuer=ROOT.subject, issuer_key=ROOT.key, serial=serial(), not_before=T - DAY, not_after=T + 400 * DAY, version=1)
    add('intermediate-version1', [leaf(v1_inter, keyid=False), v1_inter], TRUST_ROOT)
    v1_root = certificate(cn('V1 Root'), key('root'), serial=serial(), not_before=T - DAY, not_after=T + 400 * DAY, version=1)
    v1_root_inter = ca('Intermediate', key('inter'), v1_root, keyid=False)
    for kind in (ANCHOR, ANCHOR_ENFORCED):
        add(f'root-version1-kind{kind}', [leaf(v1_root_inter), v1_root_inter], [(kind, v1_root)])
    for label, value in [('zero', 0), ('negative', -5), ('large', 1 << 159), ('too-large', 1 << 170)]:
        add(f'serial-{label}', [leaf(INTER, serial=value), INTER], TRUST_ROOT)
    for label, raw in [('padded', b'\x00\x01'), ('empty', b''), ('negative-padded', b'\xff\x80')]:
        add(f'serial-{label}', [leaf(INTER, serial=raw), INTER], TRUST_ROOT)
    add('tbs-trailing-data', [leaf(INTER, extra_tbs=b'\x05\x00'), INTER], TRUST_ROOT)
    trailing = LEAF_CERT.der + b'\x00'
    add('cert-trailing-byte', [trailing, INTER], TRUST_ROOT)
    add('intermediate-trailing-byte', [LEAF_CERT, INTER.der + b'\x00'], TRUST_ROOT)
    add('leaf-garbage', [b'\x30\x03\x02\x01\x01', INTER], TRUST_ROOT)
    add('intermediate-garbage', [LEAF_CERT, b'\x30\x03\x02\x01\x01'], TRUST_ROOT)


def name_cases():
    issuer_k = key('inter')
    for label, subject_name, issuer_name in [
        ('printable-vs-utf8', name(('CN', printable('Name Intermediate'))), name(('CN', utf8('Name Intermediate')))),
        ('case', name(('CN', 'Name Intermediate')), name(('CN', 'NAME INTERMEDIATE'))),
        ('whitespace', name(('CN', 'Name Intermediate')), name(('CN', '  Name   Intermediate '))),
        ('bmp', name(('CN', tlv(0x1e, 'Name Intermediate'.encode('utf-16-be')))), name(('CN', 'Name Intermediate'))),
        ('teletex', name(('CN', tlv(0x14, b'Name Intermediate'))), name(('CN', 'Name Intermediate'))),
        ('universal', name(('CN', tlv(0x1c, 'Name Intermediate'.encode('utf-32-be')))), name(('CN', 'Name Intermediate'))),
        ('ia5', name(('CN', ia5('Name Intermediate'))), name(('CN', 'Name Intermediate'))),
        ('rdn-order', name(('O', 'Org'), ('CN', 'Name Intermediate')), name(('CN', 'Name Intermediate'), ('O', 'Org'))),
        ('multivalue-rdn', seq(set_of(seq(oid('2.5.4.3'), utf8('Name Intermediate')), seq(oid('2.5.4.10'), utf8('Org')))),
         seq(set_of(seq(oid('2.5.4.10'), utf8('Org')), seq(oid('2.5.4.3'), utf8('Name Intermediate'))))),
        ('different', name(('CN', 'Name Intermediate')), name(('CN', 'Other Intermediate'))),
        ('empty-issuer', name(('CN', 'Name Intermediate')), seq()),
        ('country-utf8', name(('C', 'US'), ('CN', 'Name Intermediate')), name(('C', utf8('US')), ('CN', 'Name Intermediate'))),
        ('invalid-printable', name(('CN', printable('Name*Intermediate'))), name(('CN', 'Name*Intermediate'))),
        ('invalid-utf8', name(('CN', tlv(0x0c, b'Name\xffIntermediate'))), name(('CN', tlv(0x0c, b'Name\xffIntermediate')))),
    ]:
        inter = ca('unused', issuer_k, ROOT, subject_name=subject_name)
        add(f'name-{label}', [leaf(inter, issuer_name=issuer_name), inter], TRUST_ROOT)


def nc(permitted=(), excluded=(), critical=True):
    return name_constraints(permitted, excluded, critical)


def name_constraint_cases():
    k = key('inter')
    constraint_sets = {
        'permit-dns': nc([('dns', 'example.test')]), 'permit-dns-dot': nc([('dns', '.example.test')]), 'permit-dns-other': nc([('dns', 'other.test')]),
        'exclude-dns': nc(excluded=[('dns', 'example.test')]), 'exclude-dns-other': nc(excluded=[('dns', 'other.test')]),
        'permit-ip': nc([('ip', '192.0.2.0/24')]), 'exclude-ip': nc(excluded=[('ip', '192.0.2.0/24')]), 'permit-ip6': nc([('ip', '2001:db8::/32')]),
        'permit-email': nc([('email', 'example.test')]), 'permit-uri': nc([('uri', '.example.test')]),
        'permit-dir': nc([('dir', name(('O', 'Org')))]), 'exclude-dir': nc(excluded=[('dir', name(('O', 'Org')))]),
        'permit-dns-noncritical': nc([('dns', 'example.test')], critical=False), 'empty': nc(),
        'permit-dns-and-ip': nc([('dns', 'example.test'), ('ip', '192.0.2.0/24')]),
        'exclude-all-dns': nc(excluded=[('dns', '')]), 'permit-empty-dns': nc([('dns', '')]),
        'permit-wildcard-dns': nc([('dns', '*.example.test')]),
        'permit-bad-ip-mask': extension('2.5.29.30', seq(implicit(0, seq(implicit(7, bytes([192, 0, 2, 0, 255, 0, 255, 0]))), constructed=True)), True),
        'permit-registered-id': extension('2.5.29.30', seq(implicit(0, seq(implicit(8, b'\x2a\x03')), constructed=True)), True),
        'with-minimum': extension('2.5.29.30', seq(implicit(0, seq(implicit(2, b'example.test'), implicit(0, b'\x01')), constructed=True)), True),
        'with-maximum': extension('2.5.29.30', seq(implicit(0, seq(implicit(2, b'example.test'), implicit(1, b'\x01')), constructed=True)), True),
    }
    leaves = {
        'dns': dict(names=[('dns', HOST)]), 'dns-other': dict(names=[('dns', 'www.other.test')]),
        'dns-apex': dict(names=[('dns', 'example.test')]), 'dns-suffix-trick': dict(names=[('dns', 'www.badexample.test')]),
        'ip': dict(names=[('ip', '192.0.2.5')]), 'ip-other': dict(names=[('ip', '198.51.100.5')]),
        'cn-only': dict(names=(), subject=cn(HOST)), 'cn-other-only': dict(names=(), subject=cn('www.other.test')),
        'cn-ip-only': dict(names=(), subject=cn('192.0.2.5')),
        'dir-org': dict(names=[('dns', HOST)], subject=name(('O', 'Org'), ('CN', HOST))),
        'email': dict(names=[('dns', HOST), ('email', 'user@example.test')]), 'email-subject': dict(names=[('dns', HOST)], subject=name(('E', 'user@other.test'), ('CN', HOST))),
        'uri': dict(names=[('dns', HOST), ('uri', 'https://www.example.test/')]),
        'wildcard': dict(names=[('dns', '*.example.test')]),
    }
    hosts = {'ip': '192.0.2.5', 'ip-other': '198.51.100.5', 'cn-ip-only': '192.0.2.5', 'dns-other': 'www.other.test', 'cn-other-only': 'www.other.test',
             'dns-apex': 'example.test', 'dns-suffix-trick': 'www.badexample.test', 'wildcard': 'foo.example.test'}
    for constraint_label, constraint in constraint_sets.items():
        inter = ca('NC Intermediate', k, ROOT, extra=[constraint])
        for leaf_label, options in leaves.items():
            add(f'nc-{constraint_label}-{leaf_label}', [leaf(inter, **options), inter], TRUST_ROOT, host=hosts.get(leaf_label, HOST))
    root = ca('NC Root', key('ncroot'), extra=[nc([('dns', 'example.test')])])
    inter = ca('Intermediate', k, root)
    for kind in (ANCHOR, ANCHOR_ENFORCED, ANCHOR_CONSTRAINED):
        for leaf_label in ('dns', 'dns-other'):
            add(f'nc-root-kind{kind}-{leaf_label}', [leaf(inter, **leaves[leaf_label]), inter], [(kind, root)], host=hosts.get(leaf_label, HOST))
    parent = ca('NC Parent', key('ncparent'), ROOT, extra=[nc([('dns', 'example.test')])])
    child = ca('NC Child', k, parent, extra=[nc([('dns', 'other.test')])])
    add('nc-nested-disjoint', [leaf(child), child, parent], TRUST_ROOT)
    self_issued_parent = ca('NC Self', key('ncself1', 'ec'), ROOT, extra=[nc([('dns', 'other.test')])])
    self_issued_child = ca('NC Self', key('ncself2', 'ec'), self_issued_parent)
    add('nc-self-issued-intermediate', [leaf(self_issued_child), self_issued_child, self_issued_parent], TRUST_ROOT)
    many = ca('NC Many', k, ROOT, extra=[nc([('dns', f'host{i}.example.test') for i in range(300)])])
    add('nc-many-subtrees', [leaf(many, names=[('dns', f'n{i}.example.test') for i in range(300)]), many], TRUST_ROOT, host='n1.example.test')


def policy_cases():
    k = key('inter')
    p1, p2, p3, anyp = '1.2.3.1', '1.2.3.2', '1.2.3.3', '2.5.29.32.0'
    shapes = {
        'none': ((), (), ()),
        'match': ((), (policies(p1),), (policies(p1),)),
        'leaf-other': ((), (policies(p1),), (policies(p2),)),
        'inter-any': ((), (policies(anyp),), (policies(p1),)),
        'leaf-any': ((), (policies(p1),), (policies(anyp),)),
        'root-require': ((policy_constraints(require=0),), (policies(p1),), (policies(p1),)),
        'root-require-mismatch': ((policy_constraints(require=0),), (policies(p1),), (policies(p2),)),
        'root-require-none': ((policy_constraints(require=0),), (), ()),
        'inter-require': ((), (policies(p1), policy_constraints(require=0)), (policies(p2),)),
        'inter-require-leaf-none': ((), (policies(p1), policy_constraints(require=0)), ()),
        'inter-inhibit-any': ((), (policies(anyp), inhibit_any_policy(0)), (policies(p1),)),
        'mapping': ((), (policies(p1), policy_mappings((p1, p2))), (policies(p2),)),
        'mapping-any': ((), (policies(p1), policy_mappings((anyp, p2))), (policies(p2),)),
        'mapping-inhibited': ((), (policies(p1), policy_mappings((p1, p2)), policy_constraints(inhibit=0)), (policies(p2),)),
        'mapping-require': ((), (policies(p1), policy_mappings((p1, p2)), policy_constraints(require=0)), (policies(p2),)),
        'root-policy-other': ((policies(p3),), (policies(p1),), (policies(p1),)),
        'root-policy-require': ((policies(p1), policy_constraints(require=0)), (policies(p1),), (policies(p1),)),
        'root-inhibit-mapping': ((policy_constraints(inhibit=0),), (policies(p1), policy_mappings((p1, p2))), (policies(p2),)),
        'root-inhibit-any': ((inhibit_any_policy(0),), (policies(anyp),), (policies(p1),)),
        'policy-constraints-empty': ((), (extension('2.5.29.36', seq(), True),), ()),
        'inhibit-any-negative': ((), (extension('2.5.29.54', integer(-1), True),), ()),
    }
    for label, (root_exts, inter_exts, leaf_exts) in shapes.items():
        root = ca('Policy Root', key('policyroot'), extra=root_exts)
        inter = ca('Policy Intermediate', k, root, extra=inter_exts)
        for kind in (ANCHOR, ANCHOR_ENFORCED):
            add(f'policy-{label}-kind{kind}', [leaf(inter, extra=leaf_exts), inter], [(kind, root)])


def signature_cases():
    k = key('inter')
    sigs = {
        'rsa-sha1': Signature('rsa', 'sha1'), 'rsa-md5': Signature('rsa', 'md5'), 'rsa-sha384': Signature('rsa', 'sha384'),
        'rsa-sha512': Signature('rsa', 'sha512'), 'rsa-sha256-no-null': Signature('rsa', 'sha256', algorithm=rsa_algorithm('sha256', params=False)),
        'pss-sha256': Signature('pss', 'sha256', 32), 'pss-sha384': Signature('pss', 'sha384', 48), 'pss-sha512': Signature('pss', 'sha512', 64),
        'pss-sha256-salt20': Signature('pss', 'sha256', 20), 'pss-sha256-mgf-sha1': Signature('pss', 'sha256', 32, 'sha1'),
        'pss-sha256-trailer': Signature('pss', 'sha256', 32, algorithm=pss_algorithm('sha256', 32, trailer=1)),
        'pss-sha1': Signature('pss', 'sha1', 20, algorithm=pss_algorithm('sha1', 20)),
        'corrupt': Signature('rsa', 'sha256', corrupt=True),
        'rsa-oid-sha1-alt': Signature('rsa', 'sha1', algorithm=seq(oid('1.3.14.3.2.29'), null())),
        'ecdsa-oid-on-rsa': Signature('rsa', 'sha256', algorithm=ecdsa_algorithm('sha256')),
    }
    for label, signature in sigs.items():
        cert = leaf(INTER, signature=signature)
        add(f'sig-leaf-{label}', [cert, INTER], TRUST_ROOT)
        inter = ca('Intermediate', k, ROOT, signature=signature)
        add(f'sig-intermediate-{label}', [leaf(inter), inter], TRUST_ROOT)
    add('sig-outer-mismatch', [leaf(INTER, outer_algorithm=rsa_algorithm('sha384')), INTER], TRUST_ROOT)
    add('sig-outer-no-null', [leaf(INTER, outer_algorithm=rsa_algorithm('sha256', params=False)), INTER], TRUST_ROOT)
    add('sig-inner-no-null', [leaf(INTER, signature=Signature('rsa', 'sha256', algorithm=rsa_algorithm('sha256', params=False)), outer_algorithm=rsa_algorithm('sha256')), INTER], TRUST_ROOT)
    add('sig-outer-sha1-alt', [leaf(INTER, signature=Signature('rsa', 'sha1'), outer_algorithm=seq(oid('1.3.14.3.2.29'), null())), INTER], TRUST_ROOT)
    ec_root = ca('EC Root', key('ecroot', 'ec', curve='P-384'))
    ec_inter = ca('EC Intermediate', key('ecinter', 'ec', curve='P-256'), ec_root)
    for curve in ('P-256', 'P-384', 'P-521', 'P-224'):
        cert = leaf(ec_inter, k=key('ecleaf', 'ec', curve=curve))
        add(f'ec-leaf-{curve}', [cert, ec_inter], [(ANCHOR, ec_root)])
        inter = ca('EC Intermediate', key('ecinter2', 'ec', curve=curve), ec_root)
        add(f'ec-intermediate-{curve}', [leaf(inter), inter], [(ANCHOR, ec_root)])
    p521_root = ca('P521 Root', key('p521root', 'ec', curve='P-521'))
    p521_inter = ca('P521 Intermediate', key('p521inter', 'ec', curve='P-521'), p521_root)
    for digest in ('sha256', 'sha384', 'sha512'):
        add(f'p521-chain-{digest}', [leaf(p521_inter, signature=Signature('ecdsa', digest)), ca('P521 Intermediate', key('p521inter', 'ec', curve='P-521'), p521_root, signature=Signature('ecdsa', digest))], [(ANCHOR, p521_root)])
        add(f'p521-leaf-signed-{digest}', [leaf(p521_inter, signature=Signature('ecdsa', digest)), p521_inter], [(ANCHOR, p521_root)])
    for digest in ('sha1', 'sha256', 'sha384', 'sha512'):
        add(f'ec-sig-{digest}', [leaf(ec_inter, signature=Signature('ecdsa', digest)), ec_inter], [(ANCHOR, ec_root)])
    add('ec-sig-null-params', [leaf(ec_inter, signature=Signature('ecdsa', 'sha256', algorithm=seq(oid('1.2.840.10045.4.3.2'), null()))), ec_inter], [(ANCHOR, ec_root)])
    add('ec-sig-corrupt', [leaf(ec_inter, signature=Signature('ecdsa', 'sha256', corrupt=True)), ec_inter], [(ANCHOR, ec_root)])
    add('ec-sig-wrong-key-type', [leaf(ec_inter, signature=Signature('ecdsa', 'sha256'), outer_algorithm=rsa_algorithm('sha256')), ec_inter], [(ANCHOR, ec_root)])
    ed_inter = ca('Ed Intermediate', key('edinter', 'ed25519'), ROOT)
    add('ed25519-intermediate-signs', [leaf(ed_inter), ed_inter], TRUST_ROOT)
    add('ed25519-leaf-key', [leaf(INTER, k=key('edleaf', 'ed25519')), INTER], TRUST_ROOT)
    for bits in (512, 1024, 1536, 3072, 4096):
        add(f'rsa{bits}-leaf-key', [leaf(INTER, k=key('rsaleaf', bits=bits)), INTER], TRUST_ROOT)
        inter = ca('Intermediate', key('rsainter', bits=bits), ROOT)
        add(f'rsa{bits}-intermediate', [leaf(inter), inter], TRUST_ROOT)
        root = ca('Sized Root', key('rsaroot', bits=bits))
        root_inter = ca('Intermediate', k, root)
        add(f'rsa{bits}-root', [leaf(root_inter), root_inter], [(ANCHOR, root)])
    add('rsa-exponent3-leaf', [leaf(INTER, k=key('rsae3', exponent=3)), INTER], TRUST_ROOT)
    e3_inter = ca('Intermediate', key('rsae3', exponent=3), ROOT)
    add('rsa-exponent3-intermediate', [leaf(e3_inter), e3_inter], TRUST_ROOT)
    bad_spki = seq(seq(oid('1.2.840.113549.1.1.1'), null()), bitstring(b'\x30\x03\x02\x01\x01'))
    add('spki-garbage-leaf', [leaf(INTER, spki=bad_spki), INTER], TRUST_ROOT)
    inter = ca('Intermediate', k, ROOT, spki=bad_spki)
    add('spki-garbage-intermediate', [leaf(inter), inter], TRUST_ROOT)
    add('spki-unknown-algorithm-leaf', [leaf(INTER, spki=seq(seq(oid('1.2.3.4.5')), bitstring(b'\x01\x02'))), INTER], TRUST_ROOT)
    add('spki-rsa-no-null-leaf', [leaf(INTER, spki=key('leaf').spki(seq(oid('1.2.840.113549.1.1.1')))), INTER], TRUST_ROOT)
    add('spki-ec-no-curve-leaf', [leaf(INTER, spki=key('ecleaf', 'ec').spki(seq(oid('1.2.840.10045.2.1')))), INTER], TRUST_ROOT)
    add('sig-wrong-issuer-key', [leaf(INTER, signature=None, issuer_name=None), ca('Intermediate', key('other'), ROOT)], TRUST_ROOT)


def crs_cases():
    path = SOURCE / 'net/data/ssl/chrome_root_store/root_store.certs'
    roots = [base64.b64decode(''.join(block.split())) for block in re.findall(r'-----BEGIN CERTIFICATE-----(.*?)-----END CERTIFICATE-----', path.read_text(), re.S)]
    for index, root in enumerate(roots):
        digest = hashlib.sha256(root).hexdigest()[:16]
        for time_label, time in (('timely', T), ('late', T_LATE), ('early', T_EARLY), ('pre-constraint', 1760000000)):
            add(f'crs-{digest}-{time_label}', [root], time=time, host='example.com')
        add(f'crs-{digest}-localhost', [root], host='localhost')
        if index % 7 == 0:
            add(f'crs-{digest}-as-intermediate', [LEAF_CERT, root])
            add(f'crs-{digest}-also-anchor', [root], [(ANCHOR, root)], host='example.com')
            add(f'crs-{digest}-distrusted', [root], [(DISTRUSTED_SPKI, extract_spki(root))], host='example.com')


def extract_spki(cert):
    def read(data, offset):
        tag = data[offset]
        size = data[offset + 1]
        header = 2
        if size & 0x80:
            count = size & 0x7f
            size = int.from_bytes(data[offset + 2:offset + 2 + count], 'big')
            header += count
        return tag, offset + header, offset + header + size
    _, start, _ = read(cert, 0)
    _, tbs_start, _ = read(cert, start)
    offset = tbs_start
    fields = []
    while len(fields) < 7:
        tag, body, end = read(cert, offset)
        fields.append((tag, offset, end))
        offset = end
    index = 6 if fields[0][0] == 0xa0 else 5
    return cert[fields[index][1]:fields[index][2]]


def pem_blocks(path):
    return [base64.b64decode(''.join(block.split())) for block in re.findall(r'-----BEGIN CERTIFICATE-----(.*?)-----END CERTIFICATE-----', path.read_text(errors='replace'), re.S)]


BSSL_TRUST = {
    'TRUSTED_ANCHOR': ANCHOR, 'TRUSTED_ANCHOR_WITH_CONSTRAINTS': ANCHOR_ENFORCED, 'TRUSTED_ANCHOR_WITH_EXPIRATION': ANCHOR_ENFORCED,
    'TRUSTED_ANCHOR_WITH_EXPIRATION_AND_CONSTRAINTS': ANCHOR_ENFORCED, 'TRUSTED_ANCHOR_OR_LEAF': ANCHOR_OR_LEAF, 'TRUSTED_LEAF': LEAF,
    'DISTRUSTED': DISTRUSTED_SPKI, 'UNSPECIFIED': None,
}


def utc(value):
    if value == 'DEFAULT':
        return epoch(2021, 10, 5, 12, 0, 1)
    year = int(value[:2])
    return epoch(2000 + year if year < 50 else 1900 + year, int(value[2:4]), int(value[4:6]), int(value[6:8]), int(value[8:10]), int(value[10:12]))


def suite_cases():
    base = SOURCE / 'third_party/boringssl/src/pki/testdata/verify_certificate_chain_unittest'
    for test in sorted(base.glob('*/*.test')):
        fields = dict(line.split(': ', 1) for line in test.read_text().splitlines() if ': ' in line and not line.startswith(('-', ' ')))
        if fields.get('key_purpose') != 'SERVER_AUTH' or fields.get('last_cert_trust') not in BSSL_TRUST:
            continue
        chain = pem_blocks(test.parent / fields['chain'])
        kind = BSSL_TRUST[fields['last_cert_trust']]
        trust = [] if kind is None else [(kind, extract_spki(chain[-1]) if kind == DISTRUSTED_SPKI else chain[-1])]
        presented = chain[:-1] if len(chain) > 1 else chain
        label = f'suite-bssl-{test.parent.name}-{test.stem}'
        add(label, presented, trust, time=utc(fields['utc_time']))
        add(label + '-full-chain', chain, trust, time=utc(fields['utc_time']))
    path_builder = SOURCE / 'third_party/boringssl/src/pki/testdata/path_builder_unittest'
    multi = {p.stem: pem_blocks(p)[0] for p in path_builder.glob('multi-root-*.pem')}
    if multi:
        target = multi['multi-root-A-by-B']
        intermediates = [v for k, v in sorted(multi.items()) if k != 'multi-root-A-by-B']
        for trusted in ('multi-root-D-by-D', 'multi-root-E-by-E', 'multi-root-C-by-D', 'multi-root-B-by-C'):
            for time in (epoch(2017, 3, 1), epoch(2015, 1, 1)):
                add(f'suite-multiroot-{trusted}-{time}', [target, *intermediates], [(ANCHOR, multi[trusted])], time=time, host='www.example.com')
    for directory in sorted(path_builder.iterdir()) if path_builder.exists() else ():
        if not directory.is_dir() or directory.name.startswith('mtc'):
            continue
        pems = {p.stem: pem_blocks(p) for p in directory.glob('*.pem')}
        roots = [blocks[0] for stem, blocks in sorted(pems.items()) if 'root' in stem.lower()]
        others = [blocks[0] for stem, blocks in sorted(pems.items()) if 'root' not in stem.lower() and blocks]
        targets = [c for stem, blocks in sorted(pems.items()) for c in blocks if 'target' in stem.lower() or 'leaf' in stem.lower()]
        for index, target in enumerate(targets[:4]):
            add(f'suite-pathbuilder-{directory.name}-{index}', [target, *[o for o in others if o != target]], [(ANCHOR, r) for r in roots], time=epoch(2017, 3, 1), host='www.example.com')
    certificates = SOURCE / 'net/data/ssl/certificates'
    test_root = pem_blocks(certificates / 'root_ca_cert.pem')[0]
    for path in sorted(certificates.glob('*.pem')):
        blocks = pem_blocks(path)
        if not blocks:
            continue
        for time_label, time in (('now', T), ('2020', epoch(2020, 6, 1)), ('2016', epoch(2016, 6, 1))):
            for host in ('127.0.0.1', 'www.example.com'):
                add(f'suite-net-{path.stem}-{time_label}-{host}', blocks, [(ANCHOR, test_root)], time=time, host=host)


def main():
    hostname_cases()
    validity_cases()
    trust_cases()
    cross_sign_cases()
    constraint_cases()
    leaf_cases()
    name_cases()
    name_constraint_cases()
    policy_cases()
    signature_cases()
    crs_cases()
    suite_cases()
    return cases


if __name__ == '__main__':
    import struct
    from wire import encode
    out = pathlib.Path(sys.argv[2])
    seen = set()
    with open(out.with_suffix('.tmp'), 'wb') as handle:
        for label, record in main():
            if label in seen:
                raise SystemExit(f'duplicate case {label}')
            seen.add(label)
            data = encode(record)
            handle.write(struct.pack('>H', len(label)) + label.encode() + struct.pack('>I', len(data)) + data)
    out.with_suffix('.tmp').replace(out)
    print('generated', len(seen), 'cases')
