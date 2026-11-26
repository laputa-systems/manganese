use std::collections::BTreeMap;

use crate::der;
use crate::der::BitString;
use crate::der::GeneralizedTime;
use crate::der::Parser;
use crate::names;
use crate::names::GeneralNames;
use crate::names::NameConstraints;
use crate::signature::SignatureAlgorithm;

pub const OID_BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];
pub const OID_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x0f];
pub const OID_EXT_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x25];
pub const OID_SUBJECT_ALT_NAME: &[u8] = &[0x55, 0x1d, 0x11];
pub const OID_NAME_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x1e];
pub const OID_CERTIFICATE_POLICIES: &[u8] = &[0x55, 0x1d, 0x20];
pub const OID_POLICY_MAPPINGS: &[u8] = &[0x55, 0x1d, 0x21];
pub const OID_POLICY_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x24];
pub const OID_INHIBIT_ANY_POLICY: &[u8] = &[0x55, 0x1d, 0x36];
pub const OID_MS_APPLICATION_POLICIES: &[u8] = &[0x2b, 0x06, 0x01, 0x04, 0x01, 0x82, 0x37, 0x15, 0x0a];
pub const OID_ANY_POLICY: &[u8] = &[0x55, 0x1d, 0x20, 0x00];
const OID_SUBJECT_KEY_IDENTIFIER: &[u8] = &[0x55, 0x1d, 0x0e];
const OID_AUTHORITY_KEY_IDENTIFIER: &[u8] = &[0x55, 0x1d, 0x23];
const OID_AUTHORITY_INFO_ACCESS: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x01, 0x01];
const OID_CPS_POINTER: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x02, 0x01];
const OID_USER_NOTICE: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x02, 0x02];

pub const KEY_USAGE_KEY_CERT_SIGN: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    V1,
    V2,
    V3,
}

#[derive(Clone, Debug)]
pub struct Extension {
    pub critical: bool,
    pub value: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub struct BasicConstraints {
    pub is_ca: bool,
    pub path_len: Option<u8>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PolicyConstraints {
    pub require_explicit_policy: Option<u8>,
    pub inhibit_policy_mapping: Option<u8>,
}

#[derive(Clone, Debug)]
pub struct PolicyMapping {
    pub issuer_domain: Vec<u8>,
    pub subject_domain: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct KeyUsage {
    bytes: Vec<u8>,
    unused_bits: u8,
}

impl KeyUsage {
    pub fn asserts(&self, bit: usize) -> bool {
        BitString { bytes: &self.bytes, unused_bits: self.unused_bits }.asserts_bit(bit)
    }
}

pub struct ParsedCertificate {
    pub der: Vec<u8>,
    pub tbs_tlv: Vec<u8>,
    pub signature_algorithm_tlv: Vec<u8>,
    pub signature_value: Vec<u8>,
    pub signature_unused_bits: u8,
    pub tbs_signature_algorithm_tlv: Vec<u8>,
    pub spki_tlv: Vec<u8>,
    pub not_before: GeneralizedTime,
    pub not_after: GeneralizedTime,
    pub signature_algorithm: Option<SignatureAlgorithm>,
    pub normalized_subject: Vec<u8>,
    pub normalized_issuer: Vec<u8>,
    pub extensions: BTreeMap<Vec<u8>, Extension>,
    pub basic_constraints: Option<BasicConstraints>,
    pub key_usage: Option<KeyUsage>,
    pub extended_key_usage: Option<Vec<Vec<u8>>>,
    pub subject_alt_names_extension: Option<Extension>,
    pub subject_alt_names: Option<GeneralNames>,
    pub name_constraints: Option<NameConstraints>,
    pub policy_oids: Option<Vec<Vec<u8>>>,
    pub policy_constraints: Option<PolicyConstraints>,
    pub policy_mappings: Option<Vec<PolicyMapping>>,
    pub inhibit_any_policy: Option<u8>,
    pub subject_key_identifier: Option<Vec<u8>>,
    pub authority_key_identifier: Option<Vec<u8>>,
}

pub struct Outer<'a> {
    pub tbs_tlv: &'a [u8],
    pub signature_algorithm_tlv: &'a [u8],
    pub signature: BitString<'a>,
}

fn is_sequence_tlv(input: &[u8]) -> bool {
    let mut parser = Parser::new(input);
    parser.read_sequence().is_some() && !parser.has_more()
}

fn read_sequence_tlv<'a>(parser: &mut Parser<'a>) -> Option<&'a [u8]> {
    let tlv = parser.read_raw_tlv()?;
    is_sequence_tlv(tlv).then_some(tlv)
}

