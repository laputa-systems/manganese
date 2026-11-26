use crate::der;
use crate::der::Parser;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Digest {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureAlgorithm {
    RsaPkcs1(Digest),
    Ecdsa(Digest),
    RsaPss(Digest),
    MlDsa44,
    MlDsa65,
    MlDsa87,
    MtcProof,
}

const OID_SHA1_WITH_RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x05];
const OID_SHA1_WITH_RSA_SIGNATURE: &[u8] = &[0x2b, 0x0e, 0x03, 0x02, 0x1d];
const OID_SHA256_WITH_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
const OID_SHA384_WITH_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c];
const OID_SHA512_WITH_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d];
const OID_ECDSA_SHA1: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x01];
const OID_ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const OID_ECDSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];
const OID_ECDSA_SHA512: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x04];
const OID_RSA_SSA_PSS: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0a];
const OID_MGF1: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x08];
const OID_ML_DSA_44: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x03, 0x11];
const OID_ML_DSA_65: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x03, 0x12];
const OID_ML_DSA_87: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x03, 0x13];
const OID_MTC_PROOF: &[u8] = &[0x2b, 0x06, 0x01, 0x04, 0x01, 0x82, 0xda, 0x4b, 0x2f, 0x00];
const OID_MD5: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x02, 0x05];
const OID_SHA1: &[u8] = &[0x2b, 0x0e, 0x03, 0x02, 0x1a];
const OID_SHA224: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x04];
const OID_SHA256: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
const OID_SHA384: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02];
const OID_SHA512: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03];
const OID_SHA512_256: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x06];

pub fn parse_algorithm_identifier(input: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut parser = Parser::new(input);
    let mut identifier = parser.read_sequence()?;
    if parser.has_more() {
        return None;
    }
    let oid = identifier.read_tag(der::OBJECT)?;
    let mut parameters: &[u8] = &[];
    if identifier.has_more() {
        parameters = identifier.read_raw_tlv()?;
    }
    (!identifier.has_more()).then_some((oid, parameters))
}

fn is_null(input: &[u8]) -> bool {
    let mut parser = Parser::new(input);
    matches!(parser.read_tag(der::NULL), Some(value) if value.is_empty()) && !parser.has_more()
}

fn parse_hash_algorithm(input: &[u8]) -> Option<Digest> {
    let mut parser = Parser::new(input);
    let mut algorithm = parser.read_sequence()?;
    let oid = algorithm.read_tag(der::OBJECT)?;
    let digest = match oid {
        OID_SHA1 => Some(Digest::Sha1),
        OID_SHA256 => Some(Digest::Sha256),
        OID_SHA384 => Some(Digest::Sha384),
        OID_SHA512 => Some(Digest::Sha512),
        OID_MD5 | OID_SHA224 | OID_SHA512_256 => None,
        _ => return None,
    };
    if algorithm.has_more() {
        let parameter = algorithm.read_tag(der::NULL)?;
        if !parameter.is_empty() || algorithm.has_more() {
            return None;
        }
    }
    digest
}

fn parse_rsa_pss(parameters: &[u8]) -> Option<SignatureAlgorithm> {
    let mut parser = Parser::new(parameters);
    let mut fields = parser.read_sequence()?;
    if parser.has_more() {
        return None;
    }
    let hash = parse_hash_algorithm(fields.read_tag(der::context_constructed(0))?)?;
    let (mgf_oid, mgf_parameters) = parse_algorithm_identifier(fields.read_tag(der::context_constructed(1))?)?;
    if mgf_oid != OID_MGF1 {
        return None;
    }
    let mgf_hash = parse_hash_algorithm(mgf_parameters)?;
    let mut salt_parser = fields.read_constructed(der::context_constructed(2))?;
    let salt = salt_parser.read_uint64()?;
    if salt_parser.has_more() || fields.has_more() || hash != mgf_hash {
        return None;
    }
    match (hash, salt) {
        (Digest::Sha256, 32) | (Digest::Sha384, 48) | (Digest::Sha512, 64) => Some(SignatureAlgorithm::RsaPss(hash)),
        _ => None,
    }
}

impl SignatureAlgorithm {
    pub fn parse(identifier: &[u8]) -> Option<Self> {
        let (oid, parameters) = parse_algorithm_identifier(identifier)?;
        let null_or_empty = parameters.is_empty() || is_null(parameters);
        match oid {
            OID_SHA1_WITH_RSA_ENCRYPTION | OID_SHA1_WITH_RSA_SIGNATURE if null_or_empty => Some(Self::RsaPkcs1(Digest::Sha1)),
            OID_SHA256_WITH_RSA if null_or_empty => Some(Self::RsaPkcs1(Digest::Sha256)),
            OID_SHA384_WITH_RSA if null_or_empty => Some(Self::RsaPkcs1(Digest::Sha384)),
            OID_SHA512_WITH_RSA if null_or_empty => Some(Self::RsaPkcs1(Digest::Sha512)),
            OID_ECDSA_SHA1 if parameters.is_empty() => Some(Self::Ecdsa(Digest::Sha1)),
            OID_ECDSA_SHA256 if parameters.is_empty() => Some(Self::Ecdsa(Digest::Sha256)),
            OID_ECDSA_SHA384 if parameters.is_empty() => Some(Self::Ecdsa(Digest::Sha384)),
            OID_ECDSA_SHA512 if parameters.is_empty() => Some(Self::Ecdsa(Digest::Sha512)),
            OID_ML_DSA_44 if parameters.is_empty() => Some(Self::MlDsa44),
            OID_ML_DSA_65 if parameters.is_empty() => Some(Self::MlDsa65),
            OID_ML_DSA_87 if parameters.is_empty() => Some(Self::MlDsa87),
            OID_RSA_SSA_PSS => parse_rsa_pss(parameters),
            OID_MTC_PROOF if parameters.is_empty() => Some(Self::MtcProof),
            _ => None,
        }
    }

    pub fn is_sha1(&self) -> bool {
        matches!(self, Self::RsaPkcs1(Digest::Sha1) | Self::Ecdsa(Digest::Sha1))
    }
}
