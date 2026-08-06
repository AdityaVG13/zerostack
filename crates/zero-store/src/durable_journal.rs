//! Durable single-root journal and recovery protocol.
//!
//! The caller supplies every path. This module does not introduce a store
//! layout. A transaction first persists a continuation cartridge, then a
//! prepared journal record. Commit publishes a synced root record and only
//! then persists a committed journal record. Recovery accepts only the old
//! root or the new root; every other journal/root pairing fails loudly.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use zero_abi::{DigestV1, canonical_json, sha256};

use crate::fs_replace::{replace_file, sync_dir};
use crate::{DurableProfileIdV1, DurableProfileV1};

pub const DURABLE_JOURNAL_SCHEMA_VERSION_V2: u16 = 2;
pub const DURABLE_BINDING_SCHEMA_VERSION_V1: u16 = 1;
pub const DURABLE_RECEIPT_SCHEMA_VERSION_V1: u16 = 1;
pub const DURABLE_JOURNAL_MAX_RECORD_BYTES_V1: u64 = 64 * 1024;

const BINDING_DOMAIN_V1: &[u8] = b"zerostack.durable_journal.binding.v1\0";
const JOURNAL_DOMAIN_V2: &[u8] = b"zerostack.durable_journal.record.v2\0";
const ROOT_DOMAIN_V1: &[u8] = b"zerostack.durable_journal.root.v1\0";
const CARTRIDGE_DOMAIN_V1: &[u8] = b"zerostack.durable_journal.cartridge.v1\0";
const OWNER_DEATH_DOMAIN_V1: &[u8] = b"zerostack.durable_journal.owner_death.v1\0";
const RECOVERY_DOMAIN_V1: &[u8] = b"zerostack.durable_journal.recovery.v1\0";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalStateV1 {
    Prepared,
    Committed,
    Aborted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AbortReasonV1 {
    ExplicitAbort,
    RecoveryObservedOldRoot,
    OwnerDeathObservedOldRoot,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryOutcomeV1 {
    NotStartedOldRoot,
    OldRootAborted,
    NewRootCommitted,
    AlreadyCommitted,
    AlreadyAborted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalFailureCodeV1 {
    SchemaVersionMismatch,
    InvalidBinding,
    ProfileSubstitution,
    JournalMissing,
    RootMissing,
    TornOrNoncanonicalRecord,
    RecordTooLarge,
    CartridgeMismatch,
    OwnerIdentityMismatch,
    SequenceMismatch,
    RootMismatch,
    JournalRootDisagreement,
    AlreadyTerminal,
    ImmutableReceiptConflict,
    IoBeforePublish,
    DirectorySyncFailedAfterPublish,
    InjectedCrash,
    OwnerDeath,
    Indeterminate,
}

impl JournalFailureCodeV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SchemaVersionMismatch => "schema_version_mismatch",
            Self::InvalidBinding => "invalid_binding",
            Self::ProfileSubstitution => "profile_substitution",
            Self::JournalMissing => "journal_missing",
            Self::RootMissing => "root_missing",
            Self::TornOrNoncanonicalRecord => "torn_or_noncanonical_record",
            Self::RecordTooLarge => "record_too_large",
            Self::CartridgeMismatch => "cartridge_mismatch",
            Self::OwnerIdentityMismatch => "owner_identity_mismatch",
            Self::SequenceMismatch => "sequence_mismatch",
            Self::RootMismatch => "root_mismatch",
            Self::JournalRootDisagreement => "journal_root_disagreement",
            Self::AlreadyTerminal => "already_terminal",
            Self::ImmutableReceiptConflict => "immutable_receipt_conflict",
            Self::IoBeforePublish => "io_before_publish",
            Self::DirectorySyncFailedAfterPublish => "directory_sync_failed_after_publish",
            Self::InjectedCrash => "injected_crash",
            Self::OwnerDeath => "owner_death",
            Self::Indeterminate => "indeterminate",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalBoundaryV1 {
    RootInitializeBeforeWrite,
    RootInitializeAfterFileSync,
    RootInitializeAfterRename,
    RootInitializeAfterDirectorySync,
    CartridgeBeforeWrite,
    CartridgeAfterFileSync,
    CartridgeAfterRename,
    CartridgeAfterDirectorySync,
    PrepareBeforeWrite,
    PrepareAfterFileSync,
    PrepareAfterRename,
    PrepareAfterDirectorySync,
    RootPublishBeforeWrite,
    RootPublishAfterFileSync,
    RootPublishAfterRename,
    RootPublishAfterDirectorySync,
    CommitBeforeWrite,
    CommitAfterFileSync,
    CommitAfterRename,
    CommitAfterDirectorySync,
    AbortBeforeWrite,
    AbortAfterFileSync,
    AbortAfterRename,
    AbortAfterDirectorySync,
    OwnerDeathBeforeWrite,
    OwnerDeathAfterFileSync,
    OwnerDeathAfterRename,
    OwnerDeathAfterDirectorySync,
    RecoveryBeforeWrite,
    RecoveryAfterFileSync,
    RecoveryAfterRename,
    RecoveryAfterDirectorySync,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalErrorV1 {
    pub code: JournalFailureCodeV1,
    pub boundary: Option<JournalBoundaryV1>,
    pub publication_may_have_changed: bool,
    pub detail: String,
}

impl JournalErrorV1 {
    fn new(code: JournalFailureCodeV1, detail: impl Into<String>) -> Self {
        Self {
            code,
            boundary: None,
            publication_may_have_changed: false,
            detail: detail.into(),
        }
    }
    fn at(
        code: JournalFailureCodeV1,
        boundary: JournalBoundaryV1,
        publication_may_have_changed: bool,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            code,
            boundary: Some(boundary),
            publication_may_have_changed,
            detail: detail.into(),
        }
    }
}
impl std::fmt::Display for JournalErrorV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}
impl std::error::Error for JournalErrorV1 {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalPathsV1 {
    root_record: PathBuf,
    journal_record: PathBuf,
    cartridge: PathBuf,
    owner_death_receipt: PathBuf,
    recovery_receipt: PathBuf,
}
impl JournalPathsV1 {
    pub fn new(
        root_record: impl Into<PathBuf>,
        journal_record: impl Into<PathBuf>,
        cartridge: impl Into<PathBuf>,
        owner_death_receipt: impl Into<PathBuf>,
        recovery_receipt: impl Into<PathBuf>,
    ) -> Result<Self, JournalErrorV1> {
        let paths = Self {
            root_record: root_record.into(),
            journal_record: journal_record.into(),
            cartridge: cartridge.into(),
            owner_death_receipt: owner_death_receipt.into(),
            recovery_receipt: recovery_receipt.into(),
        };
        let all = [
            &paths.root_record,
            &paths.journal_record,
            &paths.cartridge,
            &paths.owner_death_receipt,
            &paths.recovery_receipt,
        ];
        if all.iter().any(|path| path.file_name().is_none()) {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::InvalidBinding,
                "every journal path must name a file",
            ));
        }
        for (index, left) in all.iter().enumerate() {
            if all.iter().skip(index + 1).any(|right| left == right) {
                return Err(JournalErrorV1::new(
                    JournalFailureCodeV1::InvalidBinding,
                    "journal paths must be distinct",
                ));
            }
        }
        Ok(paths)
    }
    pub fn root_record(&self) -> &Path {
        &self.root_record
    }
    pub fn journal_record(&self) -> &Path {
        &self.journal_record
    }
    pub fn cartridge(&self) -> &Path {
        &self.cartridge
    }
    pub fn owner_death_receipt(&self) -> &Path {
        &self.owner_death_receipt
    }
    pub fn recovery_receipt(&self) -> &Path {
        &self.recovery_receipt
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JournalBindingV1 {
    pub schema_version: u16,
    pub transaction_id: DigestV1,
    pub assembly_manifest_digest: DigestV1,
    pub durable_profile_id: DurableProfileIdV1,
    pub durable_profile_digest: DigestV1,
    pub old_root: DigestV1,
    pub new_root: DigestV1,
    pub owner_identity_digest: DigestV1,
}
impl JournalBindingV1 {
    pub fn new(
        transaction_id: DigestV1,
        assembly_manifest_digest: DigestV1,
        durable_profile_id: DurableProfileIdV1,
        old_root: DigestV1,
        new_root: DigestV1,
        owner_identity_digest: DigestV1,
    ) -> Self {
        Self {
            schema_version: DURABLE_BINDING_SCHEMA_VERSION_V1,
            transaction_id,
            assembly_manifest_digest,
            durable_profile_id,
            durable_profile_digest: DurableProfileV1::new(durable_profile_id).digest(),
            old_root,
            new_root,
            owner_identity_digest,
        }
    }
    pub fn validate(&self) -> Result<(), JournalErrorV1> {
        if self.schema_version != DURABLE_BINDING_SCHEMA_VERSION_V1 {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::SchemaVersionMismatch,
                "journal binding schema version is not supported",
            ));
        }
        if [
            self.transaction_id,
            self.assembly_manifest_digest,
            self.old_root,
            self.new_root,
            self.owner_identity_digest,
        ]
        .contains(&DigestV1::ZERO)
        {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::InvalidBinding,
                "binding digests must be nonzero",
            ));
        }
        if self.old_root == self.new_root {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::InvalidBinding,
                "old and new roots must differ",
            ));
        }
        if self.durable_profile_digest != DurableProfileV1::new(self.durable_profile_id).digest() {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::ProfileSubstitution,
                "durable profile identity does not match its frozen digest",
            ));
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, JournalErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<DigestV1, JournalErrorV1> {
        Ok(domain_digest(BINDING_DOMAIN_V1, &self.canonical_bytes()?))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DurableJournalV2 {
    pub schema_version: u16,
    pub binding: JournalBindingV1,
    pub state: JournalStateV1,
    pub sequence: u64,
    pub predecessor_record_digest: Option<DigestV1>,
    pub abort_reason: Option<AbortReasonV1>,
}
impl DurableJournalV2 {
    fn prepared(binding: JournalBindingV1) -> Self {
        Self {
            schema_version: DURABLE_JOURNAL_SCHEMA_VERSION_V2,
            binding,
            state: JournalStateV1::Prepared,
            sequence: 1,
            predecessor_record_digest: None,
            abort_reason: None,
        }
    }
    fn committed(prepared: &Self, digest: DigestV1) -> Self {
        Self {
            schema_version: DURABLE_JOURNAL_SCHEMA_VERSION_V2,
            binding: prepared.binding.clone(),
            state: JournalStateV1::Committed,
            sequence: 2,
            predecessor_record_digest: Some(digest),
            abort_reason: None,
        }
    }
    fn aborted(prepared: &Self, digest: DigestV1, reason: AbortReasonV1) -> Self {
        Self {
            schema_version: DURABLE_JOURNAL_SCHEMA_VERSION_V2,
            binding: prepared.binding.clone(),
            state: JournalStateV1::Aborted,
            sequence: 2,
            predecessor_record_digest: Some(digest),
            abort_reason: Some(reason),
        }
    }
    pub fn validate(&self) -> Result<(), JournalErrorV1> {
        if self.schema_version != DURABLE_JOURNAL_SCHEMA_VERSION_V2 {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::SchemaVersionMismatch,
                "journal record schema version is not supported",
            ));
        }
        self.binding.validate()?;
        let valid = match self.state {
            JournalStateV1::Prepared => {
                self.sequence == 1
                    && self.predecessor_record_digest.is_none()
                    && self.abort_reason.is_none()
            }
            JournalStateV1::Committed => {
                self.sequence == 2
                    && self.predecessor_record_digest.is_some()
                    && self.abort_reason.is_none()
            }
            JournalStateV1::Aborted => {
                self.sequence == 2
                    && self.predecessor_record_digest.is_some()
                    && self.abort_reason.is_some()
            }
        };
        if !valid {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::SequenceMismatch,
                "journal state and sequence commitments disagree",
            ));
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, JournalErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<DigestV1, JournalErrorV1> {
        Ok(domain_digest(JOURNAL_DOMAIN_V2, &self.canonical_bytes()?))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootPublicationReceipt {
    pub schema_version: u16,
    pub transaction_id: DigestV1,
    pub root_digest: DigestV1,
    pub generation: u64,
    pub prepared_record_digest: DigestV1,
}
impl RootPublicationReceipt {
    fn initial(root_digest: DigestV1) -> Result<Self, JournalErrorV1> {
        if root_digest == DigestV1::ZERO {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::InvalidBinding,
                "initial root digest must be nonzero",
            ));
        }
        Ok(Self {
            schema_version: DURABLE_RECEIPT_SCHEMA_VERSION_V1,
            transaction_id: DigestV1::ZERO,
            root_digest,
            generation: 0,
            prepared_record_digest: DigestV1::ZERO,
        })
    }
    fn published(binding: &JournalBindingV1, prepared_record_digest: DigestV1) -> Self {
        Self {
            schema_version: DURABLE_RECEIPT_SCHEMA_VERSION_V1,
            transaction_id: binding.transaction_id,
            root_digest: binding.new_root,
            generation: 1,
            prepared_record_digest,
        }
    }
    pub fn validate(&self) -> Result<(), JournalErrorV1> {
        if self.schema_version != DURABLE_RECEIPT_SCHEMA_VERSION_V1 {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::SchemaVersionMismatch,
                "published root schema version is not supported",
            ));
        }
        let initial = self.generation == 0
            && self.transaction_id == DigestV1::ZERO
            && self.prepared_record_digest == DigestV1::ZERO;
        let transactional = self.generation == 1
            && self.transaction_id != DigestV1::ZERO
            && self.prepared_record_digest != DigestV1::ZERO;
        if self.root_digest == DigestV1::ZERO || !(initial || transactional) {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::RootMismatch,
                "root generation and transaction commitments disagree",
            ));
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, JournalErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<DigestV1, JournalErrorV1> {
        Ok(domain_digest(ROOT_DOMAIN_V1, &self.canonical_bytes()?))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationCartridgeV1 {
    pub schema_version: u16,
    pub binding_digest: DigestV1,
    pub prepared_record_digest: DigestV1,
    pub transaction_id: DigestV1,
    pub old_root: DigestV1,
    pub new_root: DigestV1,
    pub durable_profile_digest: DigestV1,
    pub owner_identity_digest: DigestV1,
}
/// Compatibility alias for callers written against the carrier draft.
pub type JournalRecordV1 = DurableJournalV2;
/// Compatibility alias for callers written against the carrier draft.
pub type PublishedRootV1 = RootPublicationReceipt;

impl ContinuationCartridgeV1 {
    fn new(
        binding: &JournalBindingV1,
        prepared_record_digest: DigestV1,
    ) -> Result<Self, JournalErrorV1> {
        Ok(Self {
            schema_version: DURABLE_RECEIPT_SCHEMA_VERSION_V1,
            binding_digest: binding.digest()?,
            prepared_record_digest,
            transaction_id: binding.transaction_id,
            old_root: binding.old_root,
            new_root: binding.new_root,
            durable_profile_digest: binding.durable_profile_digest,
            owner_identity_digest: binding.owner_identity_digest,
        })
    }
    pub fn validate_against(&self, binding: &JournalBindingV1) -> Result<(), JournalErrorV1> {
        if self.schema_version != DURABLE_RECEIPT_SCHEMA_VERSION_V1
            || self.binding_digest != binding.digest()?
            || self.transaction_id != binding.transaction_id
            || self.old_root != binding.old_root
            || self.new_root != binding.new_root
            || self.durable_profile_digest != binding.durable_profile_digest
            || self.owner_identity_digest != binding.owner_identity_digest
            || self.prepared_record_digest == DigestV1::ZERO
        {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::CartridgeMismatch,
                "continuation cartridge does not bind the journal transaction",
            ));
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, JournalErrorV1> {
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<DigestV1, JournalErrorV1> {
        Ok(domain_digest(CARTRIDGE_DOMAIN_V1, &self.canonical_bytes()?))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerDeathReceiptV1 {
    pub schema_version: u16,
    pub binding_digest: DigestV1,
    pub prepared_record_digest: DigestV1,
    pub owner_identity_digest: DigestV1,
    pub observed_journal_state: JournalStateV1,
    pub observed_root: DigestV1,
    pub observed_at_unix_ns: u64,
    pub failure_code: JournalFailureCodeV1,
    pub recovery_required: bool,
}
impl OwnerDeathReceiptV1 {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, JournalErrorV1> {
        if self.schema_version != DURABLE_RECEIPT_SCHEMA_VERSION_V1
            || self.failure_code != JournalFailureCodeV1::OwnerDeath
            || !self.recovery_required
            || self.binding_digest == DigestV1::ZERO
            || self.prepared_record_digest == DigestV1::ZERO
            || self.owner_identity_digest == DigestV1::ZERO
            || self.observed_root == DigestV1::ZERO
        {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::InvalidBinding,
                "owner-death receipt is incomplete",
            ));
        }
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<DigestV1, JournalErrorV1> {
        Ok(domain_digest(
            OWNER_DEATH_DOMAIN_V1,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryReceiptV1 {
    pub schema_version: u16,
    pub binding_digest: DigestV1,
    pub prepared_record_digest: DigestV1,
    pub terminal_record_digest: Option<DigestV1>,
    pub owner_death_receipt_digest: Option<DigestV1>,
    pub observed_root: DigestV1,
    pub outcome: RecoveryOutcomeV1,
    pub journal_root_correspondence: bool,
    pub promotable: bool,
    pub failure_code: Option<JournalFailureCodeV1>,
}
impl RecoveryReceiptV1 {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, JournalErrorV1> {
        if self.schema_version != DURABLE_RECEIPT_SCHEMA_VERSION_V1
            || self.binding_digest == DigestV1::ZERO
            || self.observed_root == DigestV1::ZERO
            || !self.journal_root_correspondence
            || !self.promotable
            || self.failure_code.is_some()
        {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::InvalidBinding,
                "successful recovery receipt is incomplete or non-promotable",
            ));
        }
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<DigestV1, JournalErrorV1> {
        Ok(domain_digest(RECOVERY_DOMAIN_V1, &self.canonical_bytes()?))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FaultPlanV1 {
    crash_at: Option<JournalBoundaryV1>,
    fired: bool,
}
impl FaultPlanV1 {
    pub const fn none() -> Self {
        Self {
            crash_at: None,
            fired: false,
        }
    }
    pub const fn crash_at(boundary: JournalBoundaryV1) -> Self {
        Self {
            crash_at: Some(boundary),
            fired: false,
        }
    }
    fn hit(&mut self, boundary: JournalBoundaryV1, changed: bool) -> Result<(), JournalErrorV1> {
        if !self.fired && self.crash_at == Some(boundary) {
            self.fired = true;
            return Err(JournalErrorV1::at(
                JournalFailureCodeV1::InjectedCrash,
                boundary,
                changed,
                "preregistered crash boundary reached",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct WriteBoundaries {
    before: JournalBoundaryV1,
    file_sync: JournalBoundaryV1,
    rename: JournalBoundaryV1,
    dir_sync: JournalBoundaryV1,
}
const ROOT_INIT: WriteBoundaries = WriteBoundaries {
    before: JournalBoundaryV1::RootInitializeBeforeWrite,
    file_sync: JournalBoundaryV1::RootInitializeAfterFileSync,
    rename: JournalBoundaryV1::RootInitializeAfterRename,
    dir_sync: JournalBoundaryV1::RootInitializeAfterDirectorySync,
};
const CARTRIDGE: WriteBoundaries = WriteBoundaries {
    before: JournalBoundaryV1::CartridgeBeforeWrite,
    file_sync: JournalBoundaryV1::CartridgeAfterFileSync,
    rename: JournalBoundaryV1::CartridgeAfterRename,
    dir_sync: JournalBoundaryV1::CartridgeAfterDirectorySync,
};
const PREPARE: WriteBoundaries = WriteBoundaries {
    before: JournalBoundaryV1::PrepareBeforeWrite,
    file_sync: JournalBoundaryV1::PrepareAfterFileSync,
    rename: JournalBoundaryV1::PrepareAfterRename,
    dir_sync: JournalBoundaryV1::PrepareAfterDirectorySync,
};
const ROOT_PUBLISH: WriteBoundaries = WriteBoundaries {
    before: JournalBoundaryV1::RootPublishBeforeWrite,
    file_sync: JournalBoundaryV1::RootPublishAfterFileSync,
    rename: JournalBoundaryV1::RootPublishAfterRename,
    dir_sync: JournalBoundaryV1::RootPublishAfterDirectorySync,
};
const COMMIT: WriteBoundaries = WriteBoundaries {
    before: JournalBoundaryV1::CommitBeforeWrite,
    file_sync: JournalBoundaryV1::CommitAfterFileSync,
    rename: JournalBoundaryV1::CommitAfterRename,
    dir_sync: JournalBoundaryV1::CommitAfterDirectorySync,
};
const ABORT: WriteBoundaries = WriteBoundaries {
    before: JournalBoundaryV1::AbortBeforeWrite,
    file_sync: JournalBoundaryV1::AbortAfterFileSync,
    rename: JournalBoundaryV1::AbortAfterRename,
    dir_sync: JournalBoundaryV1::AbortAfterDirectorySync,
};
const OWNER_DEATH: WriteBoundaries = WriteBoundaries {
    before: JournalBoundaryV1::OwnerDeathBeforeWrite,
    file_sync: JournalBoundaryV1::OwnerDeathAfterFileSync,
    rename: JournalBoundaryV1::OwnerDeathAfterRename,
    dir_sync: JournalBoundaryV1::OwnerDeathAfterDirectorySync,
};
const RECOVERY: WriteBoundaries = WriteBoundaries {
    before: JournalBoundaryV1::RecoveryBeforeWrite,
    file_sync: JournalBoundaryV1::RecoveryAfterFileSync,
    rename: JournalBoundaryV1::RecoveryAfterRename,
    dir_sync: JournalBoundaryV1::RecoveryAfterDirectorySync,
};

pub fn initialize_published_root_v1(
    paths: &JournalPathsV1,
    root: DigestV1,
) -> Result<RootPublicationReceipt, JournalErrorV1> {
    initialize_published_root_with_fault_v1(paths, root, &mut FaultPlanV1::none())
}
pub fn initialize_published_root_with_fault_v1(
    paths: &JournalPathsV1,
    root: DigestV1,
    fault: &mut FaultPlanV1,
) -> Result<RootPublicationReceipt, JournalErrorV1> {
    let expected = RootPublicationReceipt::initial(root)?;
    if let Some(existing) = read_optional::<RootPublicationReceipt>(
        paths.root_record(),
        JournalFailureCodeV1::RootMissing,
    )? {
        existing.value.validate()?;
        if existing.value == expected {
            return Ok(existing.value);
        }
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::RootMismatch,
            "initialization cannot replace an existing different root",
        ));
    }
    durable_replace(
        paths.root_record(),
        &expected.canonical_bytes()?,
        ROOT_INIT,
        fault,
    )?;
    Ok(expected)
}

pub fn prepare_journal_v1(
    paths: &JournalPathsV1,
    binding: JournalBindingV1,
) -> Result<ContinuationCartridgeV1, JournalErrorV1> {
    prepare_journal_with_fault_v1(paths, binding, &mut FaultPlanV1::none())
}
pub fn prepare_journal_with_fault_v1(
    paths: &JournalPathsV1,
    binding: JournalBindingV1,
    fault: &mut FaultPlanV1,
) -> Result<ContinuationCartridgeV1, JournalErrorV1> {
    binding.validate()?;
    if read_optional::<RecoveryReceiptV1>(
        paths.recovery_receipt(),
        JournalFailureCodeV1::JournalMissing,
    )?
    .is_some()
    {
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::AlreadyTerminal,
            "recovered transaction cannot be prepared again",
        ));
    }
    let root = read_root(paths)?;
    if root.root_digest != binding.old_root {
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::RootMismatch,
            "prepare requires the preregistered old root",
        ));
    }
    if let Some(existing) = read_optional::<DurableJournalV2>(
        paths.journal_record(),
        JournalFailureCodeV1::JournalMissing,
    )? {
        existing.value.validate()?;
        if existing.value.state == JournalStateV1::Prepared && existing.value.binding == binding {
            let cartridge = read_cartridge(paths)?;
            cartridge.validate_against(&binding)?;
            if cartridge.prepared_record_digest != existing.digest {
                return Err(JournalErrorV1::new(
                    JournalFailureCodeV1::CartridgeMismatch,
                    "cartridge does not bind the persisted prepared record",
                ));
            }
            return Ok(cartridge);
        }
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::AlreadyTerminal,
            "another or terminal transaction already occupies this journal",
        ));
    }
    let prepared = DurableJournalV2::prepared(binding.clone());
    let prepared_digest = prepared.digest()?;
    let cartridge = ContinuationCartridgeV1::new(&binding, prepared_digest)?;
    write_once(
        paths.cartridge(),
        &cartridge.canonical_bytes()?,
        CARTRIDGE,
        fault,
    )?;
    durable_replace(
        paths.journal_record(),
        &prepared.canonical_bytes()?,
        PREPARE,
        fault,
    )?;
    Ok(cartridge)
}

pub fn commit_journal_v1(
    paths: &JournalPathsV1,
    cartridge: &ContinuationCartridgeV1,
) -> Result<RecoveryReceiptV1, JournalErrorV1> {
    commit_journal_with_fault_v1(paths, cartridge, &mut FaultPlanV1::none())
}
pub fn commit_journal_with_fault_v1(
    paths: &JournalPathsV1,
    cartridge: &ContinuationCartridgeV1,
    fault: &mut FaultPlanV1,
) -> Result<RecoveryReceiptV1, JournalErrorV1> {
    if let Some(existing) = existing_recovery(paths, None)? {
        if existing.binding_digest == cartridge.binding_digest {
            return Ok(existing);
        }
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::ImmutableReceiptConflict,
            "recovery receipt and continuation cartridge bindings differ",
        ));
    }
    let journal = read_journal(paths)?;
    journal.value.validate()?;
    cartridge.validate_against(&journal.value.binding)?;
    let binding = &journal.value.binding;
    match journal.value.state {
        JournalStateV1::Aborted => {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::AlreadyTerminal,
                "aborted journal cannot commit",
            ));
        }
        JournalStateV1::Committed => {
            let root = read_root(paths)?;
            let prepared = prepared_digest(&journal.value, journal.digest)?;
            verify_new(&root, binding, prepared)?;
            return persist_recovery(
                paths,
                make_recovery(
                    binding,
                    prepared,
                    Some(journal.digest),
                    root.root_digest,
                    RecoveryOutcomeV1::AlreadyCommitted,
                    owner_death_digest(paths, binding)?,
                )?,
                fault,
            );
        }
        JournalStateV1::Prepared => {}
    }
    if journal.digest != cartridge.prepared_record_digest {
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::CartridgeMismatch,
            "prepared record digest differs from continuation cartridge",
        ));
    }
    let root = read_root(paths)?;
    if root.root_digest == binding.old_root {
        let published = RootPublicationReceipt::published(binding, journal.digest);
        durable_replace(
            paths.root_record(),
            &published.canonical_bytes()?,
            ROOT_PUBLISH,
            fault,
        )?;
    } else {
        verify_new(&root, binding, journal.digest)?;
    }
    let committed = DurableJournalV2::committed(&journal.value, journal.digest);
    durable_replace(
        paths.journal_record(),
        &committed.canonical_bytes()?,
        COMMIT,
        fault,
    )?;
    persist_recovery(
        paths,
        make_recovery(
            binding,
            journal.digest,
            Some(committed.digest()?),
            binding.new_root,
            RecoveryOutcomeV1::NewRootCommitted,
            owner_death_digest(paths, binding)?,
        )?,
        fault,
    )
}

