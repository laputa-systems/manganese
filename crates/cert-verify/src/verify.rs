use std::rc::Rc;

use crate::cert;
use crate::cert::ParsedCertificate;
use crate::ct;
use crate::der;
use crate::der::GeneralizedTime;
use crate::der::Parser;
use crate::errors::CertError;
use crate::hostname;
use crate::keys;
use crate::names;
use crate::names::GeneralNames;
use crate::names::NameConstraints;
use crate::path;
use crate::path::BuildResult;
use crate::path::CtCompliance;
use crate::path::ResultPath;
use crate::path::Sources;
use crate::path::TrustStoreInMemory;
use crate::path::TrustStores;
use crate::signature::SignatureAlgorithm;
use crate::trust::Trust;
use crate::trust_data::RootConstraint;
use crate::trust_data::TrustData;

pub const OK: i32 = 0;
pub const ERR_SSL_SERVER_CERT_BAD_FORMAT: i32 = -167;
const ERR_CERT_COMMON_NAME_INVALID: i32 = -200;
const ERR_CERT_DATE_INVALID: i32 = -201;
const ERR_CERT_AUTHORITY_INVALID: i32 = -202;
const ERR_CERT_NO_REVOCATION_MECHANISM: i32 = -204;
const ERR_CERT_UNABLE_TO_CHECK_REVOCATION: i32 = -205;
const ERR_CERT_REVOKED: i32 = -206;
const ERR_CERT_INVALID: i32 = -207;
const ERR_CERT_WEAK_SIGNATURE_ALGORITHM: i32 = -208;
const ERR_CERT_NON_UNIQUE_NAME: i32 = -210;
const ERR_CERT_WEAK_KEY: i32 = -211;
const ERR_CERT_NAME_CONSTRAINT_VIOLATION: i32 = -212;
const ERR_CERT_VALIDITY_TOO_LONG: i32 = -213;
const ERR_CERTIFICATE_TRANSPARENCY_REQUIRED: i32 = -214;
const ERR_CERT_KNOWN_INTERCEPTION_BLOCKED: i32 = -217;
const ERR_SSL_PINNED_KEY_NOT_IN_CERT_CHAIN: i32 = -150;

pub const STATUS_COMMON_NAME_INVALID: u32 = 1 << 0;
pub const STATUS_DATE_INVALID: u32 = 1 << 1;
pub const STATUS_AUTHORITY_INVALID: u32 = 1 << 2;
const STATUS_NO_REVOCATION_MECHANISM: u32 = 1 << 4;
const STATUS_UNABLE_TO_CHECK_REVOCATION: u32 = 1 << 5;
const STATUS_REVOKED: u32 = 1 << 6;
pub const STATUS_INVALID: u32 = 1 << 7;
const STATUS_WEAK_SIGNATURE_ALGORITHM: u32 = 1 << 8;
const STATUS_NON_UNIQUE_NAME: u32 = 1 << 10;
const STATUS_WEAK_KEY: u32 = 1 << 11;
const STATUS_PINNED_KEY_MISSING: u32 = 1 << 13;
const STATUS_NAME_CONSTRAINT_VIOLATION: u32 = 1 << 14;
const STATUS_VALIDITY_TOO_LONG: u32 = 1 << 15;
const STATUS_KNOWN_INTERCEPTION_DETECTED: u32 = 1 << 21;
const STATUS_CERTIFICATE_TRANSPARENCY_REQUIRED: u32 = 1 << 24;
const STATUS_KNOWN_INTERCEPTION_BLOCKED: u32 = 1 << 26;
const ERROR_STATUSES: u32 = STATUS_COMMON_NAME_INVALID
    | STATUS_DATE_INVALID
    | STATUS_AUTHORITY_INVALID
    | STATUS_NO_REVOCATION_MECHANISM
    | STATUS_UNABLE_TO_CHECK_REVOCATION
    | STATUS_REVOKED
    | STATUS_INVALID
    | STATUS_WEAK_SIGNATURE_ALGORITHM
    | STATUS_NON_UNIQUE_NAME
    | STATUS_WEAK_KEY
    | STATUS_PINNED_KEY_MISSING
    | STATUS_NAME_CONSTRAINT_VIOLATION
    | STATUS_VALIDITY_TOO_LONG
    | STATUS_CERTIFICATE_TRANSPARENCY_REQUIRED
    | STATUS_KNOWN_INTERCEPTION_BLOCKED;

