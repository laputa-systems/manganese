use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::rc::Rc;

use crate::cert;
use crate::cert::ParsedCertificate;
use crate::der::GeneralizedTime;
use crate::errors::CertError;
use crate::errors::CertErrors;
use crate::errors::CertPathErrors;
use crate::keys;
use crate::keys::PublicKey;
use crate::names::NameConstraints;
use crate::signature::SignatureAlgorithm;
use crate::trust::Trust;
use crate::trust::TrustType;

const MIN_RSA_MODULUS_BITS: usize = 1024;
const OID_SERVER_AUTH: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01];
const OID_ANY_EKU: &[u8] = &[0x55, 0x1d, 0x25, 0x00];

fn is_handled_critical_extension(oid: &[u8], extension: &cert::Extension) -> bool {
    match oid {
        cert::OID_BASIC_CONSTRAINTS | cert::OID_KEY_USAGE | cert::OID_EXT_KEY_USAGE | cert::OID_NAME_CONSTRAINTS | cert::OID_SUBJECT_ALT_NAME => true,
        cert::OID_CERTIFICATE_POLICIES => cert::parse_policy_oids(&extension.value, true).is_some(),
        cert::OID_POLICY_MAPPINGS | cert::OID_POLICY_CONSTRAINTS | cert::OID_INHIBIT_ANY_POLICY => true,
        _ => false,
    }
}

fn verify_no_unconsumed_critical_extensions(cert: &ParsedCertificate, errors: &mut CertErrors) {
    for (oid, extension) in &cert.extensions {
        if !extension.critical {
            continue;
        }
        if oid == cert::OID_MS_APPLICATION_POLICIES && cert.extended_key_usage.is_some() {
            continue;
        }
        if !is_handled_critical_extension(oid, extension) {
            errors.add_error(CertError::UnconsumedCriticalExtension);
        }
    }
}

fn verify_time_validity(cert: &ParsedCertificate, time: &GeneralizedTime, errors: &mut CertErrors) {
    if *time < cert.not_before {
        errors.add_error(CertError::ValidityFailedNotBefore);
    }
    if cert.not_after < *time {
        errors.add_error(CertError::ValidityFailedNotAfter);
    }
}

fn verify_signature_algorithms_match(cert: &ParsedCertificate, errors: &mut CertErrors) -> bool {
    if cert.signature_algorithm_tlv == cert.tbs_signature_algorithm_tlv {
        return true;
    }
    let (Some(outer), Some(inner)) = (SignatureAlgorithm::parse(&cert.signature_algorithm_tlv), SignatureAlgorithm::parse(&cert.tbs_signature_algorithm_tlv)) else {
        errors.add_error(CertError::UnacceptableSignatureAlgorithm);
        return false;
    };
    if outer == inner {
        return true;
    }
    errors.add_error(CertError::SignatureAlgorithmMismatch);
    false
}

fn verify_extended_key_usage(cert: &ParsedCertificate, errors: &mut CertErrors, is_target: bool, is_target_issuer: bool) {
    let ekus = cert.extended_key_usage.as_deref().unwrap_or(&[]);
    let has_any = ekus.iter().any(|e| e == OID_ANY_EKU);
    let mut has_server_auth = ekus.iter().any(|e| e == OID_SERVER_AUTH);
    if has_any && !has_server_auth && !is_target && !is_target_issuer {
        has_server_auth = true;
    }
    if is_target_issuer && cert.extended_key_usage.is_none() {
        has_server_auth = true;
    }
    if cert.extended_key_usage.is_some() && !has_server_auth && !has_any {
        errors.add_error(CertError::EkuLacksServerAuth);
    }
}

#[derive(Clone, Default)]
struct Node {
    parent_policies: Vec<Vec<u8>>,
    mapped: bool,
    reachable: bool,
}

#[derive(Default)]
struct LevelDetails {
    expected_policy_map: BTreeMap<Vec<u8>, Vec<Vec<u8>>>,
    has_any_policy: bool,
}

