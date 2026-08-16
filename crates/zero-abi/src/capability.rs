//! Shared, versioned cross-engine ZeroRef capability contract.
//!
//! Source anchors: FSZero `src/core/capability.rs` emits `hash.algo` and
//! `shared_cas.version`; GraphZero
//! `crates/graphzero-store/src/store/zeroref_capability.rs` emits
//! `hash.algorithm` and `shared_cas.layout_version`.

use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

/// The shared capability schema version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilitySchema {
    #[serde(rename = "zeroref-capability/v1")]
    ZeroRefV1,
}

/// Hash algorithms observed in peer descriptors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HashAlgorithm {
    Sha1,
    Sha256,
}

/// The shared-CAS layouts peers may advertise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CasLayout {
    #[serde(rename = "blobs/sha256/<hh>/<hash>")]
    BlobsSha256Hh,
    #[serde(rename = "blobs/sha256/<xx>/<hash>")]
    BlobsSha256Xx,
    #[serde(rename = "objects/<hash>")]
    ObjectsByHash,
}

/// Non-zero shared-CAS layout version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LayoutVersion(NonZeroU64);

impl LayoutVersion {
    pub const V1: Self = Self(NonZeroU64::MIN);

    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Hash identity shared across engines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HashCapability {
    #[serde(alias = "algo")]
    pub algorithm: HashAlgorithm,
}

/// Shared-CAS identity shared across engines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedCasCapability {
    pub layout: CasLayout,
    #[serde(alias = "version")]
    pub layout_version: LayoutVersion,
}

/// Behavior of one ZeroRef fragment dimension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FragmentBehavior {
    Strict,
    ClampEnd,
}

/// Independent fragment behavior for each ZeroRef v1 dimension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FragmentPolicy {
    pub byte: FragmentBehavior,
    pub line_start: FragmentBehavior,
    pub line_end: FragmentBehavior,
}

impl FragmentPolicy {
    pub const ZEROREF_V1: Self = Self {
        byte: FragmentBehavior::Strict,
        line_start: FragmentBehavior::Strict,
        line_end: FragmentBehavior::ClampEnd,
    };
}

/// Canonical peer-owned subset of the ZeroRef capability descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedCapability {
    pub schema: CapabilitySchema,
    pub hash: HashCapability,
    pub shared_cas: SharedCasCapability,
    pub fragments: FragmentPolicy,
}

impl SharedCapability {
    pub const fn zeroref_v1(
        algorithm: HashAlgorithm,
        layout: CasLayout,
        layout_version: LayoutVersion,
    ) -> Self {
        Self {
            schema: CapabilitySchema::ZeroRefV1,
            hash: HashCapability { algorithm },
            shared_cas: SharedCasCapability {
                layout,
                layout_version,
            },
            fragments: FragmentPolicy::ZEROREF_V1,
        }
    }

    /// Reports every independently incompatible peer dimension in stable order.
    #[must_use]
    pub fn compatibility_mismatches(&self, peer: &Self) -> Vec<CapabilityMismatch> {
        let mut mismatches = Vec::new();
        if self.hash.algorithm != peer.hash.algorithm {
            mismatches.push(CapabilityMismatch::HashAlgorithm {
                expected: self.hash.algorithm,
                actual: peer.hash.algorithm,
            });
        }
        if self.shared_cas.layout != peer.shared_cas.layout {
            mismatches.push(CapabilityMismatch::CasLayout {
                expected: self.shared_cas.layout,
                actual: peer.shared_cas.layout,
            });
        }
        if self.shared_cas.layout_version != peer.shared_cas.layout_version {
            mismatches.push(CapabilityMismatch::LayoutVersion {
                expected: self.shared_cas.layout_version,
                actual: peer.shared_cas.layout_version,
            });
        }
        if self.fragments.byte != peer.fragments.byte {
            mismatches.push(CapabilityMismatch::FragmentByte {
                expected: self.fragments.byte,
                actual: peer.fragments.byte,
            });
        }
        if self.fragments.line_start != peer.fragments.line_start {
            mismatches.push(CapabilityMismatch::FragmentLineStart {
                expected: self.fragments.line_start,
                actual: peer.fragments.line_start,
            });
        }
        if self.fragments.line_end != peer.fragments.line_end {
            mismatches.push(CapabilityMismatch::FragmentLineEnd {
                expected: self.fragments.line_end,
                actual: peer.fragments.line_end,
            });
        }
        mismatches
    }
}

/// One typed compatibility mismatch dimension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityMismatch {
    HashAlgorithm {
        expected: HashAlgorithm,
        actual: HashAlgorithm,
    },
    CasLayout {
        expected: CasLayout,
        actual: CasLayout,
    },
    LayoutVersion {
        expected: LayoutVersion,
        actual: LayoutVersion,
    },
    FragmentByte {
        expected: FragmentBehavior,
        actual: FragmentBehavior,
    },
    FragmentLineStart {
        expected: FragmentBehavior,
        actual: FragmentBehavior,
    },
    FragmentLineEnd {
        expected: FragmentBehavior,
        actual: FragmentBehavior,
    },
}

