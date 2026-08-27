//! Explicit GraphZero durability evidence over the canonical ZeroStore journal.
//!
//! Ordinary indexing does not call this module. Callers opt in only after they
//! have an assembly expectation, canonical surface bytes, and a candidate
//! manifest. This module proves the local store transition; it never mints a
//! hub native durability receipt.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zero_abi::{Sha256Digest, canonical_json, sha256};
use zero_store::{
    ContinuationCartridge, DurableProfile, DurableProfileId, FaultPlan, JournalBinding,
    JournalFailureCode, JournalPaths, JournalState, OwnerDeathReceipt, RecoveryOutcome,
    RecoveryReceipt, abort_journal_with_fault, commit_journal_with_fault,
    initialize_published_root, prepare_journal_with_fault, read_continuation_cartridge,
    read_journal_record, read_published_root, record_owner_death, recover_journal,
};

use super::blob_store::BlobStore;
use super::manifest::{Manifest, manifest_path, manifest_prev_path};
use super::refs::{Fragment, GzRef};
use super::shard::file_hash64;
use crate::ContentHash;

pub const DURABILITY_RECEIPT_SCHEMA_VERSION: u16 = 1;
const RECEIPT_DOMAIN: &[u8] = b"graphzero.durability_receipt.v1\0";
const MANIFEST_DOMAIN: &[u8] = b"graphzero.manifest.v1\0";
const JOURNAL_DIR: &str = ".durability";

/// Frozen feeder identities required by the local parity evidence contract.
pub const REQUIRED_FEEDER_IDS: &[&str] = &[
    "hub:Z1",
    "hub:Z4",
    "hub:Z5",
    "hub:Z6",
    "graphzero-zerostack-parity-b5ci.6",
];

/// Metadata supplied by the assembly/verifier caller.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DurabilityMetadata {
    pub source_revision: String,
    pub build_identity: String,
    pub fixture_identity: String,
    pub verifier_identity: String,
    pub assembly_manifest_digest: Sha256Digest,
    pub owner_identity_digest: Sha256Digest,
    pub durable_profile_id: DurableProfileId,
    pub required_feeder_ids: Vec<String>,
    pub available_feeder_ids: Vec<String>,
}

/// Exact caller-controlled values required when verifying a receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DurabilityReceiptExpectation {
    pub source_revision: String,
    pub build_identity: String,
    pub fixture_identity: String,
    pub verifier_identity: String,
    pub assembly_manifest_digest: Sha256Digest,
    pub owner_identity_digest: Sha256Digest,
    pub durable_profile_id: DurableProfileId,
    pub required_feeder_ids: Vec<String>,
    pub available_feeder_ids: Vec<String>,
    pub surface: CanonicalSurfaceBytes,
}

/// Canonical request/result/error/ref bytes exchanged by a surface.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalSurfaceBytes {
    pub request_bytes: Vec<u8>,
    pub result_bytes: Vec<u8>,
    pub error_bytes: Vec<u8>,
    pub ref_bytes: Vec<u8>,
    /// Fallback or unavailable sources cannot become store evidence.
    pub source_is_fallback: bool,
}

/// The manifest and referenced identities bound by a receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestIdentity {
    pub manifest_digest: Sha256Digest,
    pub latest_snapshot_id: Option<u64>,
    pub latest_global_hash: Option<u64>,
    pub latest_shard_hashes: Vec<u64>,
    pub latest_segment_ids: Vec<u64>,
    pub ref_digest: Sha256Digest,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStatus {
    Committed,
    Aborted,
    RecoveredCommitted,
    RecoveredAborted,
}

/// Local store evidence. `native_promotable` is permanently false because
/// NativeDurabilityReceipt belongs to the hub's zero-gate authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DurabilityReceipt {
    pub schema_version: u16,
    pub transaction_id: Sha256Digest,
    pub status: ReceiptStatus,
    pub store_verified: bool,
    pub native_promotable: bool,
    pub metadata: DurabilityMetadata,
    pub surface: CanonicalSurfaceBytes,
    pub old_manifest: ManifestIdentity,
    pub new_manifest: ManifestIdentity,
    pub durable_profile_digest: Sha256Digest,
    pub continuation: ContinuationCartridge,
    pub recovery: RecoveryReceipt,
    pub owner_death: Option<OwnerDeathReceipt>,
}

/// Input for an explicit evidence transaction. The expectation must exactly
/// describe the input, preventing self-authenticated metadata.
#[derive(Clone, Debug)]
pub struct DurabilityEvidenceInput {
    pub transaction_id: Sha256Digest,
    pub metadata: DurabilityMetadata,
    pub surface: CanonicalSurfaceBytes,
    pub candidate_manifest: Manifest,
    pub expectation: DurabilityReceiptExpectation,
}