pub fn abort_journal_v1(
    paths: &JournalPathsV1,
    cartridge: &ContinuationCartridgeV1,
) -> Result<RecoveryReceiptV1, JournalErrorV1> {
    abort_journal_with_fault_v1(paths, cartridge, &mut FaultPlanV1::none())
}
pub fn abort_journal_with_fault_v1(
    paths: &JournalPathsV1,
    cartridge: &ContinuationCartridgeV1,
    fault: &mut FaultPlanV1,
) -> Result<RecoveryReceiptV1, JournalErrorV1> {
    if let Some(existing) = existing_recovery(paths, None)? {
        if existing.binding_digest == cartridge.binding_digest {
            return Ok(existing);
        }
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::ImmutableReceiptConflict,
            "recovery receipt and continuation cartridge bindings differ",
        ));
    }
    let journal = read_journal(paths)?;
    journal.value.validate()?;
    cartridge.validate_against(&journal.value.binding)?;
    let binding = &journal.value.binding;
    match journal.value.state {
        JournalStateV1::Committed => {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::AlreadyTerminal,
                "committed journal cannot abort",
            ));
        }
        JournalStateV1::Aborted => {
            let root = read_root(paths)?;
            verify_old(&root, binding)?;
            let prepared = prepared_digest(&journal.value, journal.digest)?;
            return persist_recovery(
                paths,
                make_recovery(
                    binding,
                    prepared,
                    Some(journal.digest),
                    root.root_digest,
                    RecoveryOutcomeV1::AlreadyAborted,
                    owner_death_digest(paths, binding)?,
                )?,
                fault,
            );
        }
        JournalStateV1::Prepared => {}
    }
    if journal.digest != cartridge.prepared_record_digest {
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::CartridgeMismatch,
            "prepared record digest differs from continuation cartridge",
        ));
    }
    let root = read_root(paths)?;
    verify_old(&root, binding)?;
    let aborted =
        DurableJournalV2::aborted(&journal.value, journal.digest, AbortReasonV1::ExplicitAbort);
    durable_replace(
        paths.journal_record(),
        &aborted.canonical_bytes()?,
        ABORT,
        fault,
    )?;
    persist_recovery(
        paths,
        make_recovery(
            binding,
            journal.digest,
            Some(aborted.digest()?),
            binding.old_root,
            RecoveryOutcomeV1::OldRootAborted,
            owner_death_digest(paths, binding)?,
        )?,
        fault,
    )
}

