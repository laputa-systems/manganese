use graviola::hashing::Hash;
use graviola::hashing::Sha256;
use graviola::hashing::Sha384;
use graviola::hashing::Sha512;
use graviola::signing::ecdsa;
use graviola::signing::rsa;
use pki_types::AlgorithmIdentifier;
use pki_types::InvalidSignature;
use pki_types::SignatureVerificationAlgorithm;
use pki_types::SubjectPublicKeyInfoDer;
use pki_types::alg_id;

use crate::der;
use crate::der::BitString;
use crate::der::Parser;
use crate::signature::Digest;
use crate::signature::SignatureAlgorithm;

const OID_RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
const OID_EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const OID_P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const OID_P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];
const OID_P521: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x23];
const ML_DSA: [(&[u8], usize, MlDsa); 3] = [
    (&[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x03, 0x11], 1312, MlDsa::L44),
    (&[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x03, 0x12], 1952, MlDsa::L65),
    (&[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x03, 0x13], 2592, MlDsa::L87),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Curve {
    P256,
    P384,
    P521,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MlDsa {
    L44,
    L65,
    L87,
}

#[derive(Clone, Debug)]
pub enum PublicKey {
    Rsa { bits: usize },
    Ec(Curve),
    MlDsa(MlDsa),
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(Sha256::hash(data).as_ref());
    out
}

fn unsigned_integer<'a>(parser: &mut Parser<'a>) -> Option<&'a [u8]> {
    let value = parser.read_tag(der::INTEGER)?;
    if der::is_valid_integer(value)? {
        return None;
    }
    Some(value)
}

fn bit_length(value: &[u8]) -> usize {
    let start = value.iter().take_while(|b| **b == 0).count();
    match value.get(start) {
        Some(first) => (value.len() - start) * 8 - first.leading_zeros() as usize,
        None => 0,
    }
}

fn parse_rsa_key(key: &[u8]) -> Option<usize> {
    let mut outer = Parser::new(key);
    let mut sequence = outer.read_sequence()?;
    let modulus = unsigned_integer(&mut sequence)?;
    let exponent = unsigned_integer(&mut sequence)?;
    if sequence.has_more() || outer.has_more() {
        return None;
    }
    let bits = bit_length(modulus);
    let exponent_bits = bit_length(exponent);
    let odd = |value: &[u8]| value.last().is_some_and(|b| b & 1 == 1);
    if !(512..=16384).contains(&bits) || !odd(modulus) || exponent_bits < 2 || exponent_bits > 33 || !odd(exponent) {
        return None;
    }
    Some(bits)
}

fn valid_point(curve: Curve, point: &[u8]) -> bool {
    match curve {
        Curve::P256 => p256::PublicKey::from_sec1_bytes(point).is_ok(),
        Curve::P384 => p384::PublicKey::from_sec1_bytes(point).is_ok(),
        Curve::P521 => p521::PublicKey::from_sec1_bytes(point).is_ok(),
    }
}

pub fn parse_public_key(spki_tlv: &[u8]) -> Option<PublicKey> {
    let mut outer = Parser::new(spki_tlv);
    let mut spki = outer.read_sequence()?;
    let mut algorithm = spki.read_sequence()?;
    let oid = algorithm.read_tag(der::OBJECT)?;
    let key = spki.read_tag(der::BIT_STRING)?;
    if spki.has_more() || outer.has_more() {
        return None;
    }
    let params = algorithm.remaining();
    let (&padding, key) = key.split_first()?;
    if padding != 0 {
        return None;
    }
    if oid == OID_RSA_ENCRYPTION {
        let mut params_parser = Parser::new(params);
        let null = params_parser.read_tag(der::NULL)?;
        if !null.is_empty() || params_parser.has_more() {
            return None;
        }
        return parse_rsa_key(key).map(|bits| PublicKey::Rsa { bits });
    }
    if oid == OID_EC_PUBLIC_KEY {
        let mut params_parser = Parser::new(params);
        let curve = match params_parser.read_tag(der::OBJECT)? {
            OID_P256 => Curve::P256,
            OID_P384 => Curve::P384,
            OID_P521 => Curve::P521,
            _ => return None,
        };
        if params_parser.has_more() || !valid_point(curve, key) {
            return None;
        }
        return Some(PublicKey::Ec(curve));
    }
    for (ml_oid, length, level) in ML_DSA {
        if oid == ml_oid {
            return (params.is_empty() && key.len() == length).then_some(PublicKey::MlDsa(level));
        }
    }
    None
}

impl PublicKey {
    pub fn is_acceptable(&self, min_rsa_bits: usize) -> bool {
        match self {
            Self::Rsa { bits } => *bits >= min_rsa_bits,
            Self::Ec(_) | Self::MlDsa(_) => true,
        }
    }
}

#[derive(Debug)]
struct RsaPkcs1(Digest);