pub fn parse_outer(der: &[u8]) -> Option<Outer<'_>> {
    let mut parser = Parser::new(der);
    let mut certificate = parser.read_sequence()?;
    let tbs_tlv = read_sequence_tlv(&mut certificate)?;
    let signature_algorithm_tlv = read_sequence_tlv(&mut certificate)?;
    let signature = certificate.read_bit_string()?;
    if certificate.has_more() || parser.has_more() {
        return None;
    }
    Some(Outer { tbs_tlv, signature_algorithm_tlv, signature })
}

pub struct Tbs<'a> {
    pub signature_algorithm_tlv: &'a [u8],
    pub issuer_tlv: &'a [u8],
    pub subject_tlv: &'a [u8],
    pub spki_tlv: &'a [u8],
    pub not_before: GeneralizedTime,
    pub not_after: GeneralizedTime,
    pub extensions_tlv: Option<&'a [u8]>,
}

fn read_time(parser: &mut Parser<'_>) -> Option<GeneralizedTime> {
    let (tag, value) = parser.read_tag_and_value()?;
    match tag {
        der::UTC_TIME => der::parse_utc_time(value),
        der::GENERALIZED_TIME => der::parse_generalized_time(value),
        _ => None,
    }
}

fn parse_validity(tlv: &[u8]) -> Option<(GeneralizedTime, GeneralizedTime)> {
    let mut parser = Parser::new(tlv);
    let mut validity = parser.read_sequence()?;
    let not_before = read_time(&mut validity)?;
    let not_after = read_time(&mut validity)?;
    if parser.has_more() || validity.has_more() {
        return None;
    }
    Some((not_before, not_after))
}

pub fn parse_tbs(tlv: &[u8]) -> Option<Tbs<'_>> {
    let mut parser = Parser::new(tlv);
    let mut tbs = parser.read_sequence()?;
    let version = match tbs.read_optional_tag(der::context_constructed(0))? {
        Some(encoded) => {
            let mut version_parser = Parser::new(encoded);
            let version = match version_parser.read_uint64()? {
                1 => Version::V2,
                2 => Version::V3,
                _ => return None,
            };
            if version_parser.has_more() {
                return None;
            }
            version
        }
        None => Version::V1,
    };
    tbs.read_tag(der::INTEGER)?;
    let signature_algorithm_tlv = read_sequence_tlv(&mut tbs)?;
    let issuer_tlv = read_sequence_tlv(&mut tbs)?;
    let validity_tlv = tbs.read_raw_tlv()?;
    let (not_before, not_after) = parse_validity(validity_tlv)?;
    let subject_tlv = read_sequence_tlv(&mut tbs)?;
    let spki_tlv = read_sequence_tlv(&mut tbs)?;
    for number in [1, 2] {
        if let Some(unique_id) = tbs.read_optional_tag(der::context(number))? {
            der::parse_bit_string(unique_id)?;
            if version == Version::V1 {
                return None;
            }
        }
    }
    let extensions_tlv = tbs.read_optional_tag(der::context_constructed(3))?;
    if let Some(extensions) = extensions_tlv {
        if !is_sequence_tlv(extensions) || version != Version::V3 {
            return None;
        }
    }
    if tbs.has_more() || parser.has_more() {
        return None;
    }
    Some(Tbs { signature_algorithm_tlv, issuer_tlv, subject_tlv, spki_tlv, not_before, not_after, extensions_tlv })
}