const PATH_BUILDER_ITERATION_LIMIT: u32 = 20;
const CT_TIMELY_DAYS: i64 = 70;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TrustKind {
    Anchor,
    AnchorWithEnforcedConstraints,
    AnchorWithConstraints,
    Leaf,
    AnchorOrLeaf,
    UntrustedAuthority,
    DistrustedSpki,
}

pub struct TrustEntry {
    pub kind: TrustKind,
    pub der: Vec<u8>,
    pub dns_names: Vec<Vec<u8>>,
    pub cidrs: Vec<(Vec<u8>, Vec<u8>)>,
}

pub struct Outcome {
    pub error: i32,
    pub status: u32,
    pub ct: CtCompliance,
    pub chain: Vec<Vec<u8>>,
}

fn is_error(status: u32) -> bool {
    status & ERROR_STATUSES != 0
}

pub fn map_status(status: u32) -> i32 {
    if status & STATUS_INVALID != 0 {
        return ERR_CERT_INVALID;
    }
    if status & STATUS_PINNED_KEY_MISSING != 0 {
        return ERR_SSL_PINNED_KEY_NOT_IN_CERT_CHAIN;
    }
    for (flag, error) in [
        (STATUS_KNOWN_INTERCEPTION_BLOCKED, ERR_CERT_KNOWN_INTERCEPTION_BLOCKED),
        (STATUS_REVOKED, ERR_CERT_REVOKED),
        (STATUS_AUTHORITY_INVALID, ERR_CERT_AUTHORITY_INVALID),
        (STATUS_COMMON_NAME_INVALID, ERR_CERT_COMMON_NAME_INVALID),
        (STATUS_CERTIFICATE_TRANSPARENCY_REQUIRED, ERR_CERTIFICATE_TRANSPARENCY_REQUIRED),
        (STATUS_NAME_CONSTRAINT_VIOLATION, ERR_CERT_NAME_CONSTRAINT_VIOLATION),
        (STATUS_WEAK_SIGNATURE_ALGORITHM, ERR_CERT_WEAK_SIGNATURE_ALGORITHM),
        (STATUS_WEAK_KEY, ERR_CERT_WEAK_KEY),
        (STATUS_DATE_INVALID, ERR_CERT_DATE_INVALID),
        (STATUS_VALIDITY_TOO_LONG, ERR_CERT_VALIDITY_TOO_LONG),
        (STATUS_NON_UNIQUE_NAME, ERR_CERT_NON_UNIQUE_NAME),
        (STATUS_UNABLE_TO_CHECK_REVOCATION, ERR_CERT_UNABLE_TO_CHECK_REVOCATION),
        (STATUS_NO_REVOCATION_MECHANISM, ERR_CERT_NO_REVOCATION_MECHANISM),
    ] {
        if status & flag != 0 {
            return error;
        }
    }
    unreachable!("only error statuses are mapped to net errors")
}

struct X509Fields {
    common_name: Vec<u8>,
    not_before: i64,
    not_after: i64,
}

fn principal_common_name(name_tlv: &[u8]) -> Option<Vec<u8>> {
    let mut parser = Parser::new(name_tlv);
    let value = parser.read_tag(der::SEQUENCE)?;
    let mut common_name = Vec::new();
    const PRINCIPAL_OIDS: [&[u8]; 6] = [&[0x55, 0x04, 0x03], &[0x55, 0x04, 0x07], &[0x55, 0x04, 0x08], &[0x55, 0x04, 0x06], &[0x55, 0x04, 0x0a], &[0x55, 0x04, 0x0b]];
    let mut seen_single = [false; 4];
    for rdn in names::parse_name_value(value)? {
        for attribute in rdn {
            let Some(index) = PRINCIPAL_OIDS.iter().position(|oid| *oid == attribute.oid) else { continue };
            if index < 4 {
                if seen_single[index] {
                    continue;
                }
                let parsed = attribute.value_as_string()?;
                seen_single[index] = !parsed.is_empty();
                if index == 0 {
                    common_name = parsed;
                }
            } else {
                attribute.value_as_string()?;
            }
        }
    }
    Some(common_name)
}