#[derive(Debug)]
struct RsaPss(Digest);

#[derive(Debug)]
struct Ecdsa(Curve, Digest);

fn rsa_fallback(public_key: &[u8]) -> Option<::rsa::RsaPublicKey> {
    use ::rsa::BigUint;
    let mut outer = Parser::new(public_key);
    let mut sequence = outer.read_sequence()?;
    let modulus = unsigned_integer(&mut sequence)?;
    let exponent = unsigned_integer(&mut sequence)?;
    ::rsa::RsaPublicKey::new_with_max_size(BigUint::from_bytes_be(modulus), BigUint::from_bytes_be(exponent), 16384).ok()
}

fn digest_bytes(digest: Digest, message: &[u8]) -> Vec<u8> {
    use sha2::Digest as _;
    match digest {
        Digest::Sha1 => unreachable!("SHA-1 signatures are rejected before verification"),
        Digest::Sha256 => sha2::Sha256::digest(message).to_vec(),
        Digest::Sha384 => sha2::Sha384::digest(message).to_vec(),
        Digest::Sha512 => sha2::Sha512::digest(message).to_vec(),
    }
}

fn verified(result: bool) -> Result<(), InvalidSignature> {
    if result { Ok(()) } else { Err(InvalidSignature) }
}

impl SignatureVerificationAlgorithm for RsaPkcs1 {
    fn public_key_alg_id(&self) -> AlgorithmIdentifier {
        alg_id::RSA_ENCRYPTION
    }

    fn signature_alg_id(&self) -> AlgorithmIdentifier {
        match self.0 {
            Digest::Sha384 => alg_id::RSA_PKCS1_SHA384,
            Digest::Sha512 => alg_id::RSA_PKCS1_SHA512,
            Digest::Sha1 | Digest::Sha256 => alg_id::RSA_PKCS1_SHA256,
        }
    }

    fn verify_signature(&self, public_key: &[u8], message: &[u8], signature: &[u8]) -> Result<(), InvalidSignature> {
        if let Ok(key) = rsa::VerifyingKey::from_pkcs1_der(public_key) {
            let result = match self.0 {
                Digest::Sha256 => key.verify_pkcs1_sha256(signature, message),
                Digest::Sha384 => key.verify_pkcs1_sha384(signature, message),
                Digest::Sha512 => key.verify_pkcs1_sha512(signature, message),
                Digest::Sha1 => unreachable!("SHA-1 signatures are rejected before verification"),
            };
            return verified(result.is_ok());
        }
        let key = rsa_fallback(public_key).ok_or(InvalidSignature)?;
        let hashed = digest_bytes(self.0, message);
        let scheme = match self.0 {
            Digest::Sha256 => ::rsa::Pkcs1v15Sign::new::<sha2::Sha256>(),
            Digest::Sha384 => ::rsa::Pkcs1v15Sign::new::<sha2::Sha384>(),
            Digest::Sha512 => ::rsa::Pkcs1v15Sign::new::<sha2::Sha512>(),
            Digest::Sha1 => unreachable!("SHA-1 signatures are rejected before verification"),
        };
        verified(key.verify(scheme, &hashed, signature).is_ok())
    }
}

impl SignatureVerificationAlgorithm for RsaPss {
    fn public_key_alg_id(&self) -> AlgorithmIdentifier {
        alg_id::RSA_ENCRYPTION
    }

    fn signature_alg_id(&self) -> AlgorithmIdentifier {
        match self.0 {
            Digest::Sha384 => alg_id::RSA_PSS_SHA384,
            Digest::Sha512 => alg_id::RSA_PSS_SHA512,
            Digest::Sha1 | Digest::Sha256 => alg_id::RSA_PSS_SHA256,
        }
    }

    fn verify_signature(&self, public_key: &[u8], message: &[u8], signature: &[u8]) -> Result<(), InvalidSignature> {
        if let Ok(key) = rsa::VerifyingKey::from_pkcs1_der(public_key) {
            let result = match self.0 {
                Digest::Sha256 => key.verify_pss_sha256(signature, message),
                Digest::Sha384 => key.verify_pss_sha384(signature, message),
                Digest::Sha512 => key.verify_pss_sha512(signature, message),
                Digest::Sha1 => unreachable!("PSS parameters only admit SHA-2 digests"),
            };
            return verified(result.is_ok());
        }
        let key = rsa_fallback(public_key).ok_or(InvalidSignature)?;
        let hashed = digest_bytes(self.0, message);
        let scheme = match self.0 {
            Digest::Sha256 => ::rsa::Pss::new_with_salt::<sha2::Sha256>(32),
            Digest::Sha384 => ::rsa::Pss::new_with_salt::<sha2::Sha384>(48),
            Digest::Sha512 => ::rsa::Pss::new_with_salt::<sha2::Sha512>(64),
            Digest::Sha1 => unreachable!("PSS parameters only admit SHA-2 digests"),
        };
        verified(key.verify(scheme, &hashed, signature).is_ok())
    }
}