#[derive(Clone, Debug)]
pub struct DurabilityReceiptAdapter {
    store_root: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PreparedContext {
    transaction_id: Sha256Digest,
    metadata: DurabilityMetadata,
    surface: CanonicalSurfaceBytes,
    old_manifest: ManifestIdentity,
    new_manifest: ManifestIdentity,
    old_manifest_bytes: Vec<u8>,
    new_manifest_bytes: Vec<u8>,
}

pub struct PreparedDurabilityEvidence {
    adapter: DurabilityReceiptAdapter,
    input: DurabilityEvidenceInput,
    expectation: DurabilityReceiptExpectation,
    old_manifest: Manifest,
    old_identity: ManifestIdentity,
    new_identity: ManifestIdentity,
    binding: JournalBinding,
    paths: JournalPaths,
    continuation: ContinuationCartridge,
    owner_death: Option<OwnerDeathReceipt>,
}

impl DurabilityReceiptExpectation {
    pub fn new(metadata: &DurabilityMetadata, surface: &CanonicalSurfaceBytes) -> Self {
        Self {
            source_revision: metadata.source_revision.clone(),
            build_identity: metadata.build_identity.clone(),
            fixture_identity: metadata.fixture_identity.clone(),
            verifier_identity: metadata.verifier_identity.clone(),
            assembly_manifest_digest: metadata.assembly_manifest_digest,
            owner_identity_digest: metadata.owner_identity_digest,
            durable_profile_id: metadata.durable_profile_id,
            required_feeder_ids: metadata.required_feeder_ids.clone(),
            available_feeder_ids: metadata.available_feeder_ids.clone(),
            surface: surface.clone(),
        }
    }

    pub fn from_input(input: &DurabilityEvidenceInput) -> Self {
        Self::new(&input.metadata, &input.surface)
    }

    fn validate(&self) -> Result<()> {
        validate_metadata(&DurabilityMetadata {
            source_revision: self.source_revision.clone(),
            build_identity: self.build_identity.clone(),
            fixture_identity: self.fixture_identity.clone(),
            verifier_identity: self.verifier_identity.clone(),
            assembly_manifest_digest: self.assembly_manifest_digest,
            owner_identity_digest: self.owner_identity_digest,
            durable_profile_id: self.durable_profile_id,
            required_feeder_ids: self.required_feeder_ids.clone(),
            available_feeder_ids: self.available_feeder_ids.clone(),
        })?;
        validate_surface(&self.surface)
    }

    fn matches_input(&self, input: &DurabilityEvidenceInput) -> bool {
        self == &Self::from_input(input)
    }
}

impl DurabilityReceiptAdapter {
    pub fn open(store_root: impl Into<PathBuf>) -> Self {
        Self {
            store_root: store_root.into(),
        }
    }

    /// Persist the canonical continuation and prepared journal before callers
    /// publish the candidate manifest.
    pub fn prepare(&self, input: DurabilityEvidenceInput) -> Result<PreparedDurabilityEvidence> {
        let mut fault = FaultPlan::none();
        self.prepare_with_fault(input, &mut fault)
    }

    pub fn prepare_with_fault(
        &self,
        input: DurabilityEvidenceInput,
        fault: &mut FaultPlan,
    ) -> Result<PreparedDurabilityEvidence> {
        validate_input(&input)?;
        let old_manifest = reopen_manifest(&self.store_root)?;
        let old_identity = manifest_identity(&old_manifest, &[]);
        let new_identity = manifest_identity(&input.candidate_manifest, &input.surface.ref_bytes);
        ensure!(
            old_identity.manifest_digest != new_identity.manifest_digest,
            "candidate manifest must differ from the current manifest"
        );
        verify_manifest_artifacts(
            &self.store_root,
            &input.candidate_manifest,
            &input.surface.ref_bytes,
        )?;
        let binding = JournalBinding::new(
            input.transaction_id,
            input.metadata.assembly_manifest_digest,
            input.metadata.durable_profile_id,
            old_identity.manifest_digest,
            new_identity.manifest_digest,
            input.metadata.owner_identity_digest,
        );
        binding.validate().map_err(anyhow::Error::new)?;
        initialize_or_check_root(&self.store_root, old_identity.manifest_digest)?;
        let context = PreparedContext {
            transaction_id: input.transaction_id,
            metadata: input.metadata.clone(),
            surface: input.surface.clone(),
            old_manifest: old_identity.clone(),
            new_manifest: new_identity.clone(),
            old_manifest_bytes: old_manifest.encode(),
            new_manifest_bytes: input.candidate_manifest.encode(),
        };
        persist_prepared_context(&self.store_root, &context)?;
        let paths = transaction_paths(&self.store_root, input.transaction_id)?;
        let continuation = prepare_journal_with_fault(&paths, binding.clone(), fault)
            .map_err(anyhow::Error::new)
            .context("prepare canonical durable journal")?;
        Ok(PreparedDurabilityEvidence {
            adapter: self.clone(),
            expectation: input.expectation.clone(),
            input,
            old_manifest,
            old_identity,
            new_identity,
            binding,
            paths,
            continuation,
            owner_death: None,
        })
    }