fn x509_fields(der: &[u8]) -> Option<X509Fields> {
    let outer = cert::parse_outer(der)?;
    let tbs = cert::parse_tbs(outer.tbs_tlv)?;
    let common_name = principal_common_name(tbs.subject_tlv)?;
    principal_common_name(tbs.issuer_tlv)?;
    Some(X509Fields { common_name, not_before: tbs.not_before.to_posix(), not_after: tbs.not_after.to_posix() })
}

fn subject_alt_names(der: &[u8]) -> Option<GeneralNames> {
    let outer = cert::parse_outer(der)?;
    let tbs = cert::parse_tbs(outer.tbs_tlv)?;
    let extensions = cert::parse_extensions(tbs.extensions_tlv?)?;
    GeneralNames::parse(&extensions.get(cert::OID_SUBJECT_ALT_NAME)?.value)
}

struct CrsAnchorData {
    cert: Rc<ParsedCertificate>,
    constraints: Vec<RootConstraint>,
}

pub struct Verifier {
    data: TrustData,
    chrome: TrustStoreInMemory,
    crs_constraints: Vec<CrsAnchorData>,
}

struct AdditionalConstraint {
    cert: Rc<ParsedCertificate>,
    dns_names: Vec<Vec<u8>>,
    cidrs: Vec<(Vec<u8>, Vec<u8>)>,
}

struct Context<'a> {
    verifier: &'a Verifier,
    additional: TrustStoreInMemory,
    constraints: Vec<AdditionalConstraint>,
    time: i64,
    scts: &'a [u8],
    ocsp: &'a [u8],
}

fn compare_versions(a: &[u32], b: &[u32]) -> std::cmp::Ordering {
    let length = a.len().max(b.len());
    (0..length).map(|i| a.get(i).copied().unwrap_or(0).cmp(&b.get(i).copied().unwrap_or(0))).find(|o| o.is_ne()).unwrap_or(std::cmp::Ordering::Equal)
}

fn generalized(micros: i64) -> Option<GeneralizedTime> {
    GeneralizedTime::from_posix(micros.div_euclid(1_000_000))
}

impl Verifier {
    pub fn new(data: TrustData) -> Self {
        let mut chrome = TrustStoreInMemory::default();
        let mut crs_constraints = Vec::new();
        for anchor in &data.anchors {
            chrome.add(anchor.cert.clone(), Trust::ANCHOR.enforcing(anchor.enforce_expiry, anchor.enforce_constraints));
            if !anchor.constraints.is_empty() {
                crs_constraints.push(CrsAnchorData { cert: anchor.cert.clone(), constraints: anchor.constraints.clone() });
            }
        }
        Self { data, chrome, crs_constraints }
    }

    fn is_known_root(&self, cert: &ParsedCertificate) -> bool {
        self.chrome.contains(cert)
    }

    fn ct_timely(&self, time: i64) -> bool {
        let delta = time * 1_000_000 - self.data.ct_log_list_micros;
        delta / (86_400 * 1_000_000) < CT_TIMELY_DAYS
    }

    fn crs_constraints_for(&self, cert: &ParsedCertificate) -> &[RootConstraint] {
        self.crs_constraints.iter().find(|a| a.cert.der == cert.der).map_or(&[], |a| a.constraints.as_slice())
    }