fn parse_extension(tlv: &[u8]) -> Option<(Vec<u8>, Extension)> {
    let mut parser = Parser::new(tlv);
    let mut extension = parser.read_sequence()?;
    let oid = extension.read_tag(der::OBJECT)?;
    let mut critical = false;
    if let Some(encoded) = extension.read_optional_tag(der::BOOLEAN)? {
        critical = der::parse_bool(encoded)?;
        if !critical {
            return None;
        }
    }
    let value = extension.read_tag(der::OCTET_STRING)?;
    if extension.has_more() || parser.has_more() {
        return None;
    }
    Some((oid.to_vec(), Extension { critical, value: value.to_vec() }))
}

pub fn parse_extensions(tlv: &[u8]) -> Option<BTreeMap<Vec<u8>, Extension>> {
    let mut parser = Parser::new(tlv);
    let mut sequence = parser.read_sequence()?;
    if !sequence.has_more() {
        return None;
    }
    let mut out = BTreeMap::new();
    while sequence.has_more() {
        let (oid, extension) = parse_extension(sequence.read_raw_tlv()?)?;
        if out.insert(oid, extension).is_some() {
            return None;
        }
    }
    if parser.has_more() {
        return None;
    }
    Some(out)
}

fn parse_basic_constraints(value: &[u8]) -> Option<BasicConstraints> {
    let mut parser = Parser::new(value);
    let mut sequence = parser.read_sequence()?;
    let is_ca = match sequence.read_optional_tag(der::BOOLEAN)? {
        Some(encoded) => der::parse_bool(encoded)?,
        None => false,
    };
    let path_len = match sequence.read_optional_tag(der::INTEGER)? {
        Some(encoded) => Some(der::parse_uint8(encoded)?),
        None => None,
    };
    if sequence.has_more() || parser.has_more() {
        return None;
    }
    Some(BasicConstraints { is_ca, path_len })
}

fn parse_key_usage(value: &[u8]) -> Option<KeyUsage> {
    let mut parser = Parser::new(value);
    let bits = parser.read_bit_string()?;
    if parser.has_more() || bits.bytes.iter().all(|b| *b == 0) {
        return None;
    }
    Some(KeyUsage { bytes: bits.bytes.to_vec(), unused_bits: bits.unused_bits })
}

fn parse_eku(value: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut parser = Parser::new(value);
    let mut sequence = parser.read_sequence()?;
    if !sequence.has_more() {
        return None;
    }
    let mut out = Vec::new();
    while sequence.has_more() {
        out.push(sequence.read_tag(der::OBJECT)?.to_vec());
    }
    if parser.has_more() {
        return None;
    }
    Some(out)
}

fn parse_authority_info_access(value: &[u8]) -> Option<()> {
    let mut parser = Parser::new(value);
    let mut sequence = parser.read_sequence()?;
    if !sequence.has_more() {
        return None;
    }
    let mut descriptions = Vec::new();
    while sequence.has_more() {
        let mut description = sequence.read_sequence()?;
        description.read_tag(der::OBJECT)?;
        let location = description.read_raw_tlv()?;
        if description.has_more() {
            return None;
        }
        descriptions.push(location);
    }
    for location in descriptions {
        let mut location_parser = Parser::new(location);
        let (tag, uri) = location_parser.read_tag_and_value()?;
        if tag == der::context(6) && !uri.is_ascii() {
            return None;
        }
    }
    Some(())
}