    /// Reconstruct a transaction after process loss from persisted journal and
    /// continuation records. No in-memory prepared token is required.
    pub fn resume(
        &self,
        input: DurabilityEvidenceInput,
        expectation: &DurabilityReceiptExpectation,
        owner_death_observed_at_unix_ns: u64,
    ) -> Result<PreparedDurabilityEvidence> {
        validate_input(&input)?;
        expectation.validate()?;
        ensure!(
            expectation.matches_input(&input),
            "resume expectation differs from input"
        );
        let persisted_context = read_prepared_context(&self.store_root, input.transaction_id)?;
        ensure!(
            persisted_context.metadata == input.metadata
                && persisted_context.surface == input.surface,
            "resume input differs from persisted prepared context"
        );
        let paths = transaction_paths(&self.store_root, input.transaction_id)?;
        let binding = JournalBinding::new(
            input.transaction_id,
            expectation.assembly_manifest_digest,
            expectation.durable_profile_id,
            persisted_context.old_manifest.manifest_digest,
            persisted_context.new_manifest.manifest_digest,
            expectation.owner_identity_digest,
        );
        binding.validate().map_err(anyhow::Error::new)?;
        let journal_state = match read_journal_record(&paths) {
            Ok(journal) => {
                ensure!(
                    journal.binding == binding,
                    "persisted journal binding differs from prepared context"
                );
                Some(journal.state)
            }
            Err(error) if error.code == JournalFailureCode::JournalMissing => None,
            Err(error) => return Err(anyhow::Error::new(error).context("read durable journal")),
        };
        let continuation = read_continuation_cartridge(&paths).map_err(anyhow::Error::new)?;
        continuation
            .validate_against(&binding)
            .map_err(anyhow::Error::new)?;
        let candidate_digest = manifest_digest(&input.candidate_manifest);
        ensure!(
            candidate_digest == binding.new_root,
            "caller candidate is not the journal new root"
        );
        let (old_manifest, new_manifest, actual) = resume_manifests(
            &self.store_root,
            binding.old_root,
            binding.new_root,
            &input.candidate_manifest,
        )?;
        ensure!(
            manifest_digest(&actual) == binding.old_root
                || manifest_digest(&actual) == binding.new_root,
            "resume found partial or unknown manifest root"
        );
        verify_manifest_artifacts(
            &self.store_root,
            &input.candidate_manifest,
            &input.surface.ref_bytes,
        )?;
        let old_identity = manifest_identity(&old_manifest, &[]);
        let new_identity = manifest_identity(&new_manifest, &input.surface.ref_bytes);
        ensure!(
            old_identity.manifest_digest == binding.old_root
                && new_identity.manifest_digest == binding.new_root,
            "resume manifest identities differ from journal"
        );
        ensure!(
            persisted_context
                == (PreparedContext {
                    transaction_id: input.transaction_id,
                    metadata: input.metadata.clone(),
                    surface: input.surface.clone(),
                    old_manifest: old_identity.clone(),
                    new_manifest: new_identity.clone(),
                    old_manifest_bytes: old_manifest.encode(),
                    new_manifest_bytes: new_manifest.encode(),
                }),
            "resume identities differ from persisted prepared context"
        );
        let owner_death = match read_owner_death(&paths)? {
            Some(owner) => {
                ensure!(
                    owner.owner_identity_digest == expectation.owner_identity_digest,
                    "persisted owner-death identity differs from expectation"
                );
                Some(owner)
            }
            None if journal_state == Some(JournalState::Prepared) => Some(
                record_owner_death(
                    &paths,
                    expectation.owner_identity_digest,
                    owner_death_observed_at_unix_ns,
                )
                .map_err(anyhow::Error::new)?,
            ),
            None => None,
        };
        Ok(PreparedDurabilityEvidence {
            adapter: self.clone(),
            input,
            expectation: expectation.clone(),
            old_manifest,
            old_identity,
            new_identity,
            binding,
            paths,
            continuation,
            owner_death,
        })
    }

    pub fn resume_and_recover(
        &self,
        input: DurabilityEvidenceInput,
        expectation: &DurabilityReceiptExpectation,
        owner_death_observed_at_unix_ns: u64,
    ) -> Result<DurabilityReceipt> {
        self.resume(input, expectation, owner_death_observed_at_unix_ns)?
            .recover()
    }