    pub fn verify(&self, time: i64, hostname: &[u8], chain: &[Vec<u8>], trust: &[TrustEntry], scts: &[u8], ocsp: &[u8]) -> Outcome {
        let host = std::str::from_utf8(hostname).ok();
        let Some(target_fields) = x509_fields(&chain[0]) else {
            return Outcome { error: ERR_SSL_SERVER_CERT_BAD_FORMAT, status: 0, ct: CtCompliance::DetailsNotAvailable, chain: Vec::new() };
        };
        let context = Context::new(self, trust, time, scts, ocsp);
        let mut status = 0u32;
        let mut ct = CtCompliance::DetailsNotAvailable;
        let mut verified: Vec<Vec<u8>> = chain.to_vec();
        let mut public_key_hashes: Vec<[u8; 32]> = Vec::new();
        let mut known_root = false;
        let mut error = match ParsedCertificate::parse(&chain[0]) {
            None => {
                status |= STATUS_INVALID;
                ERR_CERT_INVALID
            }
            Some(target) => {
                let intermediates: Vec<Rc<ParsedCertificate>> = chain[1..].iter().filter_map(|d| ParsedCertificate::parse(d)).map(Rc::new).collect();
                let result = context.build(Rc::new(target), &intermediates);
                match result.best_path() {
                    None => {
                        status |= STATUS_AUTHORITY_INVALID;
                        ERR_CERT_AUTHORITY_INVALID
                    }
                    Some(best) => {
                        public_key_hashes = best.certs.iter().map(|c| keys::sha256(&c.spki_tlv)).collect();
                        if let Some(trusted) = best.trusted_cert() {
                            known_root = self.is_known_root(trusted);
                        }
                        verified = std::iter::once(chain[0].clone()).chain(best.certs[1..].iter().map(|c| c.der.clone())).collect();
                        status |= map_path_errors(best);
                        ct = best.delegate.ct_compliance;
                        if is_error(status) { map_status(status) } else { OK }
                    }
                }
            }
        };
        if !inspect_signature_algorithms(&verified) {
            status |= STATUS_INVALID;
            error = map_status(status);
        }
        let alt_names = subject_alt_names(&chain[0]);
        let empty = GeneralNames::default();
        let alt = alt_names.as_ref().unwrap_or(&empty);
        if !host.is_some_and(|h| hostname::verify_hostname(h, &alt.dns, &alt.ip)) {
            status |= STATUS_COMMON_NAME_INVALID;
            error = map_status(status);
        }
        if public_key_hashes.iter().any(|h| self.data.interception_spkis.contains(h)) {
            if status & STATUS_REVOKED != 0 {
                status |= STATUS_KNOWN_INTERCEPTION_BLOCKED;
                error = map_status(status);
            } else {
                status |= STATUS_KNOWN_INTERCEPTION_DETECTED;
            }
        }
        if name_constraints_violation(&public_key_hashes, &target_fields.common_name, &alt.dns, &alt.ip) {
            status |= STATUS_NAME_CONSTRAINT_VIOLATION;
            error = map_status(status);
        }
        if verified.iter().any(|der| has_weak_key(der)) {
            status |= STATUS_WEAK_KEY;
            if error == OK || is_certificate_error(error) {
                error = map_status(status);
            }
        }
        if known_root && has_too_long_validity(&target_fields) {
            status |= STATUS_VALIDITY_TOO_LONG;
            if error == OK {
                error = map_status(status);
            }
        }
        if known_root && host.is_some_and(hostname::is_non_unique) {
            status |= STATUS_NON_UNIQUE_NAME;
            if error == OK {
                error = map_status(status);
            }
        }
        Outcome { error, status, ct, chain: verified }
    }
}

fn is_certificate_error(error: i32) -> bool {
    (-219..=-200).contains(&error)
}

fn extract_spki(der: &[u8]) -> Option<&[u8]> {
    let mut parser = Parser::new(der);
    let mut certificate = parser.read_sequence()?;
    if parser.has_more() {
        return None;
    }
    let mut tbs = certificate.read_sequence()?;
    tbs.read_optional_tag(der::context_constructed(0))?;
    for tag in [der::INTEGER, der::SEQUENCE, der::SEQUENCE, der::SEQUENCE, der::SEQUENCE] {
        tbs.read_tag(tag)?;
    }
    tbs.read_raw_tlv()
}

fn has_weak_key(der: &[u8]) -> bool {
    let Some(spki) = extract_spki(der) else { return false };
    matches!(keys::parse_public_key(spki), Some(keys::PublicKey::Rsa { bits }) if bits < 1024)
}

fn extract_signature_algorithms(der: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut parser = Parser::new(der);
    let mut certificate = parser.read_sequence()?;
    let mut tbs = certificate.read_sequence()?;
    tbs.read_optional_tag(der::context_constructed(0))?;
    tbs.read_tag(der::INTEGER)?;
    let tbs_algorithm = tbs.read_raw_tlv()?;
    let outer_algorithm = certificate.read_raw_tlv()?;
    Some((outer_algorithm, tbs_algorithm))
}