pub fn record_owner_death_v1(
    paths: &JournalPathsV1,
    owner: DigestV1,
    observed_at_unix_ns: u64,
) -> Result<OwnerDeathReceiptV1, JournalErrorV1> {
    record_owner_death_with_fault_v1(paths, owner, observed_at_unix_ns, &mut FaultPlanV1::none())
}
pub fn record_owner_death_with_fault_v1(
    paths: &JournalPathsV1,
    owner: DigestV1,
    observed_at_unix_ns: u64,
    fault: &mut FaultPlanV1,
) -> Result<OwnerDeathReceiptV1, JournalErrorV1> {
    let journal = read_journal(paths)?;
    journal.value.validate()?;
    if journal.value.binding.owner_identity_digest != owner {
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::OwnerIdentityMismatch,
            "owner-death identity differs from prepared binding",
        ));
    }
    if journal.value.state != JournalStateV1::Prepared {
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::AlreadyTerminal,
            "terminal journal does not require owner-death recovery",
        ));
    }
    let root = read_root(paths)?;
    let receipt = OwnerDeathReceiptV1 {
        schema_version: DURABLE_RECEIPT_SCHEMA_VERSION_V1,
        binding_digest: journal.value.binding.digest()?,
        prepared_record_digest: journal.digest,
        owner_identity_digest: owner,
        observed_journal_state: journal.value.state,
        observed_root: root.root_digest,
        observed_at_unix_ns,
        failure_code: JournalFailureCodeV1::OwnerDeath,
        recovery_required: true,
    };
    write_once(
        paths.owner_death_receipt(),
        &receipt.canonical_bytes()?,
        OWNER_DEATH,
        fault,
    )?;
    Ok(receipt)
}

