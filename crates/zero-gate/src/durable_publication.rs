//! Journal-bound durable publication gate.
//!
//! The two-phase kernel can release an in-memory buffered commit without a
//! filesystem claim. A durable claim requires this additional gate and native
//! profile evidence. Rename alone is never accepted as durable publication.

use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zero_abi::{DigestV1 as AbiDigestV1, canonical_json};
use zero_cert::{CompletenessWitness, Query, VerifiedEvidence};
use zero_store::{
    DurableProfileIdV1, DurableProfileV1, JournalBindingV1, JournalFailureCodeV1, PublishedRootV1,
    RecoveryOutcomeV1, RecoveryReceiptV1,
};

use crate::two_phase::{
    CommitReceipt, PublicationDurabilityV1, PublishedCommit, ReceiptKind, ReceiptRecord,
    validate_receipt_record,
};

const DURABLE_PUBLICATION_DOMAIN_V1: &[u8] = b"zerostack.durable_publication.v1\0";
const NATIVE_DURABILITY_RECEIPT_DOMAIN_V1: &[u8] = b"zerostack.native_durability_receipt.v1\0";
pub const DURABLE_PUBLICATION_SCHEMA_VERSION_V1: u16 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurablePublicationFailureCodeV1 {
    SchemaVersionMismatch,
    NonCommitReceipt,
    InvalidBaseReceipt,
    AssemblyMismatch,
    BindingMismatch,
    RootMismatch,
    IncompleteRecovery,
    ProfileMismatch,
    UnverifiedNativeEvidence,
    RenameOnlyEvidence,
}
impl DurablePublicationFailureCodeV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SchemaVersionMismatch => "schema_version_mismatch",
            Self::NonCommitReceipt => "non_commit_receipt",
            Self::InvalidBaseReceipt => "invalid_base_receipt",
            Self::AssemblyMismatch => "assembly_mismatch",
            Self::BindingMismatch => "binding_mismatch",
            Self::RootMismatch => "root_mismatch",
            Self::IncompleteRecovery => "incomplete_recovery",
            Self::ProfileMismatch => "profile_mismatch",
            Self::UnverifiedNativeEvidence => "unverified_native_evidence",
            Self::RenameOnlyEvidence => "rename_only_evidence",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurablePublicationErrorV1 {
    pub code: DurablePublicationFailureCodeV1,
    pub journal_code: Option<JournalFailureCodeV1>,
    pub detail: String,
}
impl DurablePublicationErrorV1 {
    fn new(code: DurablePublicationFailureCodeV1, detail: impl Into<String>) -> Self {
        Self {
            code,
            journal_code: None,
            detail: detail.into(),
        }
    }
}
impl fmt::Display for DurablePublicationErrorV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}
impl std::error::Error for DurablePublicationErrorV1 {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeDurabilityCheckV1 {
    FileSync,
    AtomicReplace,
    DirectorySync,
    KillReopen,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativePlatformV1 {
    Linux,
    Macos,
    Windows,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeDurabilityResultV1 {
    PassedNative,
    NotRun,
}

/// Canonical payload produced by a native journal runner.
///
/// This receipt is data, not authority. The durable gate accepts it only after
/// `zero-cert` has returned `VerifiedEvidence` for the exact payload bytes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeDurabilityReceiptV1 {
    pub schema_version: u16,
    pub durable_profile_id: DurableProfileIdV1,
    pub durable_profile_digest: AbiDigestV1,
    pub platform: NativePlatformV1,
    pub filesystem: String,
    pub source_repository_head: String,
    pub source_tree_digest: AbiDigestV1,
    pub artifact_digest: AbiDigestV1,
    pub exact_command_digest: AbiDigestV1,
    pub execution_authority_digest: AbiDigestV1,
    pub native_run_id: String,
    pub checks: Vec<NativeDurabilityCheckV1>,
    pub result: NativeDurabilityResultV1,
}

/// Owned result of verifying a native receipt through `zero-cert`.
///
/// Fields are private so callers cannot turn prose or booleans into trusted
/// durability evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedDurableFilesystemEvidenceV1 {
    durable_profile_id: DurableProfileIdV1,
    durable_profile_digest: AbiDigestV1,
    filesystem: String,
    native_receipt_digest: AbiDigestV1,
    certificate_digest: AbiDigestV1,
}
impl VerifiedDurableFilesystemEvidenceV1 {
    pub const fn durable_profile_id(&self) -> DurableProfileIdV1 {
        self.durable_profile_id
    }
    pub const fn durable_profile_digest(&self) -> AbiDigestV1 {
        self.durable_profile_digest
    }
    pub fn filesystem(&self) -> &str {
        &self.filesystem
    }
    pub const fn native_receipt_digest(&self) -> AbiDigestV1 {
        self.native_receipt_digest
    }
    pub const fn certificate_digest(&self) -> AbiDigestV1 {
        self.certificate_digest
    }
}

pub fn verify_native_durability_receipt_v1(
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<VerifiedDurableFilesystemEvidenceV1, DurablePublicationErrorV1> {
    if !matches!(evidence.query(), Query::TestTrace { .. })
        || !matches!(
            &evidence.certificate().completeness,
            CompletenessWitness::TestTrace { exit_code: 0, .. }
        )
    {
        return Err(DurablePublicationErrorV1::new(
            DurablePublicationFailureCodeV1::UnverifiedNativeEvidence,
            "native durability requires a successful verified test trace",
        ));
    }
    let receipt: NativeDurabilityReceiptV1 =
        serde_json::from_slice(evidence.payload()).map_err(|error| {
            DurablePublicationErrorV1::new(
                DurablePublicationFailureCodeV1::UnverifiedNativeEvidence,
                format!("native receipt decode failed: {error}"),
            )
        })?;
    let canonical = canonical_json(&serde_json::to_value(&receipt).map_err(|error| {
        DurablePublicationErrorV1::new(
            DurablePublicationFailureCodeV1::UnverifiedNativeEvidence,
            format!("native receipt serialization failed: {error}"),
        )
    })?);
    if canonical.as_bytes() != evidence.payload() {
        return Err(DurablePublicationErrorV1::new(
            DurablePublicationFailureCodeV1::UnverifiedNativeEvidence,
            "native receipt bytes are not canonical JSON",
        ));
    }
    if receipt.schema_version != DURABLE_PUBLICATION_SCHEMA_VERSION_V1
        || receipt.result != NativeDurabilityResultV1::PassedNative
    {
        return Err(DurablePublicationErrorV1::new(
            DurablePublicationFailureCodeV1::UnverifiedNativeEvidence,
            "native receipt is not a supported passed-native result",
        ));
    }
    let expected_profile = DurableProfileV1::new(receipt.durable_profile_id).digest();
    if receipt.durable_profile_digest != expected_profile {
        return Err(DurablePublicationErrorV1::new(
            DurablePublicationFailureCodeV1::ProfileMismatch,
            "native receipt profile digest does not match its frozen profile",
        ));
    }
    let profile_matches = match (receipt.durable_profile_id, receipt.platform) {
        (DurableProfileIdV1::ApfsStrict, NativePlatformV1::Macos) => receipt.filesystem == "apfs",
        (DurableProfileIdV1::Ext4XfsStrict, NativePlatformV1::Linux) => {
            matches!(receipt.filesystem.as_str(), "ext4" | "xfs")
        }
        (DurableProfileIdV1::NtfsStrict, NativePlatformV1::Windows) => receipt.filesystem == "ntfs",
        (DurableProfileIdV1::PortableStrict, _) => false,
        _ => false,
    };
    if !profile_matches {
        return Err(DurablePublicationErrorV1::new(
            DurablePublicationFailureCodeV1::ProfileMismatch,
            "native platform and filesystem do not match the durable profile",
        ));
    }
    let required = BTreeSet::from([
        NativeDurabilityCheckV1::FileSync,
        NativeDurabilityCheckV1::AtomicReplace,
        NativeDurabilityCheckV1::DirectorySync,
        NativeDurabilityCheckV1::KillReopen,
    ]);
    let observed = receipt.checks.iter().copied().collect::<BTreeSet<_>>();
    if receipt.checks.len() != required.len() || observed != required {
        return Err(DurablePublicationErrorV1::new(
            DurablePublicationFailureCodeV1::RenameOnlyEvidence,
            "native receipt does not contain every required durability check exactly once",
        ));
    }
    if receipt.source_repository_head.len() != 40
        || !receipt
            .source_repository_head
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || receipt.source_tree_digest == AbiDigestV1::ZERO
        || receipt.artifact_digest == AbiDigestV1::ZERO
        || receipt.exact_command_digest == AbiDigestV1::ZERO
        || receipt.execution_authority_digest == AbiDigestV1::ZERO
        || receipt.native_run_id.is_empty()
        || receipt.native_run_id.len() > 128
        || receipt.native_run_id.chars().any(char::is_control)
    {
        return Err(DurablePublicationErrorV1::new(
            DurablePublicationFailureCodeV1::UnverifiedNativeEvidence,
            "native receipt provenance bindings are incomplete",
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(NATIVE_DURABILITY_RECEIPT_DOMAIN_V1);
    hasher.update((canonical.len() as u64).to_be_bytes());
    hasher.update(canonical.as_bytes());
    let native_receipt_digest = AbiDigestV1::from_bytes(hasher.finalize().into());
    let certificate_digest =
        AbiDigestV1::from_bytes(evidence.certificate().canonical_digest().map_err(|error| {
            DurablePublicationErrorV1::new(
                DurablePublicationFailureCodeV1::UnverifiedNativeEvidence,
                format!("evidence certificate serialization failed: {error}"),
            )
        })?);
    Ok(VerifiedDurableFilesystemEvidenceV1 {
        durable_profile_id: receipt.durable_profile_id,
        durable_profile_digest: receipt.durable_profile_digest,
        filesystem: receipt.filesystem,
        native_receipt_digest,
        certificate_digest,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurablePublicationEvidenceV1 {
    pub schema_version: u16,
    pub journal_binding: JournalBindingV1,
    pub recovery_receipt: RecoveryReceiptV1,
    pub published_root: PublishedRootV1,
    pub filesystem_evidence: VerifiedDurableFilesystemEvidenceV1,
}
impl DurablePublicationEvidenceV1 {
    pub fn digest(&self) -> Result<AbiDigestV1, DurablePublicationErrorV1> {
        let binding = self.journal_binding.digest().map_err(|error| {
            DurablePublicationErrorV1::new(
                DurablePublicationFailureCodeV1::BindingMismatch,
                error.to_string(),
            )
        })?;
        let recovery = self.recovery_receipt.digest().map_err(|error| {
            DurablePublicationErrorV1::new(
                DurablePublicationFailureCodeV1::IncompleteRecovery,
                error.to_string(),
            )
        })?;
        let root = self.published_root.digest().map_err(|error| {
            DurablePublicationErrorV1::new(
                DurablePublicationFailureCodeV1::RootMismatch,
                error.to_string(),
            )
        })?;
        let mut hasher = Sha256::new();
        hasher.update(DURABLE_PUBLICATION_DOMAIN_V1);
        hasher.update(self.schema_version.to_be_bytes());
        hasher.update(binding.as_bytes());
        hasher.update(recovery.as_bytes());
        hasher.update(root.as_bytes());
        hasher.update(
            self.filesystem_evidence
                .durable_profile_id()
                .as_str()
                .as_bytes(),
        );
        hasher.update(self.filesystem_evidence.durable_profile_digest().as_bytes());
        hasher.update((self.filesystem_evidence.filesystem().len() as u64).to_be_bytes());
        hasher.update(self.filesystem_evidence.filesystem().as_bytes());
        hasher.update(self.filesystem_evidence.native_receipt_digest().as_bytes());
        hasher.update(self.filesystem_evidence.certificate_digest().as_bytes());
        Ok(AbiDigestV1::from_bytes(hasher.finalize().into()))
    }
}

/// Verifies that a two-phase commit has complete journal and native profile evidence.
pub fn verify_durable_publication_v1(
    record: &ReceiptRecord,
    evidence: &DurablePublicationEvidenceV1,
) -> Result<AbiDigestV1, DurablePublicationErrorV1> {
    if evidence.schema_version != DURABLE_PUBLICATION_SCHEMA_VERSION_V1 {
        return Err(DurablePublicationErrorV1::new(
            DurablePublicationFailureCodeV1::SchemaVersionMismatch,
            "durable publication evidence schema is not supported",
        ));
    }
    if record.kind != ReceiptKind::Commit || record.failure_code.is_some() {
        return Err(DurablePublicationErrorV1::new(
            DurablePublicationFailureCodeV1::NonCommitReceipt,
            "only a successful commit receipt can claim durable publication",
        ));
    }
    evidence
        .journal_binding
        .validate()
        .map_err(|error| DurablePublicationErrorV1 {
            code: DurablePublicationFailureCodeV1::BindingMismatch,
            journal_code: Some(error.code),
            detail: error.to_string(),
        })?;
    let gate_assembly = AbiDigestV1::from_bytes(record.assembly_manifest_digest);
    if gate_assembly != evidence.journal_binding.assembly_manifest_digest {
        return Err(DurablePublicationErrorV1::new(
            DurablePublicationFailureCodeV1::AssemblyMismatch,
            "gate and journal assembly manifests differ",
        ));
    }
    let gate_root = AbiDigestV1::from_bytes(record.successor_root);
    if gate_root != evidence.journal_binding.new_root {
        return Err(DurablePublicationErrorV1::new(
            DurablePublicationFailureCodeV1::RootMismatch,
            "gate successor root differs from journal new root",
        ));
    }
    let expected_binding = evidence.journal_binding.digest().map_err(|error| {
        DurablePublicationErrorV1::new(
            DurablePublicationFailureCodeV1::BindingMismatch,
            error.to_string(),
        )
    })?;
    let recovery = &evidence.recovery_receipt;
    if recovery.binding_digest != expected_binding
        || !recovery.promotable
        || !recovery.journal_root_correspondence
        || recovery.failure_code.is_some()
        || recovery.terminal_record_digest.is_none()
        || !matches!(
            recovery.outcome,
            RecoveryOutcomeV1::NewRootCommitted | RecoveryOutcomeV1::AlreadyCommitted
        )
    {
        return Err(DurablePublicationErrorV1::new(
            DurablePublicationFailureCodeV1::IncompleteRecovery,
            "recovery receipt does not prove a complete new-root commit",
        ));
    }
    let root = &evidence.published_root;
    if root.root_digest != evidence.journal_binding.new_root
        || root.transaction_id != evidence.journal_binding.transaction_id
        || root.prepared_record_digest != recovery.prepared_record_digest
    {
        return Err(DurablePublicationErrorV1::new(
            DurablePublicationFailureCodeV1::RootMismatch,
            "published root and recovery receipt commitments differ",
        ));
    }
    let profile = &evidence.filesystem_evidence;
    if profile.durable_profile_id() != evidence.journal_binding.durable_profile_id
        || profile.durable_profile_digest() != evidence.journal_binding.durable_profile_digest
    {
        return Err(DurablePublicationErrorV1::new(
            DurablePublicationFailureCodeV1::ProfileMismatch,
            "verified native profile evidence differs from journal profile",
        ));
    }
    validate_receipt_record(record).map_err(|error| {
        DurablePublicationErrorV1::new(
            DurablePublicationFailureCodeV1::InvalidBaseReceipt,
            format!("base kernel receipt is invalid: {error}"),
        )
    })?;
    evidence.digest()
}

impl CommitReceipt {
    /// Releases a commit marked journal-verified only after the durable gate passes.
    pub fn publish_durable(
        self,
        evidence: &DurablePublicationEvidenceV1,
    ) -> Result<PublishedCommit, DurablePublicationErrorV1> {
        let evidence_digest = verify_durable_publication_v1(&self.record(), evidence)?;
        let mut published = self.publish();
        published.durability = PublicationDurabilityV1::JournalVerified {
            evidence_digest: *evidence_digest.as_bytes(),
            durable_profile_digest: *evidence
                .filesystem_evidence
                .durable_profile_digest()
                .as_bytes(),
        };
        Ok(published)
    }
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-gate/unit/durable_publication.rs"]
mod tests;
