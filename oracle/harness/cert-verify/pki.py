import calendar
import hashlib
import ipaddress
import os
import pathlib
import subprocess
import tempfile
import time

HASHES = {'md5': hashlib.md5, 'sha1': hashlib.sha1, 'sha256': hashlib.sha256, 'sha384': hashlib.sha384, 'sha512': hashlib.sha512}
DIGEST_INFO = {
    'md5': bytes.fromhex('3020300c06082a864886f70d020505000410'),
    'sha1': bytes.fromhex('3021300906052b0e03021a05000414'),
    'sha256': bytes.fromhex('3031300d060960864801650304020105000420'),
    'sha384': bytes.fromhex('3041300d060960864801650304020205000430'),
    'sha512': bytes.fromhex('3051300d060960864801650304020305000440'),
}
HASH_OIDS = {'sha1': '1.3.14.3.2.26', 'sha256': '2.16.840.1.101.3.4.2.1', 'sha384': '2.16.840.1.101.3.4.2.2', 'sha512': '2.16.840.1.101.3.4.2.3'}
RSA_SIG_OIDS = {'md5': '1.2.840.113549.1.1.4', 'sha1': '1.2.840.113549.1.1.5', 'sha256': '1.2.840.113549.1.1.11', 'sha384': '1.2.840.113549.1.1.12', 'sha512': '1.2.840.113549.1.1.13'}
ECDSA_SIG_OIDS = {'sha1': '1.2.840.10045.4.1', 'sha256': '1.2.840.10045.4.3.2', 'sha384': '1.2.840.10045.4.3.3', 'sha512': '1.2.840.10045.4.3.4'}