pub fn recover_journal_v1(
    paths: &JournalPathsV1,
    expected: &JournalBindingV1,
) -> Result<RecoveryReceiptV1, JournalErrorV1> {
    recover_journal_with_fault_v1(paths, expected, &mut FaultPlanV1::none())
}
pub fn recover_journal_with_fault_v1(
    paths: &JournalPathsV1,
    expected: &JournalBindingV1,
    fault: &mut FaultPlanV1,
) -> Result<RecoveryReceiptV1, JournalErrorV1> {
    expected.validate()?;
    if let Some(existing) = existing_recovery(paths, Some(expected))? {
        return Ok(existing);
    }
    let root = read_root(paths)?;
    let Some(journal) = read_optional::<DurableJournalV2>(
        paths.journal_record(),
        JournalFailureCodeV1::JournalMissing,
    )?
    else {
        if root.root_digest != expected.old_root {
            return Err(disagreement("missing journal accompanies a non-old root"));
        }
        let Some(cartridge) = read_optional::<ContinuationCartridgeV1>(
            paths.cartridge(),
            JournalFailureCodeV1::CartridgeMismatch,
        )?
        else {
            return persist_recovery(
                paths,
                make_recovery(
                    expected,
                    DigestV1::ZERO,
                    None,
                    expected.old_root,
                    RecoveryOutcomeV1::NotStartedOldRoot,
                    None,
                )?,
                fault,
            );
        };
        cartridge.value.validate_against(expected)?;
        let prepared = DurableJournalV2::prepared(expected.clone());
        let prepared_digest = prepared.digest()?;
        if cartridge.value.prepared_record_digest != prepared_digest {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::CartridgeMismatch,
                "continuation cartridge does not bind the reconstructable prepared record",
            ));
        }
        let aborted = DurableJournalV2::aborted(
            &prepared,
            prepared_digest,
            AbortReasonV1::RecoveryObservedOldRoot,
        );
        durable_replace(
            paths.journal_record(),
            &aborted.canonical_bytes()?,
            ABORT,
            fault,
        )?;
        return persist_recovery(
            paths,
            make_recovery(
                expected,
                prepared_digest,
                Some(aborted.digest()?),
                expected.old_root,
                RecoveryOutcomeV1::OldRootAborted,
                None,
            )?,
            fault,
        );
    };
    journal.value.validate()?;
    if journal.value.binding.digest()? != expected.digest()? {
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::InvalidBinding,
            "persisted journal binding differs from recovery expectation",
        ));
    }
    let prepared = prepared_digest(&journal.value, journal.digest)?;
    let owner_death = owner_death_digest(paths, expected)?;
    match journal.value.state {
        JournalStateV1::Prepared if root.root_digest == expected.old_root => {
            let reason = if owner_death.is_some() {
                AbortReasonV1::OwnerDeathObservedOldRoot
            } else {
                AbortReasonV1::RecoveryObservedOldRoot
            };
            let aborted = DurableJournalV2::aborted(&journal.value, journal.digest, reason);
            durable_replace(
                paths.journal_record(),
                &aborted.canonical_bytes()?,
                ABORT,
                fault,
            )?;
            persist_recovery(
                paths,
                make_recovery(
                    expected,
                    prepared,
                    Some(aborted.digest()?),
                    expected.old_root,
                    RecoveryOutcomeV1::OldRootAborted,
                    owner_death,
                )?,
                fault,
            )
        }
        JournalStateV1::Prepared if root.root_digest == expected.new_root => {
            verify_new(&root, expected, prepared)?;
            let committed = DurableJournalV2::committed(&journal.value, journal.digest);
            durable_replace(
                paths.journal_record(),
                &committed.canonical_bytes()?,
                COMMIT,
                fault,
            )?;
            persist_recovery(
                paths,
                make_recovery(
                    expected,
                    prepared,
                    Some(committed.digest()?),
                    expected.new_root,
                    RecoveryOutcomeV1::NewRootCommitted,
                    owner_death,
                )?,
                fault,
            )
        }
        JournalStateV1::Prepared => Err(disagreement(
            "prepared journal accompanies neither preregistered root",
        )),
        JournalStateV1::Committed => {
            verify_new(&root, expected, prepared)?;
            persist_recovery(
                paths,
                make_recovery(
                    expected,
                    prepared,
                    Some(journal.digest),
                    expected.new_root,
                    RecoveryOutcomeV1::AlreadyCommitted,
                    owner_death,
                )?,
                fault,
            )
        }
        JournalStateV1::Aborted => {
            verify_old(&root, expected)?;
            persist_recovery(
                paths,
                make_recovery(
                    expected,
                    prepared,
                    Some(journal.digest),
                    expected.old_root,
                    RecoveryOutcomeV1::AlreadyAborted,
                    owner_death,
                )?,
                fault,
            )
        }
    }
}