    fn verify_receipt(
        &self,
        receipt: &DurabilityReceipt,
        expectation: &DurabilityReceiptExpectation,
    ) -> Result<()> {
        receipt.validate_shape()?;
        expectation.validate()?;
        ensure!(
            receipt_matches_expectation(receipt, expectation),
            "receipt does not match caller expectation"
        );
        let persisted_context = read_prepared_context(&self.store_root, receipt.transaction_id)?;
        ensure!(
            persisted_context
                == (PreparedContext {
                    transaction_id: receipt.transaction_id,
                    metadata: receipt.metadata.clone(),
                    surface: receipt.surface.clone(),
                    old_manifest: receipt.old_manifest.clone(),
                    new_manifest: receipt.new_manifest.clone(),
                    old_manifest_bytes: persisted_context.old_manifest_bytes.clone(),
                    new_manifest_bytes: persisted_context.new_manifest_bytes.clone(),
                }),
            "receipt differs from persisted prepared context"
        );
        let persisted_old = Manifest::decode(&persisted_context.old_manifest_bytes)
            .context("decode persisted old manifest")?;
        let persisted_new = Manifest::decode(&persisted_context.new_manifest_bytes)
            .context("decode persisted new manifest")?;
        ensure!(
            manifest_identity(&persisted_old, &[]) == receipt.old_manifest,
            "persisted old manifest identity differs from receipt"
        );
        ensure!(
            manifest_identity(&persisted_new, &receipt.surface.ref_bytes) == receipt.new_manifest,
            "persisted new manifest identity differs from receipt"
        );
        let paths = transaction_paths(&self.store_root, receipt.transaction_id)?;
        let binding = JournalBinding::new(
            receipt.transaction_id,
            receipt.metadata.assembly_manifest_digest,
            receipt.metadata.durable_profile_id,
            receipt.old_manifest.manifest_digest,
            receipt.new_manifest.manifest_digest,
            receipt.metadata.owner_identity_digest,
        );
        binding.validate().map_err(anyhow::Error::new)?;
        let continuation = read_continuation_cartridge(&paths).map_err(anyhow::Error::new)?;
        ensure!(
            continuation == receipt.continuation,
            "persisted continuation was altered"
        );
        let journal = read_journal_record(&paths).map_err(anyhow::Error::new)?;
        ensure!(
            journal.binding == binding,
            "persisted journal binding was altered"
        );
        let recovery = recover_journal(&paths, &binding).map_err(anyhow::Error::new)?;
        ensure!(recovery == receipt.recovery, "recovery receipt was altered");
        let (current, previous) = read_manifest_pair(&self.store_root)?;
        let actual_digest = manifest_digest(&current);
        ensure!(
            actual_digest == receipt.old_manifest.manifest_digest
                || actual_digest == receipt.new_manifest.manifest_digest,
            "manifest is neither exact old nor committed new root"
        );
        if let Some(previous) = previous {
            let digest = manifest_digest(&previous);
            ensure!(
                digest == receipt.old_manifest.manifest_digest
                    || digest == receipt.new_manifest.manifest_digest,
                "manifest.prev contains an unknown partial root"
            );
        }
        let committed = matches!(
            receipt.status,
            ReceiptStatus::Committed | ReceiptStatus::RecoveredCommitted
        );
        let expected_root = if committed {
            receipt.new_manifest.manifest_digest
        } else {
            receipt.old_manifest.manifest_digest
        };
        let root =
            read_published_root(&root_paths(&self.store_root)).map_err(anyhow::Error::new)?;
        ensure!(
            root.root_digest == expected_root,
            "published journal root differs from receipt"
        );
        ensure!(
            actual_digest == expected_root,
            "manifest root differs from journal outcome"
        );
        if committed {
            verify_manifest_identity(
                &self.store_root,
                &current,
                &receipt.new_manifest,
                &receipt.surface.ref_bytes,
            )?;
        }
        let persisted_owner = read_owner_death(&paths)?;
        ensure!(
            persisted_owner == receipt.owner_death,
            "owner-death receipt was altered or omitted"
        );
        if let Some(owner) = &receipt.owner_death {
            ensure!(
                receipt.recovery.owner_death_receipt_digest
                    == Some(owner.digest().map_err(anyhow::Error::new)?),
                "owner-death digest is not bound into recovery"
            );
        }
        Ok(())
    }
}

impl PreparedDurabilityEvidence {
    pub fn continuation(&self) -> &ContinuationCartridge {
        &self.continuation
    }

    pub fn record_owner_death(&mut self, observed_at_unix_ns: u64) -> Result<&OwnerDeathReceipt> {
        let owner = record_owner_death(
            &self.paths,
            self.input.metadata.owner_identity_digest,
            observed_at_unix_ns,
        )
        .map_err(anyhow::Error::new)?;
        self.owner_death = Some(owner);
        Ok(self
            .owner_death
            .as_ref()
            .expect("owner death just recorded"))
    }

    pub fn commit(self, candidate: &Manifest) -> Result<DurabilityReceipt> {
        let mut fault = FaultPlan::none();
        self.commit_with_fault(candidate, &mut fault)
    }

    pub fn commit_with_fault(
        self,
        candidate: &Manifest,
        fault: &mut FaultPlan,
    ) -> Result<DurabilityReceipt> {
        ensure!(
            *candidate == self.input.candidate_manifest,
            "candidate differs from prepared evidence"
        );
        verify_manifest_artifacts(
            &self.adapter.store_root,
            candidate,
            &self.input.surface.ref_bytes,
        )?;
        candidate
            .atomic_publish(&self.adapter.store_root)
            .context("publish candidate manifest")?;
        ensure!(
            reopen_manifest(&self.adapter.store_root)? == *candidate,
            "published manifest changed during reopen"
        );
        let recovery = commit_journal_with_fault(&self.paths, &self.continuation, fault)
            .map_err(anyhow::Error::new)
            .context("commit canonical durable journal")?;
        let receipt = self.make_receipt(recovery, ReceiptStatus::Committed);
        self.adapter.verify_receipt(&receipt, &self.expectation)?;
        Ok(receipt)
    }

