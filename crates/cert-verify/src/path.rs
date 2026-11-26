use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::rc::Rc;

use crate::cert::ParsedCertificate;
use crate::chain;
use crate::der::GeneralizedTime;
use crate::errors::CertError;
use crate::errors::CertPathErrors;
use crate::trust::Trust;
use crate::trust::TrustType;

#[derive(Default)]
pub struct TrustStoreInMemory {
    entries: Vec<(Rc<ParsedCertificate>, Trust)>,
    distrusted_spkis: Vec<Vec<u8>>,
}

impl TrustStoreInMemory {
    pub fn add(&mut self, cert: Rc<ParsedCertificate>, trust: Trust) {
        self.entries.push((cert, trust));
    }

    pub fn add_distrusted_spki(&mut self, spki: Vec<u8>) {
        self.distrusted_spkis.push(spki);
    }

    fn entry(&self, cert: &ParsedCertificate) -> Option<&Trust> {
        self.entries
            .iter()
            .find(|(candidate, _)| candidate.normalized_subject == cert.normalized_subject && candidate.der == cert.der)
            .map(|(_, trust)| trust)
    }

    pub fn contains(&self, cert: &ParsedCertificate) -> bool {
        self.entry(cert).is_some()
    }

    pub fn trust(&self, cert: &ParsedCertificate) -> Trust {
        if self.distrusted_spkis.contains(&cert.spki_tlv) {
            return Trust::DISTRUSTED;
        }
        self.entry(cert).copied().unwrap_or(Trust::UNSPECIFIED)
    }

    fn issuers_of(&self, cert: &ParsedCertificate, out: &mut Vec<Rc<ParsedCertificate>>) {
        out.extend(self.entries.iter().filter(|(candidate, _)| candidate.normalized_subject == cert.normalized_issuer).map(|(c, _)| c.clone()));
    }
}

pub struct TrustStores<'a> {
    pub additional: &'a TrustStoreInMemory,
    pub chrome: &'a TrustStoreInMemory,
}

impl TrustStores<'_> {
    pub fn trust(&self, cert: &ParsedCertificate) -> Trust {
        let mut result = Trust::UNSPECIFIED;
        for store in [self.additional, self.chrome] {
            let current = store.trust(cert);
            if !current.is_unspecified() {
                result = current;
                if result.kind == TrustType::Distrusted {
                    break;
                }
            }
        }
        result
    }
}

#[derive(Clone)]
struct IssuerEntry {
    cert: Rc<ParsedCertificate>,
    trust: Trust,
    ordering: u8,
}

fn key_identifier_ordering(target: &ParsedCertificate, issuer: &ParsedCertificate, trust: &Trust) -> u8 {
    let key_match = match (&target.authority_key_identifier, &issuer.subject_key_identifier) {
        (Some(aki), Some(ski)) if aki == ski => 0,
        (Some(_), Some(_)) => 2,
        _ => 1,
    };
    match trust.kind {
        TrustType::TrustedAnchor | TrustType::TrustedAnchorOrLeaf => [0, 1, 4][key_match],
        TrustType::Unspecified | TrustType::TrustedLeaf => [2, 3, 5][key_match],
        TrustType::Distrusted => [6, 7, 8][key_match],
    }
}

fn issuer_order(a: &IssuerEntry, b: &IssuerEntry) -> Ordering {
    let a_self = a.cert.is_self_issued();
    let b_self = b.cert.is_self_issued();
    (a.ordering, b_self, b.cert.not_before, b.cert.not_after).cmp(&(b.ordering, a_self, a.cert.not_before, a.cert.not_after))
}

struct CertIssuersIter {
    cert: Rc<ParsedCertificate>,
    issuers: Vec<IssuerEntry>,
    current: usize,
    skipped: usize,
    needs_sort: bool,
    queried: bool,
    present: HashSet<Vec<u8>>,
}

pub struct Sources<'a> {
    pub trust: TrustStores<'a>,
    pub intermediates: &'a [Rc<ParsedCertificate>],
}

impl CertIssuersIter {
    fn new(cert: Rc<ParsedCertificate>) -> Self {
        Self { cert, issuers: Vec::new(), current: 0, skipped: 0, needs_sort: false, queried: false, present: HashSet::new() }
    }

    fn next_issuer(&mut self, sources: &Sources<'_>) -> Option<IssuerEntry> {
        if !self.queried {
            self.queried = true;
            let mut found = Vec::new();
            sources.trust.additional.issuers_of(&self.cert, &mut found);
            sources.trust.chrome.issuers_of(&self.cert, &mut found);
            found.extend(sources.intermediates.iter().filter(|c| c.normalized_subject == self.cert.normalized_issuer).cloned());
            for issuer in found {
                if !self.present.insert(issuer.der.clone()) {
                    continue;
                }
                let trust = sources.trust.trust(&issuer);
                let ordering = key_identifier_ordering(&self.cert, &issuer, &trust);
                self.issuers.push(IssuerEntry { cert: issuer, trust, ordering });
                self.needs_sort = true;
            }
        }
        if self.current >= self.issuers.len() {
            return None;
        }
        if self.needs_sort {
            self.issuers[self.current..].sort_by(issuer_order);
            self.needs_sort = false;
        }
        self.current += 1;
        Some(self.issuers[self.current - 1].clone())
    }

    fn had_non_skipped_issuers(&self) -> bool {
        self.issuers.len() > self.skipped
    }
}

type PathKey = (Vec<u8>, Vec<u8>, Vec<u8>);

fn path_key(cert: &ParsedCertificate) -> PathKey {
    (cert.normalized_subject.clone(), cert.subject_alt_names_value().to_vec(), cert.spki_tlv.clone())
}