#[derive(Default)]
struct ValidPolicyGraph {
    levels: Vec<BTreeMap<Vec<u8>, Node>>,
    current: LevelDetails,
}

impl ValidPolicyGraph {
    fn init(&mut self) {
        self.set_null();
        self.start_level();
        self.current.has_any_policy = true;
    }

    fn is_null(&self) -> bool {
        !self.current.has_any_policy && self.levels.last().is_none_or(BTreeMap::is_empty)
    }

    fn set_null(&mut self) {
        self.levels.clear();
        self.current = LevelDetails::default();
    }

    fn start_level(&mut self) -> LevelDetails {
        if let Some(last) = self.levels.last() {
            for (policy, node) in last {
                if !node.mapped {
                    self.current.expected_policy_map.entry(policy.clone()).or_default().push(policy.clone());
                }
            }
        }
        let previous = std::mem::take(&mut self.current);
        self.levels.push(BTreeMap::new());
        previous
    }

    fn user_constrained_policy_set(&mut self, user_initial: &BTreeSet<Vec<u8>>) -> BTreeSet<Vec<u8>> {
        if self.levels.is_empty() {
            return BTreeSet::new();
        }
        let user_has_any = user_initial.contains(cert::OID_ANY_POLICY);
        if self.current.has_any_policy {
            if user_has_any {
                return BTreeSet::from([cert::OID_ANY_POLICY.to_vec()]);
            }
            return user_initial.clone();
        }
        if let Some(last) = self.levels.last_mut() {
            for node in last.values_mut() {
                node.reachable = true;
            }
        }
        let mut out = BTreeSet::new();
        for i in (0..self.levels.len()).rev() {
            let mut parents_to_mark = Vec::new();
            for (policy, node) in &self.levels[i] {
                if !node.reachable {
                    continue;
                }
                if node.parent_policies.is_empty() {
                    if user_has_any || user_initial.contains(policy) {
                        out.insert(policy.clone());
                    }
                } else if i > 0 {
                    parents_to_mark.extend(node.parent_policies.iter().cloned());
                }
            }
            if i > 0 {
                for parent in parents_to_mark {
                    if let Some(node) = self.levels[i - 1].get_mut(&parent) {
                        node.reachable = true;
                    }
                }
            }
        }
        out
    }

    fn add_node(&mut self, policy: Vec<u8>, parent_policies: Vec<Vec<u8>>) {
        if let Some(level) = self.levels.last_mut() {
            level.entry(policy).or_insert(Node { parent_policies, mapped: false, reachable: false });
        }
    }

    fn add_policy_mapping(&mut self, issuer_policy: &[u8], subject_policy: &[u8]) {
        let Some(level) = self.levels.last_mut() else { return };
        if !level.contains_key(issuer_policy) {
            if !self.current.has_any_policy {
                return;
            }
            level.insert(issuer_policy.to_vec(), Node::default());
        }
        if let Some(node) = level.get_mut(issuer_policy) {
            node.mapped = true;
        }
        self.current.expected_policy_map.entry(subject_policy.to_vec()).or_default().push(issuer_policy.to_vec());
    }

    fn delete_node(&mut self, policy: &[u8]) {
        if let Some(level) = self.levels.last_mut() {
            level.remove(policy);
        }
    }
}

struct PathVerifier {
    graph: ValidPolicyGraph,
    user_constrained_policy_set: BTreeSet<Vec<u8>>,
    name_constraints: Vec<Rc<ParsedCertificate>>,
    explicit_policy: usize,
    inhibit_any_policy: usize,
    policy_mapping: usize,
    working_public_key: Option<(PublicKey, Vec<u8>)>,
    working_normalized_issuer: Vec<u8>,
    max_path_length: usize,
}