fn inspect_signature_algorithms(verified: &[Vec<u8>]) -> bool {
    if verified.len() <= 1 {
        return true;
    }
    verified[..verified.len() - 1].iter().all(|der| {
        let Some((outer, inner)) = extract_signature_algorithms(der) else { return false };
        match (SignatureAlgorithm::parse(outer), SignatureAlgorithm::parse(inner)) {
            (Some(a), Some(b)) if a == b => !a.is_sha1(),
            _ => false,
        }
    })
}

fn has_too_long_validity(fields: &X509Fields) -> bool {
    const NULL_TIME: i64 = -11_644_473_600;
    let (start, expiry) = (fields.not_before, fields.not_after);
    if start == NULL_TIME || expiry == NULL_TIME || start > expiry {
        return true;
    }
    let duration = expiry - start;
    let days = |n: i64| n * 86_400;
    if start >= 1_868_227_200 {
        return duration > days(47);
    }
    if start >= 1_805_068_800 {
        return duration > days(100);
    }
    if start >= 1_773_532_800 {
        return duration > days(200);
    }
    duration > days(398)
}

const ANSSI_SPKI: [u8; 32] = [
    0x86, 0xc1, 0x3a, 0x34, 0x08, 0xdd, 0x1a, 0xa7, 0x7e, 0xe8, 0xb6, 0x94, 0x7c, 0x03, 0x95, 0x87, 0x72, 0xf5, 0x31, 0x24, 0x8c, 0x16, 0x27, 0xbe,
    0xfb, 0x2c, 0x4f, 0x4b, 0x04, 0xd0, 0x44, 0x96,
];
const TEST_CONSTRAINED_SPKI: [u8; 32] = [
    0xa2, 0x2a, 0x88, 0x82, 0xba, 0x0c, 0xae, 0x9d, 0xf2, 0xc4, 0x5b, 0x15, 0xa6, 0x1e, 0xfd, 0xfd, 0x19, 0x6b, 0xb1, 0x09, 0x19, 0xfd, 0xac, 0x77,
    0x9b, 0xd6, 0x08, 0x66, 0xda, 0xa8, 0xd2, 0x88,
];
const ANSSI_DOMAINS: [&str; 13] = [".fr", ".gp", ".gf", ".mq", ".re", ".yt", ".pm", ".bl", ".mf", ".wf", ".pf", ".nc", ".tf"];

fn names_within(dns_names: &[Vec<u8>], domains: &[&str]) -> bool {
    dns_names.iter().all(|host| {
        let Ok(host) = std::str::from_utf8(host) else { return true };
        let name = match hostname::parse_host(host) {
            hostname::Host::Ip(_) => return true,
            hostname::Host::Invalid => return true,
            hostname::Host::Name(name) => name,
        };
        if !hostname::has_registry_controlled_domain(&name, hostname::Unknown::Exclude, hostname::Private::Include) {
            return true;
        }
        domains.iter().any(|domain| name.len() > domain.len() && name[name.len() - domain.len()..].eq_ignore_ascii_case(domain))
    })
}

fn name_constraints_violation(hashes: &[[u8; 32]], common_name: &[u8], dns_names: &[Vec<u8>], ips: &[Vec<u8>]) -> bool {
    for (key, domains) in [(ANSSI_SPKI, &ANSSI_DOMAINS[..]), (TEST_CONSTRAINED_SPKI, &[".example.com"][..])] {
        for hash in hashes {
            if *hash != key {
                continue;
            }
            let names = if dns_names.is_empty() && ips.is_empty() { vec![common_name.to_vec()] } else { dns_names.to_vec() };
            if !names_within(&names, domains) {
                return true;
            }
        }
    }
    false
}

fn map_path_errors(path: &ResultPath) -> u32 {
    let errors = &path.errors;
    let mut status = 0;
    if !errors.has_high_severity() {
        return status;
    }
    if errors.contains(CertError::CertificateRevoked) {
        status |= STATUS_REVOKED;
    }
    if errors.contains(CertError::UnacceptablePublicKey) {
        status |= STATUS_WEAK_KEY;
    }
    if errors.contains(CertError::ValidityFailedNotAfter) || errors.contains(CertError::ValidityFailedNotBefore) {
        status |= STATUS_DATE_INVALID;
    }
    if [
        CertError::DistrustedByTrustStore,
        CertError::VerifySignedDataFailed,
        CertError::NoIssuersFound,
        CertError::SubjectDoesNotMatchIssuer,
        CertError::IterationLimitExceeded,
        CertError::ChromeRootConstraintsFailed,
    ]
    .into_iter()
    .any(|e| errors.contains(e))
    {
        status |= STATUS_AUTHORITY_INVALID;
    }
    if errors.contains(CertError::CtRequirementsNotMet) {
        status |= STATUS_CERTIFICATE_TRANSPARENCY_REQUIRED;
    }
    if !is_error(status) {
        status |= STATUS_INVALID;
    }
    status
}