pub fn read_published_root_v1(
    paths: &JournalPathsV1,
) -> Result<RootPublicationReceipt, JournalErrorV1> {
    read_root(paths)
}
pub fn read_journal_record_v1(paths: &JournalPathsV1) -> Result<DurableJournalV2, JournalErrorV1> {
    Ok(read_journal(paths)?.value)
}
pub fn read_continuation_cartridge_v1(
    paths: &JournalPathsV1,
) -> Result<ContinuationCartridgeV1, JournalErrorV1> {
    read_cartridge(paths)
}

fn make_recovery(
    binding: &JournalBindingV1,
    prepared: DigestV1,
    terminal: Option<DigestV1>,
    root: DigestV1,
    outcome: RecoveryOutcomeV1,
    owner: Option<DigestV1>,
) -> Result<RecoveryReceiptV1, JournalErrorV1> {
    Ok(RecoveryReceiptV1 {
        schema_version: DURABLE_RECEIPT_SCHEMA_VERSION_V1,
        binding_digest: binding.digest()?,
        prepared_record_digest: prepared,
        terminal_record_digest: terminal,
        owner_death_receipt_digest: owner,
        observed_root: root,
        outcome,
        journal_root_correspondence: true,
        promotable: true,
        failure_code: None,
    })
}
fn existing_recovery(
    paths: &JournalPathsV1,
    expected: Option<&JournalBindingV1>,
) -> Result<Option<RecoveryReceiptV1>, JournalErrorV1> {
    let Some(read) = read_optional::<RecoveryReceiptV1>(
        paths.recovery_receipt(),
        JournalFailureCodeV1::JournalMissing,
    )?
    else {
        return Ok(None);
    };
    read.value.canonical_bytes()?;
    if let Some(binding) = expected
        && read.value.binding_digest != binding.digest()?
    {
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::ImmutableReceiptConflict,
            "existing recovery receipt belongs to another binding",
        ));
    }
    Ok(Some(read.value))
}
fn persist_recovery(
    paths: &JournalPathsV1,
    receipt: RecoveryReceiptV1,
    fault: &mut FaultPlanV1,
) -> Result<RecoveryReceiptV1, JournalErrorV1> {
    if let Some(existing) = existing_recovery(paths, None)? {
        if existing.binding_digest == receipt.binding_digest {
            return Ok(existing);
        }
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::ImmutableReceiptConflict,
            "recovery receipt binding changed",
        ));
    }
    write_once(
        paths.recovery_receipt(),
        &receipt.canonical_bytes()?,
        RECOVERY,
        fault,
    )?;
    Ok(receipt)
}
fn owner_death_digest(
    paths: &JournalPathsV1,
    binding: &JournalBindingV1,
) -> Result<Option<DigestV1>, JournalErrorV1> {
    let Some(read) = read_optional::<OwnerDeathReceiptV1>(
        paths.owner_death_receipt(),
        JournalFailureCodeV1::JournalMissing,
    )?
    else {
        return Ok(None);
    };
    read.value.canonical_bytes()?;
    if read.value.binding_digest != binding.digest()?
        || read.value.owner_identity_digest != binding.owner_identity_digest
    {
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::OwnerIdentityMismatch,
            "owner-death receipt differs from journal binding",
        ));
    }
    Ok(Some(read.digest))
}
fn verify_old(
    root: &RootPublicationReceipt,
    binding: &JournalBindingV1,
) -> Result<(), JournalErrorV1> {
    root.validate()?;
    if root.root_digest == binding.old_root {
        Ok(())
    } else {
        Err(disagreement("aborted journal does not accompany old root"))
    }
}
fn verify_new(
    root: &RootPublicationReceipt,
    binding: &JournalBindingV1,
    prepared: DigestV1,
) -> Result<(), JournalErrorV1> {
    root.validate()?;
    if root.root_digest == binding.new_root
        && root.transaction_id == binding.transaction_id
        && root.prepared_record_digest == prepared
    {
        Ok(())
    } else {
        Err(disagreement(
            "committed journal does not accompany its bound new root",
        ))
    }
}
fn prepared_digest(
    record: &DurableJournalV2,
    digest: DigestV1,
) -> Result<DigestV1, JournalErrorV1> {
    match record.state {
        JournalStateV1::Prepared => Ok(digest),
        _ => record.predecessor_record_digest.ok_or_else(|| {
            JournalErrorV1::new(
                JournalFailureCodeV1::SequenceMismatch,
                "terminal journal lacks prepared predecessor",
            )
        }),
    }
}
fn disagreement(detail: &str) -> JournalErrorV1 {
    JournalErrorV1::new(JournalFailureCodeV1::JournalRootDisagreement, detail)
}