fn parse_and_check_public_key(spki: &[u8], errors: &mut CertErrors) -> Option<PublicKey> {
    let Some(key) = keys::parse_public_key(spki) else {
        errors.add_error(CertError::FailedParsingSpki);
        return None;
    };
    if !key.is_acceptable(MIN_RSA_MODULUS_BITS) {
        errors.add_error(CertError::UnacceptablePublicKey);
    }
    Some(key)
}

pub fn is_signature_algorithm_acceptable(algorithm: SignatureAlgorithm) -> bool {
    !algorithm.is_sha1()
}

impl PathVerifier {
    fn verify_policies(&mut self, cert: &ParsedCertificate, is_target: bool, errors: &mut CertErrors) {
        if let Some(policy_oids) = &cert.policy_oids {
            if !self.graph.is_null() {
                let mut previous = self.graph.start_level();
                let mut cert_has_any = false;
                for oid in policy_oids {
                    if oid == cert::OID_ANY_POLICY {
                        cert_has_any = true;
                        continue;
                    }
                    if let Some(parents) = previous.expected_policy_map.remove(oid) {
                        self.graph.add_node(oid.clone(), parents);
                    } else if previous.has_any_policy {
                        self.graph.add_node(oid.clone(), Vec::new());
                    }
                }
                if cert_has_any && (self.inhibit_any_policy > 0 || (!is_target && cert.is_self_issued())) {
                    for (oid, parents) in std::mem::take(&mut previous.expected_policy_map) {
                        self.graph.add_node(oid, parents);
                    }
                    if previous.has_any_policy {
                        self.graph.current.has_any_policy = true;
                    }
                }
            }
        } else {
            self.graph.set_null();
        }
        if !(self.explicit_policy > 0 || !self.graph.is_null()) {
            errors.add_error(CertError::NoValidPolicy);
        }
    }

    fn verify_policy_mappings(&mut self, cert: &ParsedCertificate, errors: &mut CertErrors) {
        let Some(mappings) = &cert.policy_mappings else { return };
        if mappings.iter().any(|m| m.issuer_domain == cert::OID_ANY_POLICY || m.subject_domain == cert::OID_ANY_POLICY) {
            self.graph.set_null();
            errors.add_error(CertError::PolicyMappingAnyPolicy);
            return;
        }
        if self.policy_mapping > 0 {
            for mapping in mappings {
                self.graph.add_policy_mapping(&mapping.issuer_domain, &mapping.subject_domain);
            }
        } else {
            for mapping in mappings {
                self.graph.delete_node(&mapping.issuer_domain);
            }
        }
    }

    fn apply_policy_constraints(&mut self, cert: &ParsedCertificate) {
        if let Some(constraints) = &cert.policy_constraints {
            if let Some(require) = constraints.require_explicit_policy {
                self.explicit_policy = self.explicit_policy.min(usize::from(require));
            }
            if let Some(inhibit) = constraints.inhibit_policy_mapping {
                self.policy_mapping = self.policy_mapping.min(usize::from(inhibit));
            }
        }
        if let Some(skip) = cert.inhibit_any_policy {
            self.inhibit_any_policy = self.inhibit_any_policy.min(usize::from(skip));
        }
    }

    fn basic_certificate_processing(&mut self, cert: &ParsedCertificate, is_target: bool, is_target_issuer: bool, time: &GeneralizedTime, errors: &mut CertErrors) -> bool {
        let mut shortcircuit = !verify_signature_algorithms_match(cert, errors);
        let Some(algorithm) = cert.signature_algorithm.filter(|a| is_signature_algorithm_acceptable(*a)) else {
            errors.add_error(CertError::UnacceptableSignatureAlgorithm);
            return true;
        };
        match &self.working_public_key {
            Some((key, spki)) => {
                if !keys::verify_signed_data(algorithm, &cert.tbs_tlv, cert.signature(), key, spki) {
                    shortcircuit = true;
                    errors.add_error(CertError::VerifySignedDataFailed);
                }
            }
            None => shortcircuit = true,
        }
        if shortcircuit {
            return true;
        }
        verify_time_validity(cert, time, errors);
        if cert.normalized_issuer != self.working_normalized_issuer {
            errors.add_error(CertError::SubjectDoesNotMatchIssuer);
        }
        if !self.name_constraints.is_empty() && (!cert.is_self_issued() || is_target) {
            for constraining in &self.name_constraints {
                let constraints: &NameConstraints = constraining.name_constraints.as_ref().expect("only certificates with name constraints are queued");
                constraints.check(&cert.normalized_subject, cert.subject_alt_names.as_ref(), errors);
            }
        }
        self.verify_policies(cert, is_target, errors);
        verify_extended_key_usage(cert, errors, is_target, is_target_issuer);
        false
    }