impl<'a> Context<'a> {
    fn new(verifier: &'a Verifier, trust: &[TrustEntry], time: i64, scts: &'a [u8], ocsp: &'a [u8]) -> Self {
        let parse = |der: &[u8]| ParsedCertificate::parse(der).map(Rc::new);
        let x509_valid = |der: &[u8]| x509_fields(der).is_some();
        let mut additional = TrustStoreInMemory::default();
        let mut constraints = Vec::new();
        let enforced = Trust::ANCHOR.enforcing(true, true);
        let of = |kind: TrustKind| trust.iter().filter(move |e| e.kind == kind);
        for entry in of(TrustKind::DistrustedSpki) {
            additional.add_distrusted_spki(entry.der.clone());
        }
        for entry in of(TrustKind::AnchorWithConstraints) {
            if let Some(cert) = parse(&entry.der) {
                additional.add(cert.clone(), enforced);
                constraints.push(AdditionalConstraint { cert, dns_names: entry.dns_names.clone(), cidrs: entry.cidrs.clone() });
            }
        }
        for (kind, level) in [(TrustKind::Leaf, Trust::LEAF), (TrustKind::AnchorOrLeaf, Trust::ANCHOR_OR_LEAF.enforcing(true, true))] {
            for entry in of(kind) {
                let Some(cert) = parse(&entry.der) else { continue };
                if additional.contains(&cert) {
                    continue;
                }
                if !entry.dns_names.is_empty() || !entry.cidrs.is_empty() {
                    constraints.push(AdditionalConstraint { cert: cert.clone(), dns_names: entry.dns_names.clone(), cidrs: entry.cidrs.clone() });
                }
                additional.add(cert, level);
            }
        }
        let filtered = |kind: TrustKind| of(kind).filter(|e| x509_valid(&e.der)).filter_map(|e| parse(&e.der)).collect::<Vec<_>>();
        for cert in filtered(TrustKind::AnchorWithEnforcedConstraints) {
            if !additional.contains(&cert) {
                additional.add(cert, enforced);
            }
        }
        for cert in filtered(TrustKind::Anchor) {
            if !additional.contains(&cert) {
                additional.add(cert, Trust::ANCHOR);
            }
        }
        for cert in filtered(TrustKind::UntrustedAuthority) {
            if !additional.contains(&cert) {
                additional.add(cert, Trust::UNSPECIFIED);
            }
        }
        Self { verifier, additional, constraints, time, scts, ocsp }
    }

    fn is_non_chrome_anchor(&self, cert: &ParsedCertificate) -> bool {
        self.additional.trust(cert).is_anchor()
    }

    fn check_extra_constraints(&self, path: &mut ResultPath) {
        let root = path.certs.last().expect("checked paths are not empty").clone();
        let Some(constraint) = self.constraints.iter().find(|c| c.cert.der == root.der) else { return };
        if constraint.dns_names.is_empty() && constraint.cidrs.is_empty() {
            return;
        }
        let mut permitted = GeneralNames::default();
        if !constraint.dns_names.is_empty() {
            permitted.dns = constraint.dns_names.clone();
            permitted.present |= names::NAME_DNS;
        }
        if !constraint.cidrs.is_empty() {
            permitted.ip_ranges = constraint.cidrs.clone();
            permitted.present |= names::NAME_IP;
        }
        let leaf = path.certs[0].clone();
        NameConstraints::from_permitted(permitted).check(&leaf.normalized_subject, leaf.subject_alt_names.as_ref(), path.errors.for_cert(0));
    }

