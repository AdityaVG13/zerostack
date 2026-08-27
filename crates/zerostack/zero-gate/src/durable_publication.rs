//! Journal-bound durable publication gate.
//!
//! The two-phase kernel can release an in-memory buffered commit without a
//! filesystem claim. A durable claim requires this additional gate and native
//! profile evidence. Rename alone is never accepted as durable publication.

use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zero_abi::{Sha256Digest as AbiDigest, canonical_json};
use zero_cert::{CompletenessWitness, Query, VerifiedEvidence};
use zero_store::{
    DurableProfileId, DurableProfile, JournalBinding, JournalFailureCode, PublishedRoot,
    RecoveryOutcome, RecoveryReceipt,
};

use crate::two_phase::{
    CommitReceipt, PublicationDurability, PublishedCommit, ReceiptKind, ReceiptRecord,
    validate_receipt_record,
};

const DURABLE_PUBLICATION_DOMAIN: &[u8] = b"zerostack.durable_publication\0";
const NATIVE_DURABILITY_RECEIPT_DOMAIN: &[u8] = b"zerostack.native_durability_receipt\0";
pub const DURABLE_PUBLICATION_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurablePublicationFailureCode {
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
impl DurablePublicationFailureCode {
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
pub struct DurablePublicationError {
    pub code: DurablePublicationFailureCode,
    pub journal_code: Option<JournalFailureCode>,
    pub detail: String,
}
impl DurablePublicationError {
    fn new(code: DurablePublicationFailureCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            journal_code: None,
            detail: detail.into(),
        }
    }
}
impl fmt::Display for DurablePublicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}
impl std::error::Error for DurablePublicationError {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeDurabilityCheck {
    FileSync,
    AtomicReplace,
    DirectorySync,
    KillReopen,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativePlatform {
    Linux,
    Macos,
    Windows,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeDurabilityResult {
    PassedNative,
    NotRun,
}

/// Canonical payload produced by a native journal runner.
///
/// This receipt is data, not authority. The durable gate accepts it only after
/// `zero-cert` has returned `VerifiedEvidence` for the exact payload bytes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeDurabilityReceipt {
    pub schema_version: u16,
    pub durable_profile_id: DurableProfileId,
    pub durable_profile_digest: AbiDigest,
    pub platform: NativePlatform,
    pub filesystem: String,
    pub source_repository_head: String,
    pub source_tree_digest: AbiDigest,
    pub artifact_digest: AbiDigest,
    pub exact_command_digest: AbiDigest,
    pub execution_authority_digest: AbiDigest,
    pub native_run_id: String,
    pub checks: Vec<NativeDurabilityCheck>,
    pub result: NativeDurabilityResult,
}

/// Owned result of verifying a native receipt through `zero-cert`.
///
/// Fields are private so callers cannot turn prose or booleans into trusted
/// durability evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedDurableFilesystemEvidence {
    durable_profile_id: DurableProfileId,
    durable_profile_digest: AbiDigest,
    filesystem: String,
    native_receipt_digest: AbiDigest,
    certificate_digest: AbiDigest,
}
impl VerifiedDurableFilesystemEvidence {
    pub const fn durable_profile_id(&self) -> DurableProfileId {
        self.durable_profile_id
    }
    pub const fn durable_profile_digest(&self) -> AbiDigest {
        self.durable_profile_digest
    }
    pub fn filesystem(&self) -> &str {
        &self.filesystem
    }
    pub const fn native_receipt_digest(&self) -> AbiDigest {
        self.native_receipt_digest
    }
    pub const fn certificate_digest(&self) -> AbiDigest {
        self.certificate_digest
    }
}

pub fn verify_native_durability_receipt(
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<VerifiedDurableFilesystemEvidence, DurablePublicationError> {
    if !matches!(evidence.query(), Query::TestTrace { .. })
        || !matches!(
            &evidence.certificate().completeness,
            CompletenessWitness::TestTrace { exit_code: 0, .. }
        )
    {
        return Err(DurablePublicationError::new(
            DurablePublicationFailureCode::UnverifiedNativeEvidence,
            "native durability requires a successful verified test trace",
        ));
    }
    let receipt: NativeDurabilityReceipt =
        serde_json::from_slice(evidence.payload()).map_err(|error| {
            DurablePublicationError::new(
                DurablePublicationFailureCode::UnverifiedNativeEvidence,
                format!("native receipt decode failed: {error}"),
            )
        })?;
    let canonical = canonical_json(&serde_json::to_value(&receipt).map_err(|error| {
        DurablePublicationError::new(
            DurablePublicationFailureCode::UnverifiedNativeEvidence,
            format!("native receipt serialization failed: {error}"),
        )
    })?);
    if canonical.as_bytes() != evidence.payload() {
        return Err(DurablePublicationError::new(
            DurablePublicationFailureCode::UnverifiedNativeEvidence,
            "native receipt bytes are not canonical JSON",
        ));
    }
    if receipt.schema_version != DURABLE_PUBLICATION_SCHEMA_VERSION
        || receipt.result != NativeDurabilityResult::PassedNative
    {
        return Err(DurablePublicationError::new(
            DurablePublicationFailureCode::UnverifiedNativeEvidence,
            "native receipt is not a supported passed-native result",
        ));
    }
    let expected_profile = DurableProfile::new(receipt.durable_profile_id).digest();
    if receipt.durable_profile_digest != expected_profile {
        return Err(DurablePublicationError::new(
            DurablePublicationFailureCode::ProfileMismatch,
            "native receipt profile digest does not match its frozen profile",
        ));
    }
    let profile_matches = match (receipt.durable_profile_id, receipt.platform) {
        (DurableProfileId::ApfsStrict, NativePlatform::Macos) => receipt.filesystem == "apfs",
        (DurableProfileId::Ext4XfsStrict, NativePlatform::Linux) => {
            matches!(receipt.filesystem.as_str(), "ext4" | "xfs")
        }
        (DurableProfileId::NtfsStrict, NativePlatform::Windows) => receipt.filesystem == "ntfs",
        (DurableProfileId::PortableStrict, _) => false,
        _ => false,
    };
    if !profile_matches {
        return Err(DurablePublicationError::new(
            DurablePublicationFailureCode::ProfileMismatch,
            "native platform and filesystem do not match the durable profile",
        ));
    }
    let required = BTreeSet::from([
        NativeDurabilityCheck::FileSync,
        NativeDurabilityCheck::AtomicReplace,
        NativeDurabilityCheck::DirectorySync,
        NativeDurabilityCheck::KillReopen,
    ]);
    let observed = receipt.checks.iter().copied().collect::<BTreeSet<_>>();
    if receipt.checks.len() != required.len() || observed != required {
        return Err(DurablePublicationError::new(
            DurablePublicationFailureCode::RenameOnlyEvidence,
            "native receipt does not contain every required durability check exactly once",
        ));
    }
    if receipt.source_repository_head.len() != 40
        || !receipt
            .source_repository_head
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || receipt.source_tree_digest == AbiDigest::ZERO
        || receipt.artifact_digest == AbiDigest::ZERO
        || receipt.exact_command_digest == AbiDigest::ZERO
        || receipt.execution_authority_digest == AbiDigest::ZERO
        || receipt.native_run_id.is_empty()
        || receipt.native_run_id.len() > 128
        || receipt.native_run_id.chars().any(char::is_control)
    {
        return Err(DurablePublicationError::new(
            DurablePublicationFailureCode::UnverifiedNativeEvidence,
            "native receipt provenance bindings are incomplete",
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(NATIVE_DURABILITY_RECEIPT_DOMAIN);
    hasher.update((canonical.len() as u64).to_be_bytes());
    hasher.update(canonical.as_bytes());
    let native_receipt_digest = AbiDigest::from_bytes(hasher.finalize().into());
    let certificate_digest =
        AbiDigest::from_bytes(evidence.certificate().canonical_digest().map_err(|error| {
            DurablePublicationError::new(
                DurablePublicationFailureCode::UnverifiedNativeEvidence,
                format!("evidence certificate serialization failed: {error}"),
            )
        })?);
    Ok(VerifiedDurableFilesystemEvidence {
        durable_profile_id: receipt.durable_profile_id,
        durable_profile_digest: receipt.durable_profile_digest,
        filesystem: receipt.filesystem,
        native_receipt_digest,
        certificate_digest,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurablePublicationEvidence {
    pub schema_version: u16,
    pub journal_binding: JournalBinding,
    pub recovery_receipt: RecoveryReceipt,
    pub published_root: PublishedRoot,
    pub filesystem_evidence: VerifiedDurableFilesystemEvidence,
}
impl DurablePublicationEvidence {
    pub fn digest(&self) -> Result<AbiDigest, DurablePublicationError> {
        let binding = self.journal_binding.digest().map_err(|error| {
            DurablePublicationError::new(
                DurablePublicationFailureCode::BindingMismatch,
                error.to_string(),
            )
        })?;
        let recovery = self.recovery_receipt.digest().map_err(|error| {
            DurablePublicationError::new(
                DurablePublicationFailureCode::IncompleteRecovery,
                error.to_string(),
            )
        })?;
        let root = self.published_root.digest().map_err(|error| {
            DurablePublicationError::new(
                DurablePublicationFailureCode::RootMismatch,
                error.to_string(),
            )
        })?;
        let mut hasher = Sha256::new();
        hasher.update(DURABLE_PUBLICATION_DOMAIN);
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
        Ok(AbiDigest::from_bytes(hasher.finalize().into()))
    }
}

/// Verifies that a two-phase commit has complete journal and native profile evidence.
pub fn verify_durable_publication(
    record: &ReceiptRecord,
    evidence: &DurablePublicationEvidence,
) -> Result<AbiDigest, DurablePublicationError> {
    if evidence.schema_version != DURABLE_PUBLICATION_SCHEMA_VERSION {
        return Err(DurablePublicationError::new(
            DurablePublicationFailureCode::SchemaVersionMismatch,
            "durable publication evidence schema is not supported",
        ));
    }
    if record.kind != ReceiptKind::Commit || record.failure_code.is_some() {
        return Err(DurablePublicationError::new(
            DurablePublicationFailureCode::NonCommitReceipt,
            "only a successful commit receipt can claim durable publication",
        ));
    }
    evidence
        .journal_binding
        .validate()
        .map_err(|error| DurablePublicationError {
            code: DurablePublicationFailureCode::BindingMismatch,
            journal_code: Some(error.code),
            detail: error.to_string(),
        })?;
    let gate_assembly = AbiDigest::from_bytes(record.assembly_manifest_digest);
    if gate_assembly != evidence.journal_binding.assembly_manifest_digest {
        return Err(DurablePublicationError::new(
            DurablePublicationFailureCode::AssemblyMismatch,
            "gate and journal assembly manifests differ",
        ));
    }
    let gate_root = AbiDigest::from_bytes(record.successor_root);
    if gate_root != evidence.journal_binding.new_root {
        return Err(DurablePublicationError::new(
            DurablePublicationFailureCode::RootMismatch,
            "gate successor root differs from journal new root",
        ));
    }
    let expected_binding = evidence.journal_binding.digest().map_err(|error| {
        DurablePublicationError::new(
            DurablePublicationFailureCode::BindingMismatch,
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
            RecoveryOutcome::NewRootCommitted | RecoveryOutcome::AlreadyCommitted
        )
    {
        return Err(DurablePublicationError::new(
            DurablePublicationFailureCode::IncompleteRecovery,
            "recovery receipt does not prove a complete new-root commit",
        ));
    }
    let root = &evidence.published_root;
    if root.root_digest != evidence.journal_binding.new_root
        || root.transaction_id != evidence.journal_binding.transaction_id
        || root.prepared_record_digest != recovery.prepared_record_digest
    {
        return Err(DurablePublicationError::new(
            DurablePublicationFailureCode::RootMismatch,
            "published root and recovery receipt commitments differ",
        ));
    }
    let profile = &evidence.filesystem_evidence;
    if profile.durable_profile_id() != evidence.journal_binding.durable_profile_id
        || profile.durable_profile_digest() != evidence.journal_binding.durable_profile_digest
    {
        return Err(DurablePublicationError::new(
            DurablePublicationFailureCode::ProfileMismatch,
            "verified native profile evidence differs from journal profile",
        ));
    }
    validate_receipt_record(record).map_err(|error| {
        DurablePublicationError::new(
            DurablePublicationFailureCode::InvalidBaseReceipt,
            format!("base kernel receipt is invalid: {error}"),
        )
    })?;
    evidence.digest()
}

impl CommitReceipt {
    /// Releases a commit marked journal-verified only after the durable gate passes.
    pub fn publish_durable(
        self,
        evidence: &DurablePublicationEvidence,
    ) -> Result<PublishedCommit, DurablePublicationError> {
        let evidence_digest = verify_durable_publication(&self.record(), evidence)?;
        let mut published = self.publish();
        published.durability = PublicationDurability::JournalVerified {
            evidence_digest: *evidence_digest.as_bytes(),
            durable_profile_digest: *evidence
                .filesystem_evidence
                .durable_profile_digest()
                .as_bytes(),
        };
        Ok(published)
    }
}