    fn prepare_for_next_certificate(&mut self, cert: &Rc<ParsedCertificate>, errors: &mut CertErrors) {
        self.verify_policy_mappings(cert, errors);
        self.working_normalized_issuer = cert.normalized_subject.clone();
        self.working_public_key = parse_and_check_public_key(&cert.spki_tlv, errors).map(|k| (k, cert.spki_tlv.clone()));
        if cert.name_constraints.is_some() {
            self.name_constraints.push(cert.clone());
        }
        if !cert.is_self_issued() {
            self.explicit_policy = self.explicit_policy.saturating_sub(1);
            self.policy_mapping = self.policy_mapping.saturating_sub(1);
            self.inhibit_any_policy = self.inhibit_any_policy.saturating_sub(1);
        }
        self.apply_policy_constraints(cert);
        match cert.basic_constraints {
            None => errors.add_error(CertError::MissingBasicConstraints),
            Some(bc) if !bc.is_ca => errors.add_error(CertError::BasicConstraintsIndicatesNotCa),
            Some(_) => {}
        }
        if !cert.is_self_issued() {
            if self.max_path_length == 0 {
                errors.add_error(CertError::MaxPathLengthViolated);
            } else {
                self.max_path_length -= 1;
            }
        }
        if let Some(path_len) = cert.basic_constraints.and_then(|bc| bc.path_len) {
            self.max_path_length = self.max_path_length.min(usize::from(path_len));
        }
        if cert.key_usage.as_ref().is_some_and(|ku| !ku.asserts(cert::KEY_USAGE_KEY_CERT_SIGN)) {
            errors.add_error(CertError::KeyCertSignBitNotSet);
        }
        verify_no_unconsumed_critical_extensions(cert, errors);
    }

    fn wrap_up(&mut self, cert: &ParsedCertificate, user_initial: &BTreeSet<Vec<u8>>, errors: &mut CertErrors) {
        self.explicit_policy = self.explicit_policy.saturating_sub(1);
        if cert.policy_constraints.and_then(|c| c.require_explicit_policy) == Some(0) {
            self.explicit_policy = 0;
        }
        verify_no_unconsumed_critical_extensions(cert, errors);
        self.user_constrained_policy_set = self.graph.user_constrained_policy_set(user_initial);
        if self.explicit_policy == 0 && self.user_constrained_policy_set.is_empty() {
            errors.add_error(CertError::NoValidPolicy);
        }
        parse_and_check_public_key(&cert.spki_tlv, errors);
    }

    fn apply_trust_anchor_constraints(&mut self, cert: &Rc<ParsedCertificate>, errors: &mut CertErrors) {
        if cert.policy_oids.is_some() {
            self.verify_policies(cert, false, errors);
        }
        self.verify_policy_mappings(cert, errors);
        self.apply_policy_constraints(cert);
        if cert.key_usage.as_ref().is_some_and(|ku| !ku.asserts(cert::KEY_USAGE_KEY_CERT_SIGN)) {
            errors.add_error(CertError::KeyCertSignBitNotSet);
        }
        verify_extended_key_usage(cert, errors, false, false);
        if cert.name_constraints.is_some() {
            self.name_constraints.push(cert.clone());
        }
        if let Some(bc) = cert.basic_constraints {
            if !bc.is_ca {
                errors.add_error(CertError::BasicConstraintsIndicatesNotCa);
            }
            if let Some(path_len) = bc.path_len {
                self.max_path_length = usize::from(path_len);
            }
        }
        verify_no_unconsumed_critical_extensions(cert, errors);
    }