    fn satisfies_crs_constraint(&self, path: &ResultPath, constraint: &RootConstraint) -> bool {
        let leaf = &path.certs[0];
        if let Some(limit) = constraint.validity_starts_not_after {
            match generalized(limit) {
                Some(limit) if leaf.not_before <= limit => {}
                _ => return false,
            }
        }
        if let Some(limit) = constraint.validity_starts_after {
            match generalized(limit) {
                Some(limit) if leaf.not_before > limit => {}
                _ => return false,
            }
        }
        if self.verifier.ct_timely(self.time) {
            let valid = || {
                path.delegate
                    .valid_scts
                    .iter()
                    .filter(|sct| !ct::disqualified_at(&self.verifier.data.ct_logs[sct.log_index], self.time))
                    .map(|sct| i64::try_from(sct.timestamp_ms).unwrap_or(i64::MAX).saturating_mul(1000))
                    .collect::<Vec<_>>()
            };
            if let Some(limit) = constraint.sct_not_after {
                if !valid().iter().any(|timestamp| *timestamp <= limit) {
                    return false;
                }
            }
            if let Some(limit) = constraint.sct_all_after {
                let timestamps = valid();
                if timestamps.is_empty() || timestamps.iter().any(|timestamp| *timestamp <= limit) {
                    return false;
                }
            }
        }
        if !constraint.permitted_dns_names.is_empty() {
            let mut permitted = GeneralNames::default();
            permitted.dns = constraint.permitted_dns_names.iter().map(|n| n.as_bytes().to_vec()).collect();
            permitted.present |= names::NAME_DNS;
            let mut errors = crate::errors::CertErrors::default();
            NameConstraints::from_permitted(permitted).check(&leaf.normalized_subject, leaf.subject_alt_names.as_ref(), &mut errors);
            if errors.has_high_severity() {
                return false;
            }
        }
        let version = &self.verifier.data.chrome_version;
        if constraint.min_version.as_ref().is_some_and(|min| compare_versions(version, min).is_lt()) {
            return false;
        }
        if constraint.max_version_exclusive.as_ref().is_some_and(|max| compare_versions(version, max).is_ge()) {
            return false;
        }
        true
    }

    fn check_chrome_root_constraints(&self, path: &mut ResultPath) {
        let root = path.certs.last().expect("checked paths are not empty").clone();
        if self.is_non_chrome_anchor(&root) {
            return;
        }
        let constraints = self.verifier.crs_constraints_for(&root);
        if !constraints.is_empty() && !constraints.iter().any(|c| self.satisfies_crs_constraint(path, c)) {
            path.errors.other().add_error(CertError::ChromeRootConstraintsFailed);
        }
    }

    fn check_path(&self, path: &mut ResultPath) {
        path.delegate.valid_scts = ct::verify_scts(
            &self.verifier.data.ct_logs,
            &path.certs[0].der,
            path.certs.get(1).map(|cert| cert.der.as_slice()),
            self.scts,
            self.ocsp,
        );
        self.check_extra_constraints(path);
        self.check_chrome_root_constraints(path);
        if !path.is_valid() {
            return;
        }
        for (index, cert) in path.certs.iter().enumerate().rev() {
            if self.verifier.data.blocked_spkis.contains(&keys::sha256(&cert.spki_tlv)) {
                path.errors.for_cert(index).add_error(CertError::CertificateRevoked);
                return;
            }
        }
        let timely = self.verifier.ct_timely(self.time);
        path.delegate.ct_compliance = if timely {
            ct::compliance(
                &self.verifier.data.ct_logs,
                &path.delegate.valid_scts,
                self.time,
                path.certs[0].not_before.to_posix(),
                path.certs[0].not_after.to_posix(),
                self.verifier.data.ct_log_list_micros,
            )
        } else {
            CtCompliance::BuildNotTimely
        };
        let known_root = path.trusted_cert().is_some_and(|c| self.verifier.is_known_root(c));
        if known_root && timely && path.delegate.ct_compliance != CtCompliance::Complies {
            path.errors.for_cert(0).add_error(CertError::CtRequirementsNotMet);
        }
    }

    fn build(&self, target: Rc<ParsedCertificate>, intermediates: &[Rc<ParsedCertificate>]) -> BuildResult {
        let Some(time) = GeneralizedTime::from_posix(self.time) else { unreachable!("record times are within years 1 to 9999") };
        let sources = Sources { trust: TrustStores { additional: &self.additional, chrome: &self.verifier.chrome }, intermediates };
        path::build(target, &sources, &time, PATH_BUILDER_ITERATION_LIMIT, &mut |p| self.check_path(p))
    }
}