def length(n):
    if n < 0x80:
        return bytes([n])
    body = n.to_bytes((n.bit_length() + 7) // 8, 'big')
    return bytes([0x80 | len(body)]) + body


def tlv(tag, content):
    return bytes([tag]) + length(len(content)) + content


def seq(*items):
    return tlv(0x30, b''.join(items))


def set_of(*items):
    return tlv(0x31, b''.join(items))


def integer(value):
    if value == 0:
        return tlv(0x02, b'\0')
    size = (value.bit_length() + 8) // 8 if value > 0 else ((-value - 1).bit_length() + 8) // 8
    return tlv(0x02, value.to_bytes(size, 'big', signed=True))


def raw_integer(content):
    return tlv(0x02, content)


def oid_body(dotted):
    parts = [int(p) for p in dotted.split('.')]
    out = bytearray()
    for index, value in enumerate([parts[0] * 40 + parts[1]] + parts[2:]):
        chunk = [value & 0x7f]
        value >>= 7
        while value:
            chunk.append(0x80 | (value & 0x7f))
            value >>= 7
        out += bytes(reversed(chunk))
    return bytes(out)


def oid(dotted):
    return tlv(0x06, oid_body(dotted))


def bitstring(data, unused=0):
    return tlv(0x03, bytes([unused]) + data)


def octets(data):
    return tlv(0x04, data)


def null():
    return b'\x05\x00'


def boolean(value):
    return tlv(0x01, b'\xff' if value else b'\x00')


def utf8(text):
    return tlv(0x0c, text.encode())


def printable(text):
    return tlv(0x13, text.encode())


def ia5(text):
    return tlv(0x16, text.encode() if isinstance(text, str) else text)


def explicit(number, content):
    return tlv(0xa0 | number, content)


def implicit(number, content, constructed=False):
    return tlv((0xa0 if constructed else 0x80) | number, content)


def utc_time(seconds):
    return tlv(0x17, time.strftime('%y%m%d%H%M%SZ', time.gmtime(seconds)).encode())


def generalized_time(seconds):
    return tlv(0x18, time.strftime('%Y%m%d%H%M%SZ', time.gmtime(seconds)).encode())


def cert_time(seconds):
    year = time.gmtime(seconds).tm_year
    return utc_time(seconds) if 1950 <= year < 2050 else generalized_time(seconds)


def epoch(year, month=1, day=1, hour=0, minute=0, second=0):
    return calendar.timegm((year, month, day, hour, minute, second, 0, 0, 0))


def name(*attributes):
    rdns = []
    for attribute in attributes:
        if isinstance(attribute, bytes):
            rdns.append(attribute)
            continue
        kind, value = attribute
        oids = {'CN': '2.5.4.3', 'O': '2.5.4.10', 'OU': '2.5.4.11', 'C': '2.5.4.6', 'L': '2.5.4.7', 'ST': '2.5.4.8', 'E': '1.2.840.113549.1.9.1', 'DC': '0.9.2342.19200300.100.1.25'}
        encoded = value if isinstance(value, bytes) else (printable(value) if kind == 'C' else ia5(value) if kind in ('E', 'DC') else utf8(value))
        rdns.append(set_of(seq(oid(oids[kind]), encoded)))
    return seq(*rdns)


def cn(text):
    return name(('CN', text))


KEY_DIR = pathlib.Path(__file__).resolve().parent / 'keys'
SCRATCH = pathlib.Path(os.environ.get('TMPDIR', '/tmp')) / 'cert-verify-signing'
CURVE_NAMES = {'P-224': 'secp224r1', 'P-256': 'prime256v1', 'P-384': 'secp384r1', 'P-521': 'secp521r1'}


def openssl(*args, data=None):
    return subprocess.run(['openssl', *args], input=data, stdout=subprocess.PIPE, check=True).stdout


class Key:
    def __init__(self, kind, label, bits=2048, curve='P-256', exponent=65537):
        self.kind, self.bits, self.curve = kind, bits, curve
        suffix = f'rsa{bits}' + ('' if exponent == 65537 else f'e{exponent}') if kind == 'rsa' else curve.lower().replace('-', '') if kind == 'ec' else kind
        self.path = KEY_DIR / f'{label}-{suffix}.pem'
        if not self.path.exists():
            KEY_DIR.mkdir(exist_ok=True)
            if kind == 'rsa':
                pem = openssl('genpkey', '-quiet', '-algorithm', 'RSA', '-pkeyopt', f'rsa_keygen_bits:{bits}', '-pkeyopt', f'rsa_keygen_pubexp:{exponent}')
            elif kind == 'ec':
                pem = openssl('genpkey', '-quiet', '-algorithm', 'EC', '-pkeyopt', f'ec_paramgen_curve:{CURVE_NAMES[curve]}', '-pkeyopt', 'ec_param_enc:named_curve')
            else:
                pem = openssl('genpkey', '-quiet', '-algorithm', 'ED25519')
            staged = self.path.with_suffix('.tmp')
            staged.write_bytes(pem)
            os.replace(staged, self.path)
        self.public = openssl('pkey', '-in', str(self.path), '-pubout', '-outform', 'DER')

    def spki(self, algorithm=None):
        if algorithm is None:
            return self.public
        body = self.public[2 + (self.public[1] & 0x7f if self.public[1] & 0x80 else 0):]
        old_algorithm_length = 2 + body[1]
        return seq(algorithm, body[old_algorithm_length:])

    def modulus_size(self):
        return (self.bits + 7) // 8

    def raw_rsa(self, block):
        return openssl('pkeyutl', '-decrypt', '-inkey', str(self.path), '-pkeyopt', 'rsa_padding_mode:none', data=block)

    def sign_pkcs1(self, digest_name, message):
        if digest_name == 'md5':
            digest = DIGEST_INFO['md5'] + hashlib.md5(message).digest()
            size = self.modulus_size()
            return self.raw_rsa(b'\x00\x01' + b'\xff' * (size - len(digest) - 3) + b'\x00' + digest)
        return openssl('dgst', '-' + digest_name, '-sign', str(self.path), data=message)

    def sign_pss(self, digest_name, message, salt_length, mgf_digest=None):
        hasher = HASHES[digest_name]
        m_hash = hasher(message).digest()
        salt = (hashlib.sha512(b'pss-salt' + message).digest() * 4)[:salt_length]
        em_bits = self.bits - 1
        em_len = (em_bits + 7) // 8
        h = hasher(b'\0' * 8 + m_hash + salt).digest()
        db = b'\0' * (em_len - len(salt) - len(h) - 2) + b'\x01' + salt
        mask = mgf1(h, len(db), HASHES[mgf_digest or digest_name])
        masked = bytearray(a ^ b for a, b in zip(db, mask))
        masked[0] &= 0xff >> (8 * em_len - em_bits)
        block = bytes(masked) + h + b'\xbc'
        return self.raw_rsa(b'\0' * (self.modulus_size() - len(block)) + block)

    def sign_ecdsa(self, digest_name, message):
        return openssl('dgst', '-' + digest_name, '-sigopt', 'nonce-type:1', '-sign', str(self.path), data=message)

    def sign_ed25519(self, message):
        SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(dir=SCRATCH) as staged:
            staged.write(message)
            staged.flush()
            return openssl('pkeyutl', '-sign', '-rawin', '-inkey', str(self.path), '-in', staged.name)


def mgf1(seed, size, hasher):
    out = b''
    counter = 0
    while len(out) < size:
        out += hasher(seed + counter.to_bytes(4, 'big')).digest()
        counter += 1
    return out[:size]


def rsa_algorithm(digest_name, params=True):
    return seq(oid(RSA_SIG_OIDS[digest_name]), null()) if params else seq(oid(RSA_SIG_OIDS[digest_name]))


def pss_algorithm(digest_name, salt_length, mgf_digest=None, trailer=None):
    mgf_digest = mgf_digest or digest_name
    hash_alg = seq(oid(HASH_OIDS[digest_name]), null())
    mgf = seq(oid('1.2.840.113549.1.1.8'), seq(oid(HASH_OIDS[mgf_digest]), null()))
    fields = explicit(0, hash_alg) + explicit(1, mgf) + explicit(2, integer(salt_length))
    if trailer is not None:
        fields += explicit(3, integer(trailer))
    return seq(oid('1.2.840.113549.1.1.10'), seq(fields))


def ecdsa_algorithm(digest_name):
    return seq(oid(ECDSA_SIG_OIDS[digest_name]))


class Signature:
    def __init__(self, kind, digest='sha256', salt=32, mgf=None, algorithm=None, corrupt=False):
        self.kind, self.digest, self.salt, self.mgf, self.corrupt = kind, digest, salt, mgf, corrupt
        if algorithm is not None:
            self.algorithm = algorithm
        elif kind == 'rsa':
            self.algorithm = rsa_algorithm(digest)
        elif kind == 'pss':
            self.algorithm = pss_algorithm(digest, salt, mgf)
        elif kind == 'ecdsa':
            self.algorithm = ecdsa_algorithm(digest)
        else:
            self.algorithm = seq(oid('1.3.101.112'))

    def sign(self, key, message):
        if self.kind == 'rsa':
            value = key.sign_pkcs1(self.digest, message)
        elif self.kind == 'pss':
            value = key.sign_pss(self.digest, message, self.salt, self.mgf)
        elif self.kind == 'ecdsa':
            value = key.sign_ecdsa(self.digest, message)
        else:
            value = key.sign_ed25519(message)
        if self.corrupt:
            value = value[:-1] + bytes([value[-1] ^ 1])
        return value


def default_signature(key, digest='sha256'):
    if key.kind == 'rsa':
        return Signature('rsa', digest)
    if key.kind == 'ec':
        return Signature('ecdsa', digest)
    return Signature('ed25519')


def extension(dotted, value, critical=False):
    return seq(oid(dotted), *( [boolean(True)] if critical else []), octets(value))


def basic_constraints(ca, path_len=None, critical=True):
    body = (boolean(True) if ca else b'') + (integer(path_len) if path_len is not None else b'')
    return extension('2.5.29.19', seq(body) if body else seq(), critical)


KEY_USAGE_BITS = {'digitalSignature': 0, 'nonRepudiation': 1, 'keyEncipherment': 2, 'dataEncipherment': 3, 'keyAgreement': 4, 'keyCertSign': 5, 'cRLSign': 6, 'encipherOnly': 7, 'decipherOnly': 8}


def key_usage(*names, critical=True):
    bits = 0
    top = -1
    for item in names:
        bit = KEY_USAGE_BITS[item]
        bits |= 1 << bit
        top = max(top, bit)
    if top < 0:
        return extension('2.5.29.15', bitstring(b''), critical)
    size = top // 8 + 1
    value = 0
    for bit in range(size * 8):
        if bits >> bit & 1:
            value |= 1 << (size * 8 - 1 - bit)
    unused = size * 8 - 1 - top
    return extension('2.5.29.15', bitstring(value.to_bytes(size, 'big'), unused), critical)


EKU = {'serverAuth': '1.3.6.1.5.5.7.3.1', 'clientAuth': '1.3.6.1.5.5.7.3.2', 'codeSigning': '1.3.6.1.5.5.7.3.3', 'emailProtection': '1.3.6.1.5.5.7.3.4',
       'timeStamping': '1.3.6.1.5.5.7.3.8', 'OCSPSigning': '1.3.6.1.5.5.7.3.9', 'any': '2.5.29.37.0', 'serverGatedCrypto': '2.16.840.1.113730.4.1', 'msSGC': '1.3.6.1.4.1.311.10.3.3'}


def eku(*purposes, critical=False):
    return extension('2.5.29.37', seq(*(oid(EKU.get(p, p)) for p in purposes)), critical)


def ip_bytes(text):
    return ipaddress.ip_address(text).packed


def general_name(kind, value):
    if kind == 'dns':
        return implicit(2, value.encode() if isinstance(value, str) else value)
    if kind == 'ip':
        return implicit(7, ip_bytes(value) if isinstance(value, str) else value)
    if kind == 'email':
        return implicit(1, value.encode())
    if kind == 'uri':
        return implicit(6, value.encode())
    if kind == 'dir':
        return implicit(4, value, constructed=True)
    if kind == 'raw':
        return value
    raise ValueError(kind)


def san(*names, critical=False):
    return extension('2.5.29.17', seq(*(general_name(k, v) for k, v in names)), critical)


def name_constraints(permitted=(), excluded=(), critical=True):
    def subtrees(items):
        out = b''
        for kind, value in items:
            if kind == 'ip':
                address, prefix = value.split('/')
                raw = ip_bytes(address)
                bits = int(prefix)
                mask = ((1 << (len(raw) * 8)) - 1) ^ ((1 << (len(raw) * 8 - bits)) - 1)
                out += seq(implicit(7, raw + mask.to_bytes(len(raw), 'big')))
            else:
                out += seq(general_name(kind, value))
        return out
    body = b''
    if permitted:
        body += implicit(0, subtrees(permitted), constructed=True)
    if excluded:
        body += implicit(1, subtrees(excluded), constructed=True)
    return extension('2.5.29.30', seq(body), critical)


def policies(*oids, critical=False, qualifier=None):
    items = []
    for dotted in oids:
        if qualifier is not None:
            items.append(seq(oid(dotted), seq(qualifier)))
        else:
            items.append(seq(oid(dotted)))
    return extension('2.5.29.32', seq(*items), critical)


def policy_mappings(*pairs, critical=True):
    return extension('2.5.29.33', seq(*(seq(oid(a), oid(b)) for a, b in pairs)), critical)


def policy_constraints(require=None, inhibit=None, critical=True):
    body = (implicit(0, integer(require)[2:]) if require is not None else b'') + (implicit(1, integer(inhibit)[2:]) if inhibit is not None else b'')
    return extension('2.5.29.36', seq(body), critical)


def inhibit_any_policy(skip, critical=True):
    return extension('2.5.29.54', integer(skip), critical)


def ski(key_id):
    return extension('2.5.29.14', octets(key_id))


def aki(key_id=None, issuer=None, serial=None):
    body = (implicit(0, key_id) if key_id is not None else b'') + (implicit(1, issuer, constructed=True) if issuer is not None else b'') + (implicit(2, integer(serial)[2:]) if serial is not None else b'')
    return extension('2.5.29.35', seq(body))


def key_id(key):
    return hashlib.sha1(key.spki()).digest()


class Certificate:
    def __init__(self, der, key, subject):
        self.der, self.key, self.subject = der, key, subject


def certificate(subject, key, issuer=None, issuer_key=None, serial=1, not_before=0, not_after=0, extensions=(),
                signature=None, outer_algorithm=None, version=3, spki=None, not_before_tlv=None, not_after_tlv=None,
                issuer_unique_id=None, subject_unique_id=None, tbs_override=None, extra_tbs=b''):
    issuer = subject if issuer is None else issuer
    issuer_key = key if issuer_key is None else issuer_key
    signature = signature or default_signature(issuer_key)
    validity = seq(not_before_tlv or cert_time(not_before), not_after_tlv or cert_time(not_after))
    fields = b''
    if version != 1:
        fields += explicit(0, integer(version - 1))
    fields += (integer(serial) if isinstance(serial, int) else raw_integer(serial))
    fields += signature.algorithm + issuer + validity + subject + (spki if spki is not None else key.spki())
    if issuer_unique_id is not None:
        fields += implicit(1, b'\0' + issuer_unique_id)
    if subject_unique_id is not None:
        fields += implicit(2, b'\0' + subject_unique_id)
    if extensions:
        fields += explicit(3, seq(*extensions))
    fields += extra_tbs
    tbs = tbs_override if tbs_override is not None else seq(fields)
    value = signature.sign(issuer_key, tbs)
    return Certificate(seq(tbs, outer_algorithm if outer_algorithm is not None else signature.algorithm, bitstring(value)), key, subject)