    fn process_root(&mut self, cert: &Rc<ParsedCertificate>, trust: &Trust, time: &GeneralizedTime, errors: &mut CertErrors) -> bool {
        match trust.kind {
            TrustType::Unspecified | TrustType::TrustedLeaf => {
                errors.add_error(CertError::CertIsNotTrustAnchor);
                return true;
            }
            TrustType::Distrusted => {
                errors.add_error(CertError::DistrustedByTrustStore);
                return true;
            }
            TrustType::TrustedAnchor | TrustType::TrustedAnchorOrLeaf => {}
        }
        if trust.enforce_anchor_expiry {
            verify_time_validity(cert, time, errors);
        }
        if trust.enforce_anchor_constraints {
            self.apply_trust_anchor_constraints(cert, errors);
        }
        self.working_public_key = parse_and_check_public_key(&cert.spki_tlv, errors).map(|k| (k, cert.spki_tlv.clone()));
        self.working_normalized_issuer = cert.normalized_subject.clone();
        false
    }
}

fn process_single_cert_chain(cert: &ParsedCertificate, trust: &Trust, time: &GeneralizedTime, errors: &mut CertErrors) {
    match trust.kind {
        TrustType::Unspecified | TrustType::TrustedAnchor => {
            errors.add_error(CertError::CertIsNotTrustAnchor);
            return;
        }
        TrustType::Distrusted => {
            errors.add_error(CertError::DistrustedByTrustStore);
            return;
        }
        TrustType::TrustedLeaf | TrustType::TrustedAnchorOrLeaf => {}
    }
    parse_and_check_public_key(&cert.spki_tlv, errors);
    verify_time_validity(cert, time, errors);
    verify_extended_key_usage(cert, errors, true, false);
    verify_no_unconsumed_critical_extensions(cert, errors);
}

pub fn verify_certificate_chain(certs: &[Rc<ParsedCertificate>], trust: &Trust, time: &GeneralizedTime, user_initial: &BTreeSet<Vec<u8>>, errors: &mut CertPathErrors) -> BTreeSet<Vec<u8>> {
    if certs.is_empty() {
        errors.other().add_error(CertError::ChainIsEmpty);
        return BTreeSet::new();
    }
    if certs.len() == 1 {
        process_single_cert_chain(&certs[0], trust, time, errors.for_cert(0));
        return BTreeSet::new();
    }
    let n = certs.len() - 1;
    let mut verifier = PathVerifier {
        graph: ValidPolicyGraph::default(),
        user_constrained_policy_set: BTreeSet::new(),
        name_constraints: Vec::new(),
        explicit_policy: n + 1,
        inhibit_any_policy: n + 1,
        policy_mapping: n + 1,
        working_public_key: None,
        working_normalized_issuer: Vec::new(),
        max_path_length: n,
    };
    verifier.graph.init();
    for i in 0..certs.len() {
        let index = certs.len() - i - 1;
        let cert = &certs[index];
        let cert_errors = errors.for_cert(index);
        if i == 0 {
            if verifier.process_root(cert, trust, time, cert_errors) {
                return BTreeSet::new();
            }
            continue;
        }
        if verifier.basic_certificate_processing(cert, index == 0, index == 1, time, cert_errors) {
            return BTreeSet::new();
        }
        if index != 0 {
            verifier.prepare_for_next_certificate(cert, cert_errors);
        } else {
            verifier.wrap_up(cert, user_initial, cert_errors);
        }
    }
    verifier.user_constrained_policy_set
}