pub fn parse_policy_oids(value: &[u8], fail_unknown_qualifiers: bool) -> Option<Vec<Vec<u8>>> {
    let mut parser = Parser::new(value);
    let mut policies = parser.read_sequence()?;
    if parser.has_more() || !policies.has_more() {
        return None;
    }
    let mut oids = Vec::new();
    while policies.has_more() {
        let mut information = policies.read_sequence()?;
        let oid = information.read_tag(der::OBJECT)?;
        oids.push(oid.to_vec());
        if !information.has_more() {
            continue;
        }
        let mut qualifiers = information.read_sequence()?;
        if information.has_more() || !qualifiers.has_more() {
            return None;
        }
        let restrict = fail_unknown_qualifiers || oid == OID_ANY_POLICY;
        while qualifiers.has_more() {
            let mut qualifier = qualifiers.read_sequence()?;
            let qualifier_oid = qualifier.read_tag(der::OBJECT)?;
            if restrict && qualifier_oid != OID_CPS_POINTER && qualifier_oid != OID_USER_NOTICE {
                return None;
            }
            qualifier.read_raw_tlv()?;
            if qualifier.has_more() {
                return None;
            }
        }
    }
    oids.sort();
    if oids.windows(2).any(|pair| pair[0] == pair[1]) {
        return None;
    }
    Some(oids)
}

fn parse_policy_constraints(value: &[u8]) -> Option<PolicyConstraints> {
    let mut parser = Parser::new(value);
    let mut sequence = parser.read_sequence()?;
    if !sequence.has_more() {
        return None;
    }
    let mut out = PolicyConstraints::default();
    if let Some(require) = sequence.read_optional_tag(der::context(0))? {
        out.require_explicit_policy = Some(der::parse_uint8(require)?);
    }
    if let Some(inhibit) = sequence.read_optional_tag(der::context(1))? {
        out.inhibit_policy_mapping = Some(der::parse_uint8(inhibit)?);
    }
    if sequence.has_more() || parser.has_more() {
        return None;
    }
    Some(out)
}

fn parse_inhibit_any_policy(value: &[u8]) -> Option<u8> {
    let mut parser = Parser::new(value);
    let skip = parser.read_uint8()?;
    (!parser.has_more()).then_some(skip)
}

fn parse_policy_mappings(value: &[u8]) -> Option<Vec<PolicyMapping>> {
    let mut parser = Parser::new(value);
    let mut sequence = parser.read_sequence()?;
    if !sequence.has_more() {
        return None;
    }
    let mut out = Vec::new();
    while sequence.has_more() {
        let mut mapping = sequence.read_sequence()?;
        let issuer_domain = mapping.read_tag(der::OBJECT)?.to_vec();
        let subject_domain = mapping.read_tag(der::OBJECT)?.to_vec();
        if mapping.has_more() {
            return None;
        }
        out.push(PolicyMapping { issuer_domain, subject_domain });
    }
    if parser.has_more() {
        return None;
    }
    Some(out)
}

fn parse_subject_key_identifier(value: &[u8]) -> Option<Vec<u8>> {
    let mut parser = Parser::new(value);
    let id = parser.read_tag(der::OCTET_STRING)?;
    (!parser.has_more()).then(|| id.to_vec())
}

fn parse_authority_key_identifier(value: &[u8]) -> Option<Option<Vec<u8>>> {
    let mut parser = Parser::new(value);
    let mut aki = parser.read_sequence()?;
    if parser.has_more() {
        return None;
    }
    let key_identifier = aki.read_optional_tag(der::context(0))?.map(<[u8]>::to_vec);
    let issuer = aki.read_optional_tag(der::context_constructed(1))?;
    let serial = aki.read_optional_tag(der::context(2))?;
    if issuer.is_some() != serial.is_some() || aki.has_more() {
        return None;
    }
    Some(key_identifier)
}

fn sequence_value(tlv: &[u8]) -> Option<&[u8]> {
    let mut parser = Parser::new(tlv);
    let value = parser.read_tag(der::SEQUENCE)?;
    (!parser.has_more()).then_some(value)
}

