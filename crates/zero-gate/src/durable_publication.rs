//! Journal-bound durable publication gate.
//!
//! The two-phase kernel can release an in-memory buffered commit without a
//! filesystem claim. A durable claim requires this additional gate and native
//! profile evidence. Rename alone is never accepted as durable publication.

use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zero_abi::{canonical_json, DigestV1 as AbiDigestV1};
use zero_cert::{CompletenessWitness, Query, VerifiedEvidence};
use zero_store::{
    DurableProfileIdV1, DurableProfileV1, JournalBindingV1, JournalFailureCodeV1, PublishedRootV1,
    RecoveryOutcomeV1, RecoveryReceiptV1,
};

use crate::two_phase::{
    validate_receipt_record, CommitReceipt, PublicationDurabilityV1, PublishedCommit, ReceiptKind,
    ReceiptRecord,
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
mod tests {
    use std::{borrow::Cow, collections::BTreeMap};

    use super::*;
    use tempfile::tempdir;
    use zero_cert::{
        verify, EvidenceCertificate, ObjectId, OperatorLock, Provenance, Resolver, SpanRef, TestId,
    };
    use zero_store::{
        commit_journal_v1, initialize_published_root_v1, prepare_journal_v1, JournalPathsV1,
    };

    use crate::two_phase::{
        candidate_protocol_identity_v1, seal_receipt_record_for_test, AttributionClass,
        ExecutionBinding, ExecutionSurface, ResourceUsage, RestorationAccounting, SourceHead,
        WorkerEnvelope, TWO_PHASE_SCHEMA_VERSION,
    };
    use crate::{
        ExactNeutralCertificateV1, FrozenBaselineV1, QualityAdmissionV1, QualityEvidenceV1,
        ReasoningSafepointV1, ReasoningStateStatusV1, SemanticCutCertificateRecordV1,
        SemanticCutClaimV1, SemanticCutEvidenceV1,
    };
    use zero_abi::{
        raw_worker::EffectClass, verify_strict_no_downshift_v1, NativeStatePolicyV1,
        ReasoningContractV1,
    };

    fn abi(byte: u8) -> AbiDigestV1 {
        AbiDigestV1::from_bytes([byte; 32])
    }
    fn paths(directory: &std::path::Path) -> JournalPathsV1 {
        JournalPathsV1::new(
            directory.join("root.json"),
            directory.join("journal.json"),
            directory.join("cartridge.json"),
            directory.join("owner.json"),
            directory.join("recovery.json"),
        )
        .unwrap()
    }
    fn reasoning_contract() -> ReasoningContractV1 {
        ReasoningContractV1::new(
            abi(1),
            abi(20),
            abi(21),
            abi(22),
            abi(23),
            "enabled",
            "high",
            8_192,
            4_096,
            2_048,
            1_024,
            NativeStatePolicyV1::ExactRequired,
            false,
            BTreeMap::new(),
        )
        .unwrap()
    }

    fn quality_admission() -> crate::QualityAdmissionRecordV1 {
        let reasoning_contract = reasoning_contract();
        let reasoning_contract_digest = *reasoning_contract.identity_digest().unwrap().as_bytes();
        let binding = ExecutionBinding {
            schema_version: TWO_PHASE_SCHEMA_VERSION,
            assembly_manifest_digest: [2; 32],
            source_tree_digest: [1; 32],
            source_repository_heads: vec![SourceHead {
                repository: "ZeroStack".into(),
                head: "87c8ef5df0699b6345e4a829876b3f086f9c3ae5".into(),
            }],
            image_digest: [1; 32],
            state_snapshot_digest: [1; 32],
            task_fingerprint_digest: [1; 32],
            plan_digest: [1; 32],
            fixed_model_digest: [1; 32],
            baseline_reasoning_contract: reasoning_contract.clone(),
            reasoning_contract,
            baseline_reasoning_contract_digest: reasoning_contract_digest,
            reasoning_contract_digest,
            comparison_identity_digest: [1; 32],
            semantic_cut_verifier_identity_digest: [1; 32],
            predecessor_receipt_head: [1; 32],
        };
        let certificate = ExactNeutralCertificateV1::verify(
            abi(1),
            abi(1),
            abi(3),
            zero_abi::DigestV1::from_bytes(candidate_protocol_identity_v1(&binding)),
            abi(6),
            abi(6),
            abi(7),
            abi(7),
            abi(4),
            abi(4),
        )
        .unwrap();
        QualityAdmissionV1::admit_strict(
            QualityEvidenceV1::ExactNeutral(certificate),
            FrozenBaselineV1::new(abi(3), abi(4), abi(5)).unwrap(),
        )
        .unwrap()
        .record()
    }

    fn semantic_cut_record(reasoning_contract_digest: [u8; 32]) -> SemanticCutCertificateRecordV1 {
        let terminal = |receipt| {
            ReasoningSafepointV1::new(
                [1; 32],
                [2; 32],
                [3; 32],
                reasoning_contract_digest,
                [1; 32],
                [4; 32],
                ReasoningStateStatusV1::ExactPreserved,
                [5; 32],
                [6; 32],
                [7; 32],
                [8; 32],
                [receipt; 32],
            )
            .unwrap()
        };
        let claim = SemanticCutClaimV1::new_exact(
            [1; 32],
            [9; 32],
            [1; 32],
            terminal(10),
            terminal(11),
            [12; 32],
            [12; 32],
            [13; 32],
            [13; 32],
            [14; 32],
            [15; 32],
            [1; 32],
            [1; 32],
            [16; 32],
        )
        .unwrap();
        let bytes = claim.canonical_bytes().unwrap();
        let digest = zero_abi::sha256(&bytes);
        let span = SpanRef {
            object_id: ObjectId(digest),
            object_digest: digest,
            byte_start: 0,
            byte_len: bytes.len() as u64,
            span_digest: digest,
        };
        let certificate = EvidenceCertificate {
            query: Query::TestTrace { test: TestId(9) },
            spans: vec![span],
            payload: Cow::Borrowed(&bytes),
            provenance: Provenance {
                parser_id: "canonical-json".into(),
                parser_version: "1".into(),
                index_id: "native-receipts".into(),
                index_version: "1".into(),
                operator_id: "native-journal".into(),
                operator_version: "1".into(),
            },
            completeness: CompletenessWitness::TestTrace {
                operator: OperatorLock {
                    operator_id: "native-journal".into(),
                    operator_version: "1".into(),
                },
                test: TestId(9),
                exit_code: 0,
                trace_digest: digest,
            },
            input_token_cost: 0,
            backend_work_units: 1,
        };
        let resident = Resident {
            object: ObjectId(digest),
            bytes: &bytes,
        };
        let evidence = verify(&certificate, &resident).unwrap();
        SemanticCutEvidenceV1::verify_owner_scoped(claim, &evidence)
            .unwrap()
            .record()
    }

    fn record() -> ReceiptRecord {
        let reasoning_contract = reasoning_contract();
        let reasoning_contract_digest = *reasoning_contract.identity_digest().unwrap().as_bytes();
        let reasoning_admission =
            verify_strict_no_downshift_v1(&reasoning_contract, &reasoning_contract)
                .unwrap()
                .record();
        let semantic_cut = semantic_cut_record(reasoning_contract_digest);
        let semantic_cut_certificate_digest = semantic_cut.certificate_digest;
        let semantic_cut_verifier_identity_digest = semantic_cut.verifier_identity_digest;
        let terminal_rcq_identity_digest = semantic_cut.claim.terminal_rcq_identity_digest();
        let mut record = ReceiptRecord {
            schema_version: TWO_PHASE_SCHEMA_VERSION,
            kind: ReceiptKind::Commit,
            permit_id: [1; 32],
            binding_digest: [1; 32],
            admission_digest: [1; 32],
            assembly_manifest_digest: [2; 32],
            source_tree_digest: [1; 32],
            source_repository_heads: vec![SourceHead {
                repository: "ZeroStack".into(),
                head: "87c8ef5df0699b6345e4a829876b3f086f9c3ae5".into(),
            }],
            image_digest: [1; 32],
            state_snapshot_digest: [1; 32],
            task_fingerprint_digest: [1; 32],
            plan_digest: [1; 32],
            fixed_model_digest: [1; 32],
            baseline_reasoning_contract: reasoning_contract.clone(),
            reasoning_contract,
            baseline_reasoning_contract_digest: reasoning_contract_digest,
            reasoning_contract_digest,
            reasoning_admission,
            comparison_identity_digest: [1; 32],
            semantic_cut_verifier_identity_digest,
            artifact_set_digest: [1; 32],
            semantic_cut_certificate_digest,
            semantic_cut,
            terminal_rcq_identity_digest,
            snap_certificate_digest: None,
            safety_shield_digest: [1; 32],
            quality_admission: quality_admission(),
            final_quality_selection: crate::QualitySelectionV1::Candidate,
            transaction_receipt_digest: [1; 32],
            deoptimization_execution_receipt_digest: None,
            attribution_class: AttributionClass::Fixed,
            effect_class: EffectClass::ReversibleMutation,
            resource_envelope: WorkerEnvelope {
                fuel: 1,
                deadline_ms: 1,
                io_bytes: 1,
                output_bytes: 1,
                memory_bytes: 1,
                processes: 1,
                risk_units: 1,
                worker_steps: 1,
            },
            surface: ExecutionSurface::Mcp,
            verification_digest: Some([1; 32]),
            output_digest: [1; 32],
            effects_digest: [1; 32],
            resource_usage: ResourceUsage {
                fuel: 1,
                elapsed_ms: 1,
                io_bytes: 1,
                memory_bytes: 1,
                processes: 1,
                risk_units: 1,
                worker_steps: 1,
            },
            predecessor_receipt_head: [1; 32],
            successor_root: [4; 32],
            trace_digest: [1; 32],
            receipt_head: [1; 32],
            failure_code: None,
            restoration: RestorationAccounting {
                attempted: 0,
                completed: 0,
                debt: 0,
            },
        };
        seal_receipt_record_for_test(&mut record);
        record
    }

    struct Resident<'a> {
        object: ObjectId,
        bytes: &'a [u8],
    }
    impl Resolver for Resident<'_> {
        fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
            (*object_id == self.object).then_some(self.bytes)
        }
        fn trusted_operator_version<'a>(&'a self, operator_id: &str) -> Option<&'a str> {
            (operator_id == "native-journal").then_some("1")
        }
        fn trusted_parser_version<'a>(&'a self, parser_id: &str) -> Option<&'a str> {
            (parser_id == "canonical-json").then_some("1")
        }
        fn trusted_index_version<'a>(&'a self, index_id: &str) -> Option<&'a str> {
            (index_id == "native-receipts").then_some("1")
        }
    }

    fn native_receipt(
        checks: Vec<NativeDurabilityCheckV1>,
        result: NativeDurabilityResultV1,
    ) -> NativeDurabilityReceiptV1 {
        NativeDurabilityReceiptV1 {
            schema_version: DURABLE_PUBLICATION_SCHEMA_VERSION_V1,
            durable_profile_id: DurableProfileIdV1::ApfsStrict,
            durable_profile_digest: DurableProfileV1::new(DurableProfileIdV1::ApfsStrict).digest(),
            platform: NativePlatformV1::Macos,
            filesystem: "apfs".into(),
            source_repository_head: "87c8ef5df0699b6345e4a829876b3f086f9c3ae5".into(),
            source_tree_digest: abi(6),
            artifact_digest: abi(7),
            exact_command_digest: abi(8),
            execution_authority_digest: abi(9),
            native_run_id: "native-run-1".into(),
            checks,
            result,
        }
    }

    fn verify_native_receipt(
        receipt: &NativeDurabilityReceiptV1,
    ) -> Result<VerifiedDurableFilesystemEvidenceV1, DurablePublicationErrorV1> {
        let payload = canonical_json(&serde_json::to_value(receipt).unwrap()).into_bytes();
        let digest = zero_abi::sha256(&payload);
        let object = ObjectId(digest);
        let certificate = EvidenceCertificate {
            query: Query::TestTrace { test: TestId(6) },
            spans: vec![SpanRef {
                object_id: object,
                byte_start: 0,
                byte_len: payload.len() as u64,
                object_digest: digest,
                span_digest: digest,
            }],
            payload: Cow::Owned(payload.clone()),
            provenance: Provenance {
                parser_id: "canonical-json".into(),
                parser_version: "1".into(),
                index_id: "native-receipts".into(),
                index_version: "1".into(),
                operator_id: "native-journal".into(),
                operator_version: "1".into(),
            },
            completeness: CompletenessWitness::TestTrace {
                operator: OperatorLock {
                    operator_id: "native-journal".into(),
                    operator_version: "1".into(),
                },
                test: TestId(6),
                exit_code: 0,
                trace_digest: digest,
            },
            input_token_cost: 0,
            backend_work_units: 1,
        };
        let resident = Resident {
            object,
            bytes: &payload,
        };
        let verified = verify(&certificate, &resident).unwrap();
        verify_native_durability_receipt_v1(&verified)
    }

    fn required_checks() -> Vec<NativeDurabilityCheckV1> {
        vec![
            NativeDurabilityCheckV1::FileSync,
            NativeDurabilityCheckV1::AtomicReplace,
            NativeDurabilityCheckV1::DirectorySync,
            NativeDurabilityCheckV1::KillReopen,
        ]
    }

    fn evidence() -> (tempfile::TempDir, DurablePublicationEvidenceV1) {
        let directory = tempdir().unwrap();
        let paths = paths(directory.path());
        let binding = JournalBindingV1::new(
            abi(1),
            abi(2),
            DurableProfileIdV1::ApfsStrict,
            abi(3),
            abi(4),
            abi(5),
        );
        initialize_published_root_v1(&paths, binding.old_root).unwrap();
        let cartridge = prepare_journal_v1(&paths, binding.clone()).unwrap();
        let recovery = commit_journal_v1(&paths, &cartridge).unwrap();
        let root = zero_store::read_published_root_v1(&paths).unwrap();
        let profile = verify_native_receipt(&native_receipt(
            required_checks(),
            NativeDurabilityResultV1::PassedNative,
        ))
        .unwrap();
        (
            directory,
            DurablePublicationEvidenceV1 {
                schema_version: 1,
                journal_binding: binding,
                recovery_receipt: recovery,
                published_root: root,
                filesystem_evidence: profile,
            },
        )
    }

    #[test]
    fn durable_publication_requires_verified_journal_and_profile_evidence() {
        let (_directory, evidence) = evidence();
        assert_ne!(
            verify_durable_publication_v1(&record(), &evidence).unwrap(),
            AbiDigestV1::ZERO
        );
    }

    #[test]
    fn native_receipt_rejects_rename_only_and_not_run_claims() {
        let mut checks = required_checks();
        checks.pop();
        assert_eq!(
            verify_native_receipt(&native_receipt(
                checks,
                NativeDurabilityResultV1::PassedNative,
            ))
            .unwrap_err()
            .code,
            DurablePublicationFailureCodeV1::RenameOnlyEvidence
        );
        assert_eq!(
            verify_native_receipt(&native_receipt(
                required_checks(),
                NativeDurabilityResultV1::NotRun,
            ))
            .unwrap_err()
            .code,
            DurablePublicationFailureCodeV1::UnverifiedNativeEvidence
        );
    }

    #[test]
    fn durable_publication_rejects_invalid_base_kernel_receipt() {
        let (_directory, evidence) = evidence();
        let mut gate = record();
        gate.receipt_head = [0x99; 32];
        assert_eq!(
            verify_durable_publication_v1(&gate, &evidence)
                .unwrap_err()
                .code,
            DurablePublicationFailureCodeV1::InvalidBaseReceipt
        );
    }

    #[test]
    fn durable_publication_rejects_incomplete_recovery_mutant() {
        let (_directory, mut evidence) = evidence();
        evidence.recovery_receipt.promotable = false;
        assert_eq!(
            verify_durable_publication_v1(&record(), &evidence)
                .unwrap_err()
                .code,
            DurablePublicationFailureCodeV1::IncompleteRecovery
        );
    }

    #[test]
    fn durable_publication_rejects_schema_kind_assembly_root_and_profile_mutants() {
        let (_directory, mut evidence) = evidence();
        evidence.schema_version = 2;
        assert_eq!(
            verify_durable_publication_v1(&record(), &evidence)
                .unwrap_err()
                .code,
            DurablePublicationFailureCodeV1::SchemaVersionMismatch
        );
        evidence.schema_version = 1;

        let mut gate = record();
        gate.kind = ReceiptKind::Fallback;
        assert_eq!(
            verify_durable_publication_v1(&gate, &evidence)
                .unwrap_err()
                .code,
            DurablePublicationFailureCodeV1::NonCommitReceipt
        );
        gate = record();
        gate.assembly_manifest_digest = [9; 32];
        assert_eq!(
            verify_durable_publication_v1(&gate, &evidence)
                .unwrap_err()
                .code,
            DurablePublicationFailureCodeV1::AssemblyMismatch
        );
        gate = record();
        gate.successor_root = [9; 32];
        assert_eq!(
            verify_durable_publication_v1(&gate, &evidence)
                .unwrap_err()
                .code,
            DurablePublicationFailureCodeV1::RootMismatch
        );

        evidence.filesystem_evidence.durable_profile_id = DurableProfileIdV1::Ext4XfsStrict;
        evidence.filesystem_evidence.durable_profile_digest =
            DurableProfileV1::new(DurableProfileIdV1::Ext4XfsStrict).digest();
        assert_eq!(
            verify_durable_publication_v1(&record(), &evidence)
                .unwrap_err()
                .code,
            DurablePublicationFailureCodeV1::ProfileMismatch
        );
    }
}