    pub fn abort(self) -> Result<DurabilityReceipt> {
        ensure!(
            reopen_manifest(&self.adapter.store_root)? == self.old_manifest,
            "cannot abort after a different manifest publish"
        );
        let mut fault = FaultPlan::none();
        let recovery = abort_journal_with_fault(&self.paths, &self.continuation, &mut fault)
            .map_err(anyhow::Error::new)
            .context("abort canonical durable journal")?;
        let receipt = self.make_receipt(recovery, ReceiptStatus::Aborted);
        self.adapter.verify_receipt(&receipt, &self.expectation)?;
        Ok(receipt)
    }

    pub fn recover(self) -> Result<DurabilityReceipt> {
        let recovery = recover_journal(&self.paths, &self.binding)
            .map_err(anyhow::Error::new)
            .context("recover canonical durable journal")?;
        let (status, expected_manifest) = match recovery.outcome {
            RecoveryOutcome::NewRootCommitted | RecoveryOutcome::AlreadyCommitted => (
                ReceiptStatus::RecoveredCommitted,
                &self.input.candidate_manifest,
            ),
            RecoveryOutcome::OldRootAborted
            | RecoveryOutcome::AlreadyAborted
            | RecoveryOutcome::NotStartedOldRoot => {
                (ReceiptStatus::RecoveredAborted, &self.old_manifest)
            }
        };
        let current = reopen_manifest(&self.adapter.store_root)?;
        ensure!(
            manifest_digest(&current) == self.binding.old_root
                || manifest_digest(&current) == self.binding.new_root,
            "recovery found a partial or unknown manifest"
        );
        if current != *expected_manifest {
            expected_manifest
                .atomic_publish(&self.adapter.store_root)
                .context("settle manifest to canonical journal outcome")?;
        }
        ensure!(
            reopen_manifest(&self.adapter.store_root)? == *expected_manifest,
            "manifest did not settle to canonical journal outcome"
        );
        let receipt = self.make_receipt(recovery, status);
        self.adapter.verify_receipt(&receipt, &self.expectation)?;
        Ok(receipt)
    }

    fn make_receipt(&self, recovery: RecoveryReceipt, status: ReceiptStatus) -> DurabilityReceipt {
        DurabilityReceipt {
            schema_version: DURABILITY_RECEIPT_SCHEMA_VERSION,
            transaction_id: self.input.transaction_id,
            status,
            store_verified: true,
            native_promotable: false,
            metadata: self.input.metadata.clone(),
            surface: self.input.surface.clone(),
            old_manifest: self.old_identity.clone(),
            new_manifest: self.new_identity.clone(),
            durable_profile_digest: DurableProfile::new(self.input.metadata.durable_profile_id)
                .digest(),
            continuation: self.continuation.clone(),
            recovery,
            owner_death: self.owner_death.clone(),
        }
    }
}

impl DurabilityReceipt {
    pub fn validate_shape(&self) -> Result<()> {
        ensure!(
            self.schema_version == DURABILITY_RECEIPT_SCHEMA_VERSION,
            "unsupported durability receipt schema"
        );
        ensure!(
            self.transaction_id != Sha256Digest::ZERO,
            "receipt transaction id is missing"
        );
        ensure!(self.store_verified, "receipt is not store-verified");
        ensure!(
            !self.native_promotable,
            "local receipt cannot be promoted to native evidence"
        );
        validate_metadata(&self.metadata)?;
        validate_surface(&self.surface)?;
        ensure!(self.recovery.promotable, "incomplete recovery receipt");
        ensure!(
            self.recovery.journal_root_correspondence,
            "recovery/root correspondence is missing"
        );
        ensure!(
            self.recovery.failure_code.is_none(),
            "recovery has a failure code"
        );
        ensure!(
            self.recovery.binding_digest == self.continuation.binding_digest,
            "recovery binding differs from continuation"
        );
        ensure!(
            self.recovery.prepared_record_digest == self.continuation.prepared_record_digest,
            "recovery prepared digest differs from continuation"
        );
        ensure!(
            self.old_manifest.manifest_digest != Sha256Digest::ZERO,
            "old manifest digest is missing"
        );
        ensure!(
            self.new_manifest.manifest_digest != Sha256Digest::ZERO,
            "new manifest digest is missing"
        );
        ensure!(
            self.old_manifest.manifest_digest != self.new_manifest.manifest_digest,
            "manifest roots must differ"
        );
        ensure!(
            self.durable_profile_digest
                == DurableProfile::new(self.metadata.durable_profile_id).digest(),
            "durable profile substitution"
        );
        ensure!(
            matches!(
                self.status,
                ReceiptStatus::Committed
                    | ReceiptStatus::Aborted
                    | ReceiptStatus::RecoveredCommitted
                    | ReceiptStatus::RecoveredAborted
            ),
            "unknown receipt status"
        );
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.validate_shape()?;
        Ok(canonical_json(&serde_json::to_value(self)?).into_bytes())
    }

