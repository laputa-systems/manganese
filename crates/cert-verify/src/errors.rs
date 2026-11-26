#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CertError {
    ValidityFailedNotBefore,
    ValidityFailedNotAfter,
    SignatureAlgorithmMismatch,
    UnacceptableSignatureAlgorithm,
    VerifySignedDataFailed,
    SubjectDoesNotMatchIssuer,
    NotPermittedByNameConstraints,
    TooManyNameConstraintChecks,
    NoValidPolicy,
    PolicyMappingAnyPolicy,
    MissingBasicConstraints,
    BasicConstraintsIndicatesNotCa,
    MaxPathLengthViolated,
    KeyCertSignBitNotSet,
    UnconsumedCriticalExtension,
    FailedParsingSpki,
    UnacceptablePublicKey,
    CertIsNotTrustAnchor,
    DistrustedByTrustStore,
    ChainIsEmpty,
    InternalError,
    NoIssuersFound,
    IterationLimitExceeded,
    EkuLacksServerAuth,
    CertificateRevoked,
    ChromeRootConstraintsFailed,
    CtRequirementsNotMet,
}

#[derive(Clone, Debug, Default)]
pub struct CertErrors {
    errors: Vec<CertError>,
}

impl CertErrors {
    pub fn add_error(&mut self, error: CertError) {
        self.errors.push(error);
    }

    pub fn has_high_severity(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn contains(&self, error: CertError) -> bool {
        self.errors.contains(&error)
    }
}

#[derive(Clone, Debug, Default)]
pub struct CertPathErrors {
    certs: Vec<CertErrors>,
    other: CertErrors,
}

impl CertPathErrors {
    pub fn for_cert(&mut self, index: usize) -> &mut CertErrors {
        if self.certs.len() <= index {
            self.certs.resize_with(index + 1, CertErrors::default);
        }
        &mut self.certs[index]
    }

    pub fn other(&mut self) -> &mut CertErrors {
        &mut self.other
    }

    pub fn has_high_severity(&self) -> bool {
        self.other.has_high_severity() || self.certs.iter().any(CertErrors::has_high_severity)
    }

    pub fn contains(&self, error: CertError) -> bool {
        self.other.contains(error) || self.certs.iter().any(|errors| errors.contains(error))
    }
}