struct CertPathIter {
    next: Option<IssuerEntry>,
    path: Vec<CertIssuersIter>,
    present: BTreeSet<PathKey>,
}

pub struct ResultPath {
    pub certs: Vec<Rc<ParsedCertificate>>,
    pub trust: Trust,
    pub errors: CertPathErrors,
    pub delegate: DelegateData,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CtCompliance {
    NotEnoughScts,
    BuildNotTimely,
    DetailsNotAvailable,
}

#[derive(Clone, Copy, Debug)]
pub struct DelegateData {
    pub ct_compliance: CtCompliance,
}

impl ResultPath {
    pub fn trusted_cert(&self) -> Option<&Rc<ParsedCertificate>> {
        match self.trust.kind {
            TrustType::TrustedAnchor | TrustType::TrustedAnchorOrLeaf | TrustType::TrustedLeaf => self.certs.last(),
            TrustType::Unspecified | TrustType::Distrusted => None,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.trusted_cert().is_some() && !self.errors.has_high_severity()
    }
}

enum Step {
    Path,
    Exhausted,
}

impl CertPathIter {
    fn new(target: Rc<ParsedCertificate>, trust: Trust) -> Self {
        Self { next: Some(IssuerEntry { cert: target, trust, ordering: 0 }), path: Vec::new(), present: BTreeSet::new() }
    }

    fn copy_path(&self) -> Vec<Rc<ParsedCertificate>> {
        self.path.iter().map(|node| node.cert.clone()).collect()
    }

    fn pop(&mut self) {
        let node = self.path.pop().expect("pop only follows a non-empty path");
        self.present.remove(&path_key(&node.cert));
    }

    fn next_path(&mut self, sources: &Sources<'_>, out: &mut ResultPath, iterations: &mut u32, max_iterations: u32) -> Step {
        loop {
            let mut entry = match self.next.take() {
                Some(entry) => entry,
                None => {
                    if self.path.is_empty() {
                        return Step::Exhausted;
                    }
                    *iterations += 1;
                    if *iterations > max_iterations {
                        out.certs = self.copy_path();
                        out.errors.other().add_error(CertError::IterationLimitExceeded);
                        return Step::Exhausted;
                    }
                    let back = self.path.last_mut().expect("path is not empty");
                    match back.next_issuer(sources) {
                        Some(entry) => entry,
                        None => {
                            if !back.had_non_skipped_issuers() {
                                out.certs = self.copy_path();
                                out.errors.for_cert(out.certs.len() - 1).add_error(CertError::NoIssuersFound);
                                self.pop();
                                return Step::Path;
                            }
                            self.pop();
                            continue;
                        }
                    }
                }
            };
            match entry.trust.kind {
                TrustType::TrustedAnchor if self.path.is_empty() => entry.trust = Trust::UNSPECIFIED,
                TrustType::TrustedLeaf if !self.path.is_empty() => entry.trust = Trust::UNSPECIFIED,
                _ => {}
            }
            if entry.trust.is_unspecified() {
                let key = path_key(&entry.cert);
                if self.present.contains(&key) {
                    self.path.last_mut().expect("a duplicate needs a parent").skipped += 1;
                    continue;
                }
                self.present.insert(key);
                self.path.push(CertIssuersIter::new(entry.cert));
                continue;
            }
            out.certs = self.copy_path();
            out.certs.push(entry.cert);
            out.trust = entry.trust;
            return Step::Path;
        }
    }
}

pub struct BuildResult {
    pub paths: Vec<ResultPath>,
    pub best: usize,
}

impl BuildResult {
    pub fn best_path(&self) -> Option<&ResultPath> {
        self.paths.get(self.best)
    }

    pub fn has_valid_path(&self) -> bool {
        self.best_path().is_some_and(ResultPath::is_valid)
    }

    fn add(&mut self, path: ResultPath) {
        if !self.has_valid_path() {
            let old_best_unspecified = self.best_path().map(|p| p.trust.is_unspecified());
            if path.is_valid() || (!path.trust.is_unspecified() && old_best_unspecified == Some(true)) {
                self.best = self.paths.len();
            }
        }
        self.paths.push(path);
    }
}

pub fn build(target: Rc<ParsedCertificate>, sources: &Sources<'_>, time: &GeneralizedTime, max_iterations: u32, check: &mut dyn FnMut(&mut ResultPath)) -> BuildResult {
    let target_trust = sources.trust.trust(&target);
    let mut iter = CertPathIter::new(target, target_trust);
    let mut result = BuildResult { paths: Vec::new(), best: 0 };
    let mut iterations = 0;
    let any_policy = BTreeSet::from([crate::cert::OID_ANY_POLICY.to_vec()]);
    loop {
        let mut path = ResultPath {
            certs: Vec::new(),
            trust: Trust::UNSPECIFIED,
            errors: CertPathErrors::default(),
            delegate: DelegateData { ct_compliance: CtCompliance::DetailsNotAvailable },
        };
        match iter.next_path(sources, &mut path, &mut iterations, max_iterations) {
            Step::Exhausted => {
                if !path.certs.is_empty() {
                    if !path.errors.has_high_severity() {
                        path.errors.other().add_error(CertError::InternalError);
                    }
                    check(&mut path);
                    result.add(path);
                }
                return result;
            }
            Step::Path => {
                if path.trust.is_unspecified() {
                    if !path.errors.has_high_severity() {
                        path.errors.other().add_error(CertError::InternalError);
                    }
                } else {
                    chain::verify_certificate_chain(&path.certs, &path.trust, time, &any_policy, &mut path.errors);
                }
                check(&mut path);
                let good = path.is_valid();
                result.add(path);
                if good {
                    return result;
                }
            }
        }
    }
}