fn uncompressed(curve: Curve, point: &[u8]) -> Option<Vec<u8>> {
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    match curve {
        Curve::P256 => Some(p256::PublicKey::from_sec1_bytes(point).ok()?.to_encoded_point(false).as_bytes().to_vec()),
        Curve::P384 => Some(p384::PublicKey::from_sec1_bytes(point).ok()?.to_encoded_point(false).as_bytes().to_vec()),
        Curve::P521 => Some(p521::PublicKey::from_sec1_bytes(point).ok()?.to_encoded_point(false).as_bytes().to_vec()),
    }
}

fn graviola_ecdsa<C: ecdsa::Curve>(point: &[u8], digest: Digest, message: &[u8], signature: &[u8]) -> bool {
    let Ok(key) = ecdsa::VerifyingKey::<C>::from_x962_uncompressed(point) else { return false };
    match digest {
        Digest::Sha256 => key.verify_asn1::<Sha256>(&[message], signature).is_ok(),
        Digest::Sha384 => key.verify_asn1::<Sha384>(&[message], signature).is_ok(),
        Digest::Sha512 => key.verify_asn1::<Sha512>(&[message], signature).is_ok(),
        Digest::Sha1 => unreachable!("SHA-1 signatures are rejected before verification"),
    }
}

fn p521_ecdsa(point: &[u8], digest: Digest, message: &[u8], signature: &[u8]) -> bool {
    use p521::ecdsa::signature::hazmat::PrehashVerifier;
    let (Ok(key), Ok(signature)) = (p521::ecdsa::VerifyingKey::from_sec1_bytes(point), p521::ecdsa::Signature::from_der(signature)) else {
        return false;
    };
    let digest = digest_bytes(digest, message);
    let mut field = vec![0u8; 66 - digest.len()];
    field.extend_from_slice(&digest);
    key.verify_prehash(&field, &signature).is_ok()
}

impl SignatureVerificationAlgorithm for Ecdsa {
    fn public_key_alg_id(&self) -> AlgorithmIdentifier {
        match self.0 {
            Curve::P256 => alg_id::ECDSA_P256,
            Curve::P384 => alg_id::ECDSA_P384,
            Curve::P521 => alg_id::ECDSA_P521,
        }
    }

    fn signature_alg_id(&self) -> AlgorithmIdentifier {
        match self.1 {
            Digest::Sha384 => alg_id::ECDSA_SHA384,
            Digest::Sha512 => alg_id::ECDSA_SHA512,
            Digest::Sha1 | Digest::Sha256 => alg_id::ECDSA_SHA256,
        }
    }

    fn verify_signature(&self, public_key: &[u8], message: &[u8], signature: &[u8]) -> Result<(), InvalidSignature> {
        let point = uncompressed(self.0, public_key).ok_or(InvalidSignature)?;
        verified(match self.0 {
            Curve::P256 => graviola_ecdsa::<ecdsa::P256>(&point, self.1, message, signature),
            Curve::P384 => graviola_ecdsa::<ecdsa::P384>(&point, self.1, message, signature),
            Curve::P521 => p521_ecdsa(&point, self.1, message, signature),
        })
    }
}

pub fn verify_signed_data(algorithm: SignatureAlgorithm, message: &[u8], signature: BitString<'_>, key: &PublicKey, spki_tlv: &[u8]) -> bool {
    let verifier: Box<dyn SignatureVerificationAlgorithm> = match (algorithm, key) {
        (SignatureAlgorithm::RsaPkcs1(digest), PublicKey::Rsa { .. }) => Box::new(RsaPkcs1(digest)),
        (SignatureAlgorithm::RsaPss(digest), PublicKey::Rsa { .. }) => Box::new(RsaPss(digest)),
        (SignatureAlgorithm::Ecdsa(digest), PublicKey::Ec(curve)) => Box::new(Ecdsa(*curve, digest)),
        (SignatureAlgorithm::MlDsa44, PublicKey::MlDsa(MlDsa::L44))
        | (SignatureAlgorithm::MlDsa65, PublicKey::MlDsa(MlDsa::L65))
        | (SignatureAlgorithm::MlDsa87, PublicKey::MlDsa(MlDsa::L87)) => {
            panic!("ML-DSA signature verification is outside the record domain: no Graviola or released RustCrypto ML-DSA verifier exists")
        }
        _ => return false,
    };
    if signature.unused_bits != 0 {
        return false;
    }
    let spki = SubjectPublicKeyInfoDer::from(spki_tlv);
    match webpki::RawPublicKeyEntity::try_from(&spki) {
        Ok(entity) => entity.verify_signature(verifier.as_ref(), message, signature.bytes).is_ok(),
        Err(_) => false,
    }
}
