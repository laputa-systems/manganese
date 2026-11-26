#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustType {
    Unspecified,
    Distrusted,
    TrustedAnchor,
    TrustedAnchorOrLeaf,
    TrustedLeaf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trust {
    pub kind: TrustType,
    pub enforce_anchor_expiry: bool,
    pub enforce_anchor_constraints: bool,
}

impl Trust {
    pub const UNSPECIFIED: Self = Self::of(TrustType::Unspecified);
    pub const DISTRUSTED: Self = Self::of(TrustType::Distrusted);
    pub const ANCHOR: Self = Self::of(TrustType::TrustedAnchor);
    pub const LEAF: Self = Self::of(TrustType::TrustedLeaf);
    pub const ANCHOR_OR_LEAF: Self = Self::of(TrustType::TrustedAnchorOrLeaf);

    const fn of(kind: TrustType) -> Self {
        Self { kind, enforce_anchor_expiry: false, enforce_anchor_constraints: false }
    }

    pub const fn enforcing(self, expiry: bool, constraints: bool) -> Self {
        Self { enforce_anchor_expiry: expiry, enforce_anchor_constraints: constraints, ..self }
    }

    pub fn is_unspecified(&self) -> bool {
        self.kind == TrustType::Unspecified
    }

    pub fn is_anchor(&self) -> bool {
        matches!(self.kind, TrustType::TrustedAnchor | TrustType::TrustedAnchorOrLeaf)
    }
}