impl ParsedCertificate {
    pub fn parse(der: &[u8]) -> Option<Self> {
        let outer = parse_outer(der)?;
        let tbs = parse_tbs(outer.tbs_tlv)?;
        let normalized_subject = names::normalize_name(sequence_value(tbs.subject_tlv)?)?;
        let normalized_issuer = names::normalize_name(sequence_value(tbs.issuer_tlv)?)?;
        let mut cert = Self {
            der: der.to_vec(),
            tbs_tlv: outer.tbs_tlv.to_vec(),
            signature_algorithm_tlv: outer.signature_algorithm_tlv.to_vec(),
            signature_value: outer.signature.bytes.to_vec(),
            signature_unused_bits: outer.signature.unused_bits,
            tbs_signature_algorithm_tlv: tbs.signature_algorithm_tlv.to_vec(),
            spki_tlv: tbs.spki_tlv.to_vec(),
            not_before: tbs.not_before,
            not_after: tbs.not_after,
            signature_algorithm: SignatureAlgorithm::parse(outer.signature_algorithm_tlv),
            normalized_subject,
            normalized_issuer,
            extensions: BTreeMap::new(),
            basic_constraints: None,
            key_usage: None,
            extended_key_usage: None,
            subject_alt_names_extension: None,
            subject_alt_names: None,
            name_constraints: None,
            policy_oids: None,
            policy_constraints: None,
            policy_mappings: None,
            inhibit_any_policy: None,
            subject_key_identifier: None,
            authority_key_identifier: None,
        };
        let Some(extensions_tlv) = tbs.extensions_tlv else { return Some(cert) };
        cert.extensions = parse_extensions(extensions_tlv)?;
        let extensions = cert.extensions.clone();
        let get = |oid: &[u8]| extensions.get(oid);
        if let Some(extension) = get(OID_BASIC_CONSTRAINTS) {
            cert.basic_constraints = Some(parse_basic_constraints(&extension.value)?);
        }
        if let Some(extension) = get(OID_KEY_USAGE) {
            cert.key_usage = Some(parse_key_usage(&extension.value)?);
        }
        if let Some(extension) = get(OID_EXT_KEY_USAGE) {
            cert.extended_key_usage = Some(parse_eku(&extension.value)?);
        }
        if let Some(extension) = get(OID_SUBJECT_ALT_NAME) {
            cert.subject_alt_names = Some(GeneralNames::parse(&extension.value)?);
            if sequence_value(tbs.subject_tlv)?.is_empty() && !extension.critical {
                return None;
            }
            cert.subject_alt_names_extension = Some(extension.clone());
        }
        if let Some(extension) = get(OID_NAME_CONSTRAINTS) {
            cert.name_constraints = Some(NameConstraints::parse(&extension.value, extension.critical)?);
        }
        if let Some(extension) = get(OID_AUTHORITY_INFO_ACCESS) {
            parse_authority_info_access(&extension.value)?;
        }
        if let Some(extension) = get(OID_CERTIFICATE_POLICIES) {
            cert.policy_oids = Some(parse_policy_oids(&extension.value, false)?);
        }
        if let Some(extension) = get(OID_POLICY_CONSTRAINTS) {
            cert.policy_constraints = Some(parse_policy_constraints(&extension.value)?);
        }
        if let Some(extension) = get(OID_POLICY_MAPPINGS) {
            cert.policy_mappings = Some(parse_policy_mappings(&extension.value)?);
        }
        if let Some(extension) = get(OID_INHIBIT_ANY_POLICY) {
            cert.inhibit_any_policy = Some(parse_inhibit_any_policy(&extension.value)?);
        }
        if let Some(extension) = get(OID_SUBJECT_KEY_IDENTIFIER) {
            cert.subject_key_identifier = Some(parse_subject_key_identifier(&extension.value)?);
        }
        if let Some(extension) = get(OID_AUTHORITY_KEY_IDENTIFIER) {
            cert.authority_key_identifier = parse_authority_key_identifier(&extension.value)?;
        }
        Some(cert)
    }

    pub fn is_self_issued(&self) -> bool {
        self.normalized_subject == self.normalized_issuer
    }

    pub fn subject_alt_names_value(&self) -> &[u8] {
        self.subject_alt_names_extension.as_ref().map_or(&[], |e| e.value.as_slice())
    }

    pub fn signature(&self) -> BitString<'_> {
        BitString { bytes: &self.signature_value, unused_bits: self.signature_unused_bits }
    }
}