struct CanonicalRead<T> {
    value: T,
    digest: DigestV1,
}
fn read_root(paths: &JournalPathsV1) -> Result<RootPublicationReceipt, JournalErrorV1> {
    let read = read_canonical::<RootPublicationReceipt>(
        paths.root_record(),
        JournalFailureCodeV1::RootMissing,
    )?;
    read.value.validate()?;
    Ok(read.value)
}
fn read_journal(paths: &JournalPathsV1) -> Result<CanonicalRead<DurableJournalV2>, JournalErrorV1> {
    read_canonical(paths.journal_record(), JournalFailureCodeV1::JournalMissing)
}
fn read_cartridge(paths: &JournalPathsV1) -> Result<ContinuationCartridgeV1, JournalErrorV1> {
    Ok(read_canonical::<ContinuationCartridgeV1>(
        paths.cartridge(),
        JournalFailureCodeV1::CartridgeMismatch,
    )?
    .value)
}
fn read_optional<T>(
    path: &Path,
    missing: JournalFailureCodeV1,
) -> Result<Option<CanonicalRead<T>>, JournalErrorV1>
where
    T: DeserializeOwned + Serialize,
{
    match read_canonical(path, missing) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.code == missing => Ok(None),
        Err(error) => Err(error),
    }
}
fn read_canonical<T>(
    path: &Path,
    missing: JournalFailureCodeV1,
) -> Result<CanonicalRead<T>, JournalErrorV1>
where
    T: DeserializeOwned + Serialize,
{
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(JournalErrorV1::new(missing, "required record is absent"));
        }
        Err(error) => {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::IoBeforePublish,
                format!("record stat failed: {error}"),
            ));
        }
    };
    if !metadata.file_type().is_file() {
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::TornOrNoncanonicalRecord,
            "record is not a regular file",
        ));
    }
    if metadata.len() > DURABLE_JOURNAL_MAX_RECORD_BYTES_V1 {
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::RecordTooLarge,
            "record exceeds the frozen byte bound",
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        JournalErrorV1::new(
            JournalFailureCodeV1::IoBeforePublish,
            format!("record read failed: {error}"),
        )
    })?;
    let value: T = serde_json::from_slice(&bytes).map_err(|error| {
        JournalErrorV1::new(
            JournalFailureCodeV1::TornOrNoncanonicalRecord,
            format!("record decode failed: {error}"),
        )
    })?;
    let canonical = canonical_bytes(&value)?;
    if canonical != bytes {
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::TornOrNoncanonicalRecord,
            "record bytes are not canonical JSON",
        ));
    }
    Ok(CanonicalRead {
        value,
        digest: typed_domain_digest::<T>(&canonical)?,
    })
}
fn typed_domain_digest<T>(bytes: &[u8]) -> Result<DigestV1, JournalErrorV1> {
    let name = std::any::type_name::<T>();
    let digest = if name.ends_with("DurableJournalV2") {
        domain_digest(JOURNAL_DOMAIN_V2, bytes)
    } else if name.ends_with("RootPublicationReceipt") {
        domain_digest(ROOT_DOMAIN_V1, bytes)
    } else if name.ends_with("ContinuationCartridgeV1") {
        domain_digest(CARTRIDGE_DOMAIN_V1, bytes)
    } else if name.ends_with("OwnerDeathReceiptV1") {
        domain_digest(OWNER_DEATH_DOMAIN_V1, bytes)
    } else if name.ends_with("RecoveryReceiptV1") {
        domain_digest(RECOVERY_DOMAIN_V1, bytes)
    } else {
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::InvalidBinding,
            format!("no frozen digest domain for record type {name}"),
        ));
    };
    Ok(digest)
}
fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, JournalErrorV1> {
    let value = serde_json::to_value(value).map_err(|error| {
        JournalErrorV1::new(
            JournalFailureCodeV1::InvalidBinding,
            format!("record serialization failed: {error}"),
        )
    })?;
    let bytes = canonical_json(&value).into_bytes();
    if bytes.len() as u64 > DURABLE_JOURNAL_MAX_RECORD_BYTES_V1 {
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::RecordTooLarge,
            "canonical record exceeds the frozen byte bound",
        ));
    }
    Ok(bytes)
}
fn domain_digest(domain: &[u8], bytes: &[u8]) -> DigestV1 {
    let mut bound = Vec::with_capacity(domain.len() + 8 + bytes.len());
    bound.extend_from_slice(domain);
    bound.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    bound.extend_from_slice(bytes);
    DigestV1::from_bytes(sha256(&bound))
}
fn write_once(
    path: &Path,
    bytes: &[u8],
    boundaries: WriteBoundaries,
    fault: &mut FaultPlanV1,
) -> Result<(), JournalErrorV1> {
    match fs::read(path) {
        Ok(existing) if existing == bytes => return Ok(()),
        Ok(_) => {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::ImmutableReceiptConflict,
                "immutable record already exists with different bytes",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(JournalErrorV1::new(
                JournalFailureCodeV1::IoBeforePublish,
                format!("immutable record read failed: {error}"),
            ));
        }
    }
    durable_replace(path, bytes, boundaries, fault)
}
fn durable_replace(
    path: &Path,
    bytes: &[u8],
    boundaries: WriteBoundaries,
    fault: &mut FaultPlanV1,
) -> Result<(), JournalErrorV1> {
    if bytes.len() as u64 > DURABLE_JOURNAL_MAX_RECORD_BYTES_V1 {
        return Err(JournalErrorV1::new(
            JournalFailureCodeV1::RecordTooLarge,
            "write exceeds the frozen record byte bound",
        ));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| io_before(boundaries.before, error))?;
    let file_name = path.file_name().ok_or_else(|| {
        JournalErrorV1::new(
            JournalFailureCodeV1::InvalidBinding,
            "record path has no file name",
        )
    })?;
    let (mut file, temp) =
        open_unique_temp(parent, file_name).map_err(|error| io_before(boundaries.before, error))?;
    let mut published = false;
    let result = (|| {
        fault.hit(boundaries.before, false)?;
        file.write_all(bytes)
            .map_err(|error| io_before(boundaries.before, error))?;
        file.sync_all()
            .map_err(|error| io_before(boundaries.file_sync, error))?;
        fault.hit(boundaries.file_sync, false)?;
        drop(file);
        replace_file(&temp, path).map_err(|error| io_before(boundaries.rename, error))?;
        published = true;
        fault.hit(boundaries.rename, true)?;
        sync_dir(parent).map_err(|error| {
            JournalErrorV1::at(
                JournalFailureCodeV1::DirectorySyncFailedAfterPublish,
                boundaries.dir_sync,
                true,
                format!("parent directory sync failed: {error}"),
            )
        })?;
        fault.hit(boundaries.dir_sync, true)?;
        Ok(())
    })();
    if !published {
        let _ = fs::remove_file(&temp);
    }
    result
}
fn open_unique_temp(parent: &Path, file_name: &std::ffi::OsStr) -> io::Result<(File, PathBuf)> {
    loop {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut name = std::ffi::OsString::from(".");
        name.push(file_name);
        name.push(format!(".journal-tmp-{}-{sequence}", std::process::id()));
        let path = parent.join(name);
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}
fn io_before(boundary: JournalBoundaryV1, error: io::Error) -> JournalErrorV1 {
    JournalErrorV1::at(
        JournalFailureCodeV1::IoBeforePublish,
        boundary,
        false,
        format!("durable write failed before publication: {error}"),
    )
}

/// Machine-readable contract summary used by conformance generators.
pub fn durable_journal_contract_v1() -> serde_json::Value {
    json!({
        "schema_version": DURABLE_JOURNAL_SCHEMA_VERSION_V2,
        "journal_schema_version": DURABLE_JOURNAL_SCHEMA_VERSION_V2,
        "receipt_schema_version": DURABLE_RECEIPT_SCHEMA_VERSION_V1,
        "max_record_bytes": DURABLE_JOURNAL_MAX_RECORD_BYTES_V1,
        "states": ["prepared", "committed", "aborted"],
        "publication_law": "root_is_old_or_new_and_journal_corresponds",
        "prepare_order": ["cartridge_file_sync", "cartridge_directory_sync",
            "prepared_file_sync", "prepared_directory_sync"],
        "commit_order": ["root_file_sync", "root_rename", "root_directory_sync",
            "committed_file_sync", "committed_directory_sync"],
        "abort_order": ["old_root_verified", "aborted_file_sync", "aborted_directory_sync"],
        "recovery": "prepared_plus_old_aborts; prepared_plus_new_commits; other_pairings_reject",
        "immutable_receipts": ["continuation_cartridge", "owner_death", "recovery"],
        "typed_failure_codes": ["schema_version_mismatch", "invalid_binding",
            "profile_substitution", "journal_missing", "root_missing",
            "torn_or_noncanonical_record", "record_too_large", "cartridge_mismatch",
            "owner_identity_mismatch", "sequence_mismatch", "root_mismatch",
            "journal_root_disagreement", "already_terminal", "immutable_receipt_conflict",
            "io_before_publish", "directory_sync_failed_after_publish", "injected_crash",
            "owner_death", "indeterminate"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }
    fn paths(directory: &Path) -> JournalPathsV1 {
        JournalPathsV1::new(
            directory.join("root.json"),
            directory.join("journal.json"),
            directory.join("cartridge.json"),
            directory.join("owner-death.json"),
            directory.join("recovery.json"),
        )
        .unwrap()
    }
    fn binding() -> JournalBindingV1 {
        JournalBindingV1::new(
            digest(1),
            digest(2),
            DurableProfileIdV1::PortableStrict,
            digest(3),
            digest(4),
            digest(5),
        )
    }
    fn setup() -> (tempfile::TempDir, JournalPathsV1, JournalBindingV1) {
        let directory = tempdir().unwrap();
        let paths = paths(directory.path());
        let binding = binding();
        initialize_published_root_v1(&paths, binding.old_root).unwrap();
        (directory, paths, binding)
    }
    #[test]
    fn journal_recovery_commit_and_abort_are_idempotent() {
        let (_directory, journal_paths, binding) = setup();
        let cartridge = prepare_journal_v1(&journal_paths, binding.clone()).unwrap();
        let committed = commit_journal_v1(&journal_paths, &cartridge).unwrap();
        assert_eq!(committed.outcome, RecoveryOutcomeV1::NewRootCommitted);
        assert_eq!(
            read_published_root_v1(&journal_paths).unwrap().root_digest,
            binding.new_root
        );
        assert_eq!(
            read_journal_record_v1(&journal_paths).unwrap().state,
            JournalStateV1::Committed
        );
        assert_eq!(
            recover_journal_v1(&journal_paths, &binding).unwrap(),
            committed
        );
        let second = tempdir().unwrap();
        let second_paths = paths(second.path());
        initialize_published_root_v1(&second_paths, binding.old_root).unwrap();
        let cartridge = prepare_journal_v1(&second_paths, binding.clone()).unwrap();
        let aborted = abort_journal_v1(&second_paths, &cartridge).unwrap();
        assert_eq!(aborted.outcome, RecoveryOutcomeV1::OldRootAborted);
        assert_eq!(
            recover_journal_v1(&second_paths, &binding).unwrap(),
            aborted
        );
    }
    #[test]
    fn journal_recovery_owner_death_is_typed_and_completes_safely() {
        let (_directory, paths, binding) = setup();
        prepare_journal_v1(&paths, binding.clone()).unwrap();
        let owner = record_owner_death_v1(&paths, binding.owner_identity_digest, 77).unwrap();
        assert_eq!(owner.failure_code, JournalFailureCodeV1::OwnerDeath);
        let recovered = recover_journal_v1(&paths, &binding).unwrap();
        assert_eq!(recovered.outcome, RecoveryOutcomeV1::OldRootAborted);
        assert_eq!(
            recovered.owner_death_receipt_digest,
            Some(owner.digest().unwrap())
        );
    }
    #[test]
    fn journal_recovery_torn_and_profile_substitution_fail_loudly() {
        let (_directory, paths, binding) = setup();
        prepare_journal_v1(&paths, binding.clone()).unwrap();
        fs::write(paths.journal_record(), b"{\"schema_version\":1").unwrap();
        assert_eq!(
            recover_journal_v1(&paths, &binding).unwrap_err().code,
            JournalFailureCodeV1::TornOrNoncanonicalRecord
        );
        let mut substituted = binding;
        substituted.durable_profile_id = DurableProfileIdV1::NtfsStrict;
        assert_eq!(
            substituted.validate().unwrap_err().code,
            JournalFailureCodeV1::ProfileSubstitution
        );
    }
    #[test]
    fn journal_recovery_root_disagreement_is_never_guessed() {
        let (_directory, paths, binding) = setup();
        prepare_journal_v1(&paths, binding.clone()).unwrap();
        let unrelated = RootPublicationReceipt::initial(digest(9)).unwrap();
        fs::write(paths.root_record(), unrelated.canonical_bytes().unwrap()).unwrap();
        assert_eq!(
            recover_journal_v1(&paths, &binding).unwrap_err().code,
            JournalFailureCodeV1::JournalRootDisagreement
        );
    }

    #[test]
    fn journal_recovery_finishes_a_cartridge_only_prepare_as_abort() {
        let (_directory, paths, binding) = setup();
        let mut fault = FaultPlanV1::crash_at(JournalBoundaryV1::PrepareBeforeWrite);
        assert_eq!(
            prepare_journal_with_fault_v1(&paths, binding.clone(), &mut fault)
                .unwrap_err()
                .code,
            JournalFailureCodeV1::InjectedCrash
        );
        let recovered = recover_journal_v1(&paths, &binding).unwrap();
        assert_eq!(recovered.outcome, RecoveryOutcomeV1::OldRootAborted);
        assert_ne!(recovered.prepared_record_digest, DigestV1::ZERO);
        assert_eq!(
            read_journal_record_v1(&paths).unwrap().state,
            JournalStateV1::Aborted
        );
    }

    #[test]
    fn journal_recovery_rejects_a_foreign_cartridge() {
        let (_directory, paths, binding) = setup();
        let mut fault = FaultPlanV1::crash_at(JournalBoundaryV1::PrepareBeforeWrite);
        prepare_journal_with_fault_v1(&paths, binding.clone(), &mut fault).unwrap_err();
        let mut foreign = binding;
        foreign.transaction_id = digest(9);
        assert_eq!(
            recover_journal_v1(&paths, &foreign).unwrap_err().code,
            JournalFailureCodeV1::CartridgeMismatch
        );
    }

    #[test]
    fn journal_owner_death_after_new_root_publication_finishes_the_commit() {
        let (_directory, paths, binding) = setup();
        let cartridge = prepare_journal_v1(&paths, binding.clone()).unwrap();
        let mut fault = FaultPlanV1::crash_at(JournalBoundaryV1::CommitBeforeWrite);
        assert_eq!(
            commit_journal_with_fault_v1(&paths, &cartridge, &mut fault)
                .unwrap_err()
                .code,
            JournalFailureCodeV1::InjectedCrash
        );
        let owner = record_owner_death_v1(&paths, binding.owner_identity_digest, 77).unwrap();
        assert_eq!(owner.observed_root, binding.new_root);
        let recovered = recover_journal_v1(&paths, &binding).unwrap();
        assert_eq!(recovered.outcome, RecoveryOutcomeV1::NewRootCommitted);
        assert_eq!(
            recovered.owner_death_receipt_digest,
            Some(owner.digest().unwrap())
        );
    }
}