    pub fn digest(&self) -> Result<Sha256Digest> {
        let bytes = self.canonical_bytes()?;
        let mut bound = Vec::with_capacity(RECEIPT_DOMAIN.len() + bytes.len());
        bound.extend_from_slice(RECEIPT_DOMAIN);
        bound.extend_from_slice(&bytes);
        Ok(Sha256Digest::from_bytes(sha256(&bound)))
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self> {
        let receipt: Self = serde_json::from_slice(bytes).context("decode durability receipt")?;
        ensure!(
            receipt.canonical_bytes()? == bytes,
            "durability receipt is not canonical"
        );
        Ok(receipt)
    }

    pub fn verify(
        &self,
        store_root: impl AsRef<Path>,
        expectation: &DurabilityReceiptExpectation,
    ) -> Result<()> {
        DurabilityReceiptAdapter::open(store_root.as_ref()).verify_receipt(self, expectation)
    }
}

fn validate_input(input: &DurabilityEvidenceInput) -> Result<()> {
    validate_metadata(&input.metadata)?;
    validate_surface(&input.surface)?;
    ensure!(
        input.transaction_id != Sha256Digest::ZERO,
        "transaction id is missing"
    );
    ensure!(
        input.candidate_manifest.latest().is_some(),
        "candidate manifest has no latest snapshot"
    );
    ensure!(
        input.expectation.matches_input(input),
        "input expectation does not exactly match metadata/surface"
    );
    input.expectation.validate()
}

fn validate_metadata(metadata: &DurabilityMetadata) -> Result<()> {
    for (name, value) in [
        ("source revision", &metadata.source_revision),
        ("build identity", &metadata.build_identity),
        ("fixture identity", &metadata.fixture_identity),
        ("verifier identity", &metadata.verifier_identity),
    ] {
        ensure!(!value.is_empty(), "{name} is missing");
    }
    ensure!(
        metadata.assembly_manifest_digest != Sha256Digest::ZERO,
        "assembly manifest digest is missing"
    );
    ensure!(
        metadata.owner_identity_digest != Sha256Digest::ZERO,
        "owner identity digest is missing"
    );
    let frozen_ordered: Vec<String> = REQUIRED_FEEDER_IDS
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    ensure!(
        metadata.required_feeder_ids == frozen_ordered,
        "required feeder list differs from frozen canonical contract"
    );
    let frozen: BTreeSet<&str> = REQUIRED_FEEDER_IDS.iter().copied().collect();
    let available: BTreeSet<&str> = metadata
        .available_feeder_ids
        .iter()
        .map(String::as_str)
        .collect();
    ensure!(
        frozen.is_subset(&available),
        "available feeder set is missing a required feeder"
    );
    Ok(())
}

fn validate_surface(surface: &CanonicalSurfaceBytes) -> Result<()> {
    ensure!(
        !surface.source_is_fallback,
        "fallback/unavailable evidence cannot be promoted"
    );
    for (name, bytes) in [
        ("request", &surface.request_bytes),
        ("result", &surface.result_bytes),
        ("error", &surface.error_bytes),
        ("ref", &surface.ref_bytes),
    ] {
        ensure!(!bytes.is_empty(), "canonical {name} bytes are missing");
        let value: Value = serde_json::from_slice(bytes)
            .with_context(|| format!("decode canonical {name} bytes"))?;
        ensure!(
            canonical_json(&value).as_bytes() == bytes,
            "canonical {name} bytes are not canonical JSON"
        );
    }
    let refs: Vec<String> =
        serde_json::from_slice(&surface.ref_bytes).context("decode canonical refs")?;
    ensure!(!refs.is_empty(), "canonical ref set is empty");
    for reference in refs {
        ensure!(
            reference.starts_with("gz://blob/"),
            "evidence ref is not a full gz://blob ref: {reference}"
        );
        let GzRef::Blob { hash, fragment } = GzRef::parse(&reference)
            .map_err(|error| anyhow::anyhow!("invalid evidence ref {reference}: {error}"))?
        else {
            bail!("evidence ref is not a blob: {reference}")
        };
        ensure!(
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "evidence ref is not full lowercase sha256: {reference}"
        );
        ensure!(
            matches!(fragment, Fragment::None) && !reference.contains('#'),
            "fragment evidence ref is not allowed: {reference}"
        );
        ensure!(
            reference == format!("gz://blob/{hash}"),
            "evidence ref is not canonical: {reference}"
        );
    }
    Ok(())
}

fn receipt_matches_expectation(
    receipt: &DurabilityReceipt,
    expected: &DurabilityReceiptExpectation,
) -> bool {
    receipt.metadata.source_revision == expected.source_revision
        && receipt.metadata.build_identity == expected.build_identity
        && receipt.metadata.fixture_identity == expected.fixture_identity
        && receipt.metadata.verifier_identity == expected.verifier_identity
        && receipt.metadata.assembly_manifest_digest == expected.assembly_manifest_digest
        && receipt.metadata.owner_identity_digest == expected.owner_identity_digest
        && receipt.metadata.durable_profile_id == expected.durable_profile_id
        && receipt.metadata.required_feeder_ids == expected.required_feeder_ids
        && receipt.metadata.available_feeder_ids == expected.available_feeder_ids
        && receipt.surface == expected.surface
}

fn root_paths(store_root: &Path) -> JournalPaths {
    JournalPaths::new(
        store_root.join(JOURNAL_DIR).join("root.json"),
        store_root.join(JOURNAL_DIR).join("root-journal.json"),
        store_root.join(JOURNAL_DIR).join("root-cartridge.json"),
        store_root.join(JOURNAL_DIR).join("root-owner-death.json"),
        store_root.join(JOURNAL_DIR).join("root-recovery.json"),
    )
    .expect("static root journal paths are distinct")
}

fn transaction_dir(store_root: &Path, transaction_id: Sha256Digest) -> PathBuf {
    store_root.join(JOURNAL_DIR).join(transaction_id.to_hex())
}

fn transaction_paths(store_root: &Path, transaction_id: Sha256Digest) -> Result<JournalPaths> {
    let dir = transaction_dir(store_root, transaction_id);
    Ok(JournalPaths::new(
        root_paths(store_root).root_record(),
        dir.join("journal.json"),
        dir.join("cartridge.json"),
        dir.join("owner-death.json"),
        dir.join("recovery.json"),
    )?)
}

fn prepared_context_path(store_root: &Path, transaction_id: Sha256Digest) -> PathBuf {
    transaction_dir(store_root, transaction_id).join("prepared-context.json")
}

fn prepared_context_bytes(context: &PreparedContext) -> Result<Vec<u8>> {
    Ok(canonical_json(&serde_json::to_value(context)?).into_bytes())
}

fn persist_prepared_context(store_root: &Path, context: &PreparedContext) -> Result<()> {
    let path = prepared_context_path(store_root, context.transaction_id);
    let bytes = prepared_context_bytes(context)?;
    match fs::read(&path) {
        Ok(existing) => ensure!(
            existing == bytes,
            "transaction id already has a different prepared context"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            super::atomic_write_file(&path, &bytes)
                .with_context(|| format!("persist prepared context {}", path.display()))?;
        }
        Err(error) => {
            return Err(error).with_context(|| format!("read prepared context {}", path.display()));
        }
    }
    Ok(())
}

fn read_prepared_context(
    store_root: &Path,
    transaction_id: Sha256Digest,
) -> Result<PreparedContext> {
    let path = prepared_context_path(store_root, transaction_id);
    let bytes =
        fs::read(&path).with_context(|| format!("read prepared context {}", path.display()))?;
    let context: PreparedContext =
        serde_json::from_slice(&bytes).context("decode prepared context")?;
    ensure!(
        prepared_context_bytes(&context)? == bytes,
        "prepared context is not canonical"
    );
    ensure!(
        context.transaction_id == transaction_id,
        "prepared context transaction id differs from path"
    );
    Ok(context)
}

fn initialize_or_check_root(store_root: &Path, old_root: Sha256Digest) -> Result<()> {
    let paths = root_paths(store_root);
    match read_published_root(&paths) {
        Ok(root) => ensure!(
            root.root_digest == old_root,
            "durability root is stale relative to current manifest"
        ),
        Err(error) if error.code == JournalFailureCode::RootMissing => {
            initialize_published_root(&paths, old_root).map_err(anyhow::Error::new)?;
        }
        Err(error) => {
            return Err(anyhow::Error::new(error).context("read published durability root"));
        }
    }
    Ok(())
}

fn manifest_digest(manifest: &Manifest) -> Sha256Digest {
    let bytes = manifest.encode();
    let mut bound = Vec::with_capacity(MANIFEST_DOMAIN.len() + bytes.len());
    bound.extend_from_slice(MANIFEST_DOMAIN);
    bound.extend_from_slice(&bytes);
    Sha256Digest::from_bytes(sha256(&bound))
}

fn manifest_identity(manifest: &Manifest, ref_bytes: &[u8]) -> ManifestIdentity {
    let latest = manifest.latest();
    let (latest_snapshot_id, latest_global_hash, latest_shard_hashes, latest_segment_ids) = latest
        .map_or((None, None, Vec::new(), Vec::new()), |entry| {
            (
                Some(entry.snapshot_id),
                Some(entry.global_hash),
                entry.shard_hashes.clone(),
                entry.segment_ids.clone(),
            )
        });
    ManifestIdentity {
        manifest_digest: manifest_digest(manifest),
        latest_snapshot_id,
        latest_global_hash,
        latest_shard_hashes,
        latest_segment_ids,
        ref_digest: Sha256Digest::from_bytes(sha256(ref_bytes)),
    }
}

fn read_manifest_pair(store_root: &Path) -> Result<(Manifest, Option<Manifest>)> {
    let current = read_manifest_file(&manifest_path(store_root))?.unwrap_or_default();
    let previous = read_manifest_file(&manifest_prev_path(store_root))?;
    Ok((current, previous))
}

fn read_manifest_file(path: &Path) -> Result<Option<Manifest>> {
    match fs::read(path) {
        Ok(bytes) => {
            Ok(Some(Manifest::decode(&bytes).with_context(|| {
                format!("decode manifest {}", path.display())
            })?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read manifest {}", path.display())),
    }
}

fn reopen_manifest(store_root: &Path) -> Result<Manifest> {
    read_manifest_pair(store_root)
        .map(|(current, _)| current)
        .context("fresh-process-style manifest reopen")
}

fn resume_manifests(
    store_root: &Path,
    old_root: Sha256Digest,
    new_root: Sha256Digest,
    candidate: &Manifest,
) -> Result<(Manifest, Manifest, Manifest)> {
    let (current, previous) = read_manifest_pair(store_root)?;
    let current_digest = manifest_digest(&current);
    ensure!(
        current_digest == old_root || current_digest == new_root,
        "current manifest is partial or unknown"
    );
    if let Some(previous) = &previous {
        let digest = manifest_digest(previous);
        ensure!(
            digest == old_root || digest == new_root,
            "manifest.prev is partial or unknown"
        );
    }
    let old = [Some(current.clone()), previous.clone()]
        .into_iter()
        .flatten()
        .find(|manifest| manifest_digest(manifest) == old_root)
        .or_else(|| {
            (manifest_digest(&Manifest::default()) == old_root).then_some(Manifest::default())
        })
        .ok_or_else(|| anyhow::anyhow!("old manifest root is unavailable"))?;
    let new = [Some(current.clone()), previous.clone()]
        .into_iter()
        .flatten()
        .find(|manifest| manifest_digest(manifest) == new_root)
        .or_else(|| (manifest_digest(candidate) == new_root).then(|| candidate.clone()))
        .ok_or_else(|| anyhow::anyhow!("new manifest root is unavailable"))?;
    Ok((old, new, current))
}

fn verify_manifest_identity(
    store_root: &Path,
    manifest: &Manifest,
    expected: &ManifestIdentity,
    ref_bytes: &[u8],
) -> Result<()> {
    ensure!(
        manifest_identity(manifest, ref_bytes) == *expected,
        "manifest identity does not match receipt"
    );
    verify_manifest_artifacts(store_root, manifest, ref_bytes)
}

fn verify_manifest_artifacts(
    store_root: &Path,
    manifest: &Manifest,
    ref_bytes: &[u8],
) -> Result<()> {
    let identity = manifest_identity(manifest, ref_bytes);
    ensure!(
        identity.ref_digest == Sha256Digest::from_bytes(sha256(ref_bytes)),
        "ref identity differs from manifest"
    );
    let Some(entry) = manifest.latest() else {
        return Ok(());
    };
    let shards = store_root.join("shards");
    verify_file_hash(
        &shards.join(super::indexer::global_file_name(entry.snapshot_id)),
        entry.global_hash,
    )?;
    for (index, expected) in entry.shard_hashes.iter().enumerate() {
        verify_file_hash(
            &shards.join(super::indexer::shard_file_name(entry.snapshot_id, index)),
            *expected,
        )?;
    }
    for segment_id in &entry.segment_ids {
        super::delta_log::read_segment(
            &store_root
                .join("wal")
                .join(format!("seg_{segment_id:08}.log")),
        )
        .with_context(|| format!("verify referenced WAL segment {segment_id}"))?;
    }
    let blob_store = BlobStore::open(store_root)?;
    let refs: Vec<String> =
        serde_json::from_slice(ref_bytes).context("decode refs for artifact verification")?;
    for reference in refs {
        let GzRef::Blob { hash, fragment } = GzRef::parse(&reference)? else {
            bail!("unverified nonblob evidence ref: {reference}")
        };
        ensure!(
            hash.len() == 64 && matches!(fragment, Fragment::None),
            "unverified partial blob ref: {reference}"
        );
        let content_hash = ContentHash::from_hex(&hash)
            .ok_or_else(|| anyhow::anyhow!("invalid blob hash in ref"))?;
        ensure!(
            blob_store.get(&content_hash)?.is_some(),
            "referenced blob is missing: {hash}"
        );
    }
    Ok(())
}

fn verify_file_hash(path: &Path, expected: u64) -> Result<()> {
    let bytes =
        fs::read(path).with_context(|| format!("read published artifact {}", path.display()))?;
    ensure!(
        file_hash64(&bytes) == expected,
        "published artifact hash mismatch: {}",
        path.display()
    );
    Ok(())
}

fn read_owner_death(paths: &JournalPaths) -> Result<Option<OwnerDeathReceipt>> {
    match fs::read(paths.owner_death_receipt()) {
        Ok(bytes) => {
            let owner: OwnerDeathReceipt =
                serde_json::from_slice(&bytes).context("decode owner-death receipt")?;
            ensure!(
                owner.canonical_bytes().is_ok(),
                "owner-death receipt is not canonical"
            );
            Ok(Some(owner))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("read owner-death receipt"),
    }
}

#[cfg(test)]
#[path = "../../../../../tests/graphzero/unit/graphzero-store/durability_receipt_tests.rs"]
mod tests;
