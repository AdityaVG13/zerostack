//! Durable single-root journal and recovery protocol.
//!
//! The caller supplies every path. This module does not introduce a store
//! layout. A transaction first persists a continuation cartridge, then a
//! prepared journal record. Commit publishes a synced root record and only
//! then persists a committed journal record. Recovery accepts only the old
//! root or the new root; every other journal/root pairing fails loudly.
//!
//! Two binding formats share one state machine (generic over
//! [`JournalBindingLike`]): the v1 four-term binding (old/new root +
//! transaction + owner) kept for compatibility, and the v2 five-term binding
//! (ZS-STORE-006) that adds nonce, protected scope, and a lease to the same
//! atomic record. The v2 commit surface (`prepare_lease_journal`,
//! `commit_lease_journal`, ...) is exercised by the two-writer commit-surface
//! integration test.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use zero_abi::{Sha256Digest, canonical_json, sha256};

use crate::fs_replace::{replace_file, sync_dir};
use crate::{DurableProfile, DurableProfileId};

pub const DURABLE_JOURNAL_SCHEMA_VERSION: u16 = 2;
pub const DURABLE_LEASE_JOURNAL_SCHEMA_VERSION: u16 = 3;
pub const DURABLE_BINDING_SCHEMA_VERSION: u16 = 1;
pub const DURABLE_LEASE_BINDING_SCHEMA_VERSION: u16 = 2;
pub const DURABLE_LEASE_SCHEMA_VERSION: u16 = 1;
pub const DURABLE_RECEIPT_SCHEMA_VERSION: u16 = 1;
pub const DURABLE_JOURNAL_MAX_RECORD_BYTES: u64 = 64 * 1024;

const BINDING_DOMAIN: &[u8] = b"zerostack.durable_journal.binding\0";
const LEASE_BINDING_DOMAIN: &[u8] = b"zerostack.durable_journal.lease_binding\0";
const JOURNAL_DOMAIN: &[u8] = b"zerostack.durable_journal.record\0";
const LEASE_JOURNAL_DOMAIN: &[u8] = b"zerostack.durable_journal.lease_record\0";
const ROOT_DOMAIN: &[u8] = b"zerostack.durable_journal.root\0";
const CARTRIDGE_DOMAIN: &[u8] = b"zerostack.durable_journal.cartridge\0";
const OWNER_DEATH_DOMAIN: &[u8] = b"zerostack.durable_journal.owner_death\0";
const RECOVERY_DOMAIN: &[u8] = b"zerostack.durable_journal.recovery\0";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalState {
    Prepared,
    Committed,
    Aborted,
}

impl JournalState {
    /// Explicit authority matrix: every terminal state has exactly one
    /// producer. Prepared may move to Committed (commit) or Aborted (abort
    /// or recovery); terminal states never leave. Idempotent replay of the
    /// same terminal is allowed only when the authority's receipt is already
    /// persisted and verified (handled at call sites), not via state transition.
    pub fn can_transition_to(self, next: JournalState) -> bool {
        matches!(
            (self, next),
            (JournalState::Prepared, JournalState::Committed)
                | (JournalState::Prepared, JournalState::Aborted)
        )
    }
    pub fn is_terminal(self) -> bool {
        matches!(self, JournalState::Committed | JournalState::Aborted)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AbortReason {
    ExplicitAbort,
    RecoveryObservedOldRoot,
    OwnerDeathObservedOldRoot,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryOutcome {
    NotStartedOldRoot,
    OldRootAborted,
    NewRootCommitted,
    AlreadyCommitted,
    AlreadyAborted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalFailureCode {
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
    LeaseExpired,
    ImmutableReceiptConflict,
    IoBeforePublish,
    DirectorySyncFailedAfterPublish,
    InjectedCrash,
    OwnerDeath,
    Indeterminate,
}

impl JournalFailureCode {
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
            Self::LeaseExpired => "lease_expired",
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
pub enum JournalBoundary {
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
pub struct JournalError {
    pub code: JournalFailureCode,
    pub boundary: Option<JournalBoundary>,
    pub publication_may_have_changed: bool,
    pub detail: String,
}

impl JournalError {
    fn new(code: JournalFailureCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            boundary: None,
            publication_may_have_changed: false,
            detail: detail.into(),
        }
    }
    fn at(
        code: JournalFailureCode,
        boundary: JournalBoundary,
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
impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}
impl std::error::Error for JournalError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalPaths {
    root_record: PathBuf,
    journal_record: PathBuf,
    cartridge: PathBuf,
    owner_death_receipt: PathBuf,
    recovery_receipt: PathBuf,
}
impl JournalPaths {
    pub fn new(
        root_record: impl Into<PathBuf>,
        journal_record: impl Into<PathBuf>,
        cartridge: impl Into<PathBuf>,
        owner_death_receipt: impl Into<PathBuf>,
        recovery_receipt: impl Into<PathBuf>,
    ) -> Result<Self, JournalError> {
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
            return Err(JournalError::new(
                JournalFailureCode::InvalidBinding,
                "every journal path must name a file",
            ));
        }
        for (index, left) in all.iter().enumerate() {
            if all.iter().skip(index + 1).any(|right| left == right) {
                return Err(JournalError::new(
                    JournalFailureCode::InvalidBinding,
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
pub struct JournalBinding {
    pub schema_version: u16,
    pub transaction_id: Sha256Digest,
    pub assembly_manifest_digest: Sha256Digest,
    pub durable_profile_id: DurableProfileId,
    pub durable_profile_digest: Sha256Digest,
    pub old_root: Sha256Digest,
    pub new_root: Sha256Digest,
    pub owner_identity_digest: Sha256Digest,
}
impl JournalBinding {
    pub fn new(
        transaction_id: Sha256Digest,
        assembly_manifest_digest: Sha256Digest,
        durable_profile_id: DurableProfileId,
        old_root: Sha256Digest,
        new_root: Sha256Digest,
        owner_identity_digest: Sha256Digest,
    ) -> Self {
        Self {
            schema_version: DURABLE_BINDING_SCHEMA_VERSION,
            transaction_id,
            assembly_manifest_digest,
            durable_profile_id,
            durable_profile_digest: DurableProfile::new(durable_profile_id).digest(),
            old_root,
            new_root,
            owner_identity_digest,
        }
    }
    pub fn validate(&self) -> Result<(), JournalError> {
        if self.schema_version != DURABLE_BINDING_SCHEMA_VERSION {
            return Err(JournalError::new(
                JournalFailureCode::SchemaVersionMismatch,
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
        .contains(&Sha256Digest::ZERO)
        {
            return Err(JournalError::new(
                JournalFailureCode::InvalidBinding,
                "binding digests must be nonzero",
            ));
        }
        if self.old_root == self.new_root {
            return Err(JournalError::new(
                JournalFailureCode::InvalidBinding,
                "old and new roots must differ",
            ));
        }
        if self.durable_profile_digest != DurableProfile::new(self.durable_profile_id).digest() {
            return Err(JournalError::new(
                JournalFailureCode::ProfileSubstitution,
                "durable profile identity does not match its frozen digest",
            ));
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, JournalError> {
        self.validate()?;
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<Sha256Digest, JournalError> {
        Ok(domain_digest(BINDING_DOMAIN, &self.canonical_bytes()?))
    }
}

/// The contract the durable journal state machine requires of a commit
/// binding. Both the v1 four-term and the v2 five-term binding satisfy it, so
/// one crash-safe state machine serves both formats.
pub trait JournalBindingLike:
    Clone + std::fmt::Debug + PartialEq + Serialize + DeserializeOwned
{
    fn binding_validate(&self) -> Result<(), JournalError>;
    fn binding_digest(&self) -> Result<Sha256Digest, JournalError>;
    fn old_root(&self) -> Sha256Digest;
    fn new_root(&self) -> Sha256Digest;
    fn transaction_id(&self) -> Sha256Digest;
    fn durable_profile_digest(&self) -> Sha256Digest;
    fn owner_identity_digest(&self) -> Sha256Digest;
    /// Lease deadline for fresh attempts; `None` means the binding carries no
    /// lease and no expiry gate applies.
    fn lease_expires_at_unix_ns(&self) -> Option<u64>;
    /// Schema version stamped on journal records carrying this binding.
    fn record_schema_version() -> u16;
    /// Frozen digest domain for journal records carrying this binding.
    fn record_domain() -> &'static [u8];
}

impl JournalBindingLike for JournalBinding {
    fn binding_validate(&self) -> Result<(), JournalError> {
        self.validate()
    }
    fn binding_digest(&self) -> Result<Sha256Digest, JournalError> {
        self.digest()
    }
    fn old_root(&self) -> Sha256Digest {
        self.old_root
    }
    fn new_root(&self) -> Sha256Digest {
        self.new_root
    }
    fn transaction_id(&self) -> Sha256Digest {
        self.transaction_id
    }
    fn durable_profile_digest(&self) -> Sha256Digest {
        self.durable_profile_digest
    }
    fn owner_identity_digest(&self) -> Sha256Digest {
        self.owner_identity_digest
    }
    fn lease_expires_at_unix_ns(&self) -> Option<u64> {
        None
    }
    fn record_schema_version() -> u16 {
        DURABLE_JOURNAL_SCHEMA_VERSION
    }
    fn record_domain() -> &'static [u8] {
        JOURNAL_DOMAIN
    }
}

/// Lease term carried inside a five-term commit binding. The lease names the
/// protection under which the commit is authorized; expiry gates fresh
/// attempts (prepare), and the journal's immutable-record law refuses any
/// replay of a consumed lease instead of silently re-executing.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BindingLease {
    pub schema_version: u16,
    pub lease_id: Sha256Digest,
    pub epoch: u64,
    pub expires_at_unix_ns: u64,
}
impl BindingLease {
    pub fn new(lease_id: Sha256Digest, epoch: u64, expires_at_unix_ns: u64) -> Self {
        Self {
            schema_version: DURABLE_LEASE_SCHEMA_VERSION,
            lease_id,
            epoch,
            expires_at_unix_ns,
        }
    }
    pub fn validate(&self) -> Result<(), JournalError> {
        if self.schema_version != DURABLE_LEASE_SCHEMA_VERSION {
            return Err(JournalError::new(
                JournalFailureCode::SchemaVersionMismatch,
                "lease schema version is not supported",
            ));
        }
        if self.lease_id == Sha256Digest::ZERO {
            return Err(JournalError::new(
                JournalFailureCode::InvalidBinding,
                "lease identity must be nonzero",
            ));
        }
        if self.epoch == 0 {
            return Err(JournalError::new(
                JournalFailureCode::InvalidBinding,
                "lease epoch must be positive",
            ));
        }
        if self.expires_at_unix_ns == 0 {
            return Err(JournalError::new(
                JournalFailureCode::InvalidBinding,
                "lease expiry must be nonzero",
            ));
        }
        Ok(())
    }
}

/// Five-term commit binding (ZS-STORE-006): parent root/epoch (`old_root`),
/// authorized delta root (`new_root`), protected scope, nonce, and lease are
/// all captured in one atomic record, together with the session/ledger
/// identity (`owner_identity_digest`) that produced the commit. A committed
/// record therefore carries full provenance, and reads can verify the binding
/// (see [`verify_committed_lease_binding`]).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JournalLeaseBinding {
    pub schema_version: u16,
    pub transaction_id: Sha256Digest,
    pub assembly_manifest_digest: Sha256Digest,
    pub durable_profile_id: DurableProfileId,
    pub durable_profile_digest: Sha256Digest,
    pub old_root: Sha256Digest,
    pub new_root: Sha256Digest,
    pub owner_identity_digest: Sha256Digest,
    pub nonce: Sha256Digest,
    pub protected_scope: Sha256Digest,
    pub lease: BindingLease,
}
impl JournalLeaseBinding {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        transaction_id: Sha256Digest,
        assembly_manifest_digest: Sha256Digest,
        durable_profile_id: DurableProfileId,
        old_root: Sha256Digest,
        new_root: Sha256Digest,
        owner_identity_digest: Sha256Digest,
        nonce: Sha256Digest,
        protected_scope: Sha256Digest,
        lease: BindingLease,
    ) -> Self {
        Self {
            schema_version: DURABLE_LEASE_BINDING_SCHEMA_VERSION,
            transaction_id,
            assembly_manifest_digest,
            durable_profile_id,
            durable_profile_digest: DurableProfile::new(durable_profile_id).digest(),
            old_root,
            new_root,
            owner_identity_digest,
            nonce,
            protected_scope,
            lease,
        }
    }
    pub fn validate(&self) -> Result<(), JournalError> {
        if self.schema_version != DURABLE_LEASE_BINDING_SCHEMA_VERSION {
            return Err(JournalError::new(
                JournalFailureCode::SchemaVersionMismatch,
                "five-term binding schema version is not supported",
            ));
        }
        if [
            self.transaction_id,
            self.assembly_manifest_digest,
            self.old_root,
            self.new_root,
            self.owner_identity_digest,
            self.nonce,
            self.protected_scope,
        ]
        .contains(&Sha256Digest::ZERO)
        {
            return Err(JournalError::new(
                JournalFailureCode::InvalidBinding,
                "five-term binding digests must be nonzero",
            ));
        }
        if self.old_root == self.new_root {
            return Err(JournalError::new(
                JournalFailureCode::InvalidBinding,
                "old and new roots must differ",
            ));
        }
        if self.durable_profile_digest != DurableProfile::new(self.durable_profile_id).digest() {
            return Err(JournalError::new(
                JournalFailureCode::ProfileSubstitution,
                "durable profile identity does not match its frozen digest",
            ));
        }
        self.lease.validate()?;
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, JournalError> {
        self.validate()?;
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<Sha256Digest, JournalError> {
        Ok(domain_digest(
            LEASE_BINDING_DOMAIN,
            &self.canonical_bytes()?,
        ))
    }
}
impl JournalBindingLike for JournalLeaseBinding {
    fn binding_validate(&self) -> Result<(), JournalError> {
        self.validate()
    }
    fn binding_digest(&self) -> Result<Sha256Digest, JournalError> {
        self.digest()
    }
    fn old_root(&self) -> Sha256Digest {
        self.old_root
    }
    fn new_root(&self) -> Sha256Digest {
        self.new_root
    }
    fn transaction_id(&self) -> Sha256Digest {
        self.transaction_id
    }
    fn durable_profile_digest(&self) -> Sha256Digest {
        self.durable_profile_digest
    }
    fn owner_identity_digest(&self) -> Sha256Digest {
        self.owner_identity_digest
    }
    fn lease_expires_at_unix_ns(&self) -> Option<u64> {
        Some(self.lease.expires_at_unix_ns)
    }
    fn record_schema_version() -> u16 {
        DURABLE_LEASE_JOURNAL_SCHEMA_VERSION
    }
    fn record_domain() -> &'static [u8] {
        LEASE_JOURNAL_DOMAIN
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DurableJournalRecord<B> {
    pub schema_version: u16,
    pub binding: B,
    pub state: JournalState,
    pub sequence: u64,
    pub predecessor_record_digest: Option<Sha256Digest>,
    pub abort_reason: Option<AbortReason>,
}
/// Durable journal record carrying the v1 four-term binding. Byte-compatible
/// with the pre-R9 record format.
pub type DurableJournal = DurableJournalRecord<JournalBinding>;
/// Durable journal record carrying the v2 five-term binding (nonce +
/// protected scope + lease added to the v1 terms).
pub type DurableLeaseJournal = DurableJournalRecord<JournalLeaseBinding>;

impl<B: JournalBindingLike> DurableJournalRecord<B> {
    fn prepared(binding: B) -> Self {
        Self {
            schema_version: B::record_schema_version(),
            binding,
            state: JournalState::Prepared,
            sequence: 1,
            predecessor_record_digest: None,
            abort_reason: None,
        }
    }
    fn committed(prepared: &Self, digest: Sha256Digest) -> Self {
        Self {
            schema_version: prepared.schema_version,
            binding: prepared.binding.clone(),
            state: JournalState::Committed,
            sequence: 2,
            predecessor_record_digest: Some(digest),
            abort_reason: None,
        }
    }
    fn aborted(prepared: &Self, digest: Sha256Digest, reason: AbortReason) -> Self {
        Self {
            schema_version: prepared.schema_version,
            binding: prepared.binding.clone(),
            state: JournalState::Aborted,
            sequence: 2,
            predecessor_record_digest: Some(digest),
            abort_reason: Some(reason),
        }
    }
    pub fn validate(&self) -> Result<(), JournalError> {
        if self.schema_version != B::record_schema_version() {
            return Err(JournalError::new(
                JournalFailureCode::SchemaVersionMismatch,
                "journal record schema version is not supported",
            ));
        }
        self.binding.binding_validate()?;
        let valid = match self.state {
            JournalState::Prepared => {
                self.sequence == 1
                    && self.predecessor_record_digest.is_none()
                    && self.abort_reason.is_none()
            }
            JournalState::Committed => {
                self.sequence == 2
                    && self.predecessor_record_digest.is_some()
                    && self.abort_reason.is_none()
            }
            JournalState::Aborted => {
                self.sequence == 2
                    && self.predecessor_record_digest.is_some()
                    && self.abort_reason.is_some()
            }
        };
        if !valid {
            return Err(JournalError::new(
                JournalFailureCode::SequenceMismatch,
                "journal state and sequence commitments disagree",
            ));
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, JournalError> {
        self.validate()?;
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<Sha256Digest, JournalError> {
        Ok(domain_digest(B::record_domain(), &self.canonical_bytes()?))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootPublicationReceipt {
    pub schema_version: u16,
    pub transaction_id: Sha256Digest,
    pub root_digest: Sha256Digest,
    pub generation: u64,
    pub prepared_record_digest: Sha256Digest,
}
impl RootPublicationReceipt {
    fn initial(root_digest: Sha256Digest) -> Result<Self, JournalError> {
        if root_digest == Sha256Digest::ZERO {
            return Err(JournalError::new(
                JournalFailureCode::InvalidBinding,
                "initial root digest must be nonzero",
            ));
        }
        Ok(Self {
            schema_version: DURABLE_RECEIPT_SCHEMA_VERSION,
            transaction_id: Sha256Digest::ZERO,
            root_digest,
            generation: 0,
            prepared_record_digest: Sha256Digest::ZERO,
        })
    }
    fn published<B: JournalBindingLike>(binding: &B, prepared_record_digest: Sha256Digest) -> Self {
        Self {
            schema_version: DURABLE_RECEIPT_SCHEMA_VERSION,
            transaction_id: binding.transaction_id(),
            root_digest: binding.new_root(),
            generation: 1,
            prepared_record_digest,
        }
    }
    pub fn validate(&self) -> Result<(), JournalError> {
        if self.schema_version != DURABLE_RECEIPT_SCHEMA_VERSION {
            return Err(JournalError::new(
                JournalFailureCode::SchemaVersionMismatch,
                "published root schema version is not supported",
            ));
        }
        let initial = self.generation == 0
            && self.transaction_id == Sha256Digest::ZERO
            && self.prepared_record_digest == Sha256Digest::ZERO;
        let transactional = self.generation == 1
            && self.transaction_id != Sha256Digest::ZERO
            && self.prepared_record_digest != Sha256Digest::ZERO;
        if self.root_digest == Sha256Digest::ZERO || !(initial || transactional) {
            return Err(JournalError::new(
                JournalFailureCode::RootMismatch,
                "root generation and transaction commitments disagree",
            ));
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, JournalError> {
        self.validate()?;
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<Sha256Digest, JournalError> {
        Ok(domain_digest(ROOT_DOMAIN, &self.canonical_bytes()?))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationCartridgeRecord<B> {
    pub schema_version: u16,
    pub binding_digest: Sha256Digest,
    pub prepared_record_digest: Sha256Digest,
    pub transaction_id: Sha256Digest,
    pub old_root: Sha256Digest,
    pub new_root: Sha256Digest,
    pub durable_profile_digest: Sha256Digest,
    pub owner_identity_digest: Sha256Digest,
    // The phantom keeps a v1 cartridge and a v2 cartridge distinct types so
    // the state machine cannot mix binding formats. Serde skips it.
    _binding: std::marker::PhantomData<B>,
}
/// Compatibility alias for callers written against the carrier draft.
pub type JournalRecord = DurableJournal;
/// Compatibility alias for callers written against the carrier draft.
pub type PublishedRoot = RootPublicationReceipt;
/// Cartridge for the v1 four-term binding (pre-R9 shape).
pub type ContinuationCartridge = ContinuationCartridgeRecord<JournalBinding>;
/// Cartridge for the v2 five-term binding.
pub type ContinuationLeaseCartridge = ContinuationCartridgeRecord<JournalLeaseBinding>;

impl<B: JournalBindingLike> ContinuationCartridgeRecord<B> {
    fn new(binding: &B, prepared_record_digest: Sha256Digest) -> Result<Self, JournalError> {
        Ok(Self {
            schema_version: DURABLE_RECEIPT_SCHEMA_VERSION,
            binding_digest: binding.binding_digest()?,
            prepared_record_digest,
            transaction_id: binding.transaction_id(),
            old_root: binding.old_root(),
            new_root: binding.new_root(),
            durable_profile_digest: binding.durable_profile_digest(),
            owner_identity_digest: binding.owner_identity_digest(),
            _binding: std::marker::PhantomData,
        })
    }
    pub fn validate_against(&self, binding: &B) -> Result<(), JournalError> {
        if self.schema_version != DURABLE_RECEIPT_SCHEMA_VERSION
            || self.binding_digest != binding.binding_digest()?
            || self.transaction_id != binding.transaction_id()
            || self.old_root != binding.old_root()
            || self.new_root != binding.new_root()
            || self.durable_profile_digest != binding.durable_profile_digest()
            || self.owner_identity_digest != binding.owner_identity_digest()
            || self.prepared_record_digest == Sha256Digest::ZERO
        {
            return Err(JournalError::new(
                JournalFailureCode::CartridgeMismatch,
                "continuation cartridge does not bind the journal transaction",
            ));
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, JournalError> {
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<Sha256Digest, JournalError> {
        Ok(domain_digest(CARTRIDGE_DOMAIN, &self.canonical_bytes()?))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerDeathReceipt {
    pub schema_version: u16,
    pub binding_digest: Sha256Digest,
    pub prepared_record_digest: Sha256Digest,
    pub owner_identity_digest: Sha256Digest,
    pub observed_journal_state: JournalState,
    pub observed_root: Sha256Digest,
    pub observed_at_unix_ns: u64,
    pub failure_code: JournalFailureCode,
    pub recovery_required: bool,
}
impl OwnerDeathReceipt {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, JournalError> {
        if self.schema_version != DURABLE_RECEIPT_SCHEMA_VERSION
            || self.failure_code != JournalFailureCode::OwnerDeath
            || !self.recovery_required
            || self.binding_digest == Sha256Digest::ZERO
            || self.prepared_record_digest == Sha256Digest::ZERO
            || self.owner_identity_digest == Sha256Digest::ZERO
            || self.observed_root == Sha256Digest::ZERO
        {
            return Err(JournalError::new(
                JournalFailureCode::InvalidBinding,
                "owner-death receipt is incomplete",
            ));
        }
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<Sha256Digest, JournalError> {
        Ok(domain_digest(OWNER_DEATH_DOMAIN, &self.canonical_bytes()?))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryReceipt {
    pub schema_version: u16,
    pub binding_digest: Sha256Digest,
    pub prepared_record_digest: Sha256Digest,
    pub terminal_record_digest: Option<Sha256Digest>,
    pub owner_death_receipt_digest: Option<Sha256Digest>,
    pub observed_root: Sha256Digest,
    pub outcome: RecoveryOutcome,
    pub journal_root_correspondence: bool,
    pub promotable: bool,
    pub failure_code: Option<JournalFailureCode>,
}
impl RecoveryReceipt {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, JournalError> {
        if self.schema_version != DURABLE_RECEIPT_SCHEMA_VERSION
            || self.binding_digest == Sha256Digest::ZERO
            || self.observed_root == Sha256Digest::ZERO
            || !self.journal_root_correspondence
            || !self.promotable
            || self.failure_code.is_some()
        {
            return Err(JournalError::new(
                JournalFailureCode::InvalidBinding,
                "successful recovery receipt is incomplete or non-promotable",
            ));
        }
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<Sha256Digest, JournalError> {
        Ok(domain_digest(RECOVERY_DOMAIN, &self.canonical_bytes()?))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FaultPlan {
    crash_at: Option<JournalBoundary>,
    fired: bool,
}
impl FaultPlan {
    pub const fn none() -> Self {
        Self {
            crash_at: None,
            fired: false,
        }
    }
    pub const fn crash_at(boundary: JournalBoundary) -> Self {
        Self {
            crash_at: Some(boundary),
            fired: false,
        }
    }
    fn hit(&mut self, boundary: JournalBoundary, changed: bool) -> Result<(), JournalError> {
        if !self.fired && self.crash_at == Some(boundary) {
            self.fired = true;
            return Err(JournalError::at(
                JournalFailureCode::InjectedCrash,
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
    before: JournalBoundary,
    file_sync: JournalBoundary,
    rename: JournalBoundary,
    dir_sync: JournalBoundary,
}
const ROOT_INIT: WriteBoundaries = WriteBoundaries {
    before: JournalBoundary::RootInitializeBeforeWrite,
    file_sync: JournalBoundary::RootInitializeAfterFileSync,
    rename: JournalBoundary::RootInitializeAfterRename,
    dir_sync: JournalBoundary::RootInitializeAfterDirectorySync,
};
const CARTRIDGE: WriteBoundaries = WriteBoundaries {
    before: JournalBoundary::CartridgeBeforeWrite,
    file_sync: JournalBoundary::CartridgeAfterFileSync,
    rename: JournalBoundary::CartridgeAfterRename,
    dir_sync: JournalBoundary::CartridgeAfterDirectorySync,
};
const PREPARE: WriteBoundaries = WriteBoundaries {
    before: JournalBoundary::PrepareBeforeWrite,
    file_sync: JournalBoundary::PrepareAfterFileSync,
    rename: JournalBoundary::PrepareAfterRename,
    dir_sync: JournalBoundary::PrepareAfterDirectorySync,
};
const ROOT_PUBLISH: WriteBoundaries = WriteBoundaries {
    before: JournalBoundary::RootPublishBeforeWrite,
    file_sync: JournalBoundary::RootPublishAfterFileSync,
    rename: JournalBoundary::RootPublishAfterRename,
    dir_sync: JournalBoundary::RootPublishAfterDirectorySync,
};
const COMMIT: WriteBoundaries = WriteBoundaries {
    before: JournalBoundary::CommitBeforeWrite,
    file_sync: JournalBoundary::CommitAfterFileSync,
    rename: JournalBoundary::CommitAfterRename,
    dir_sync: JournalBoundary::CommitAfterDirectorySync,
};
const ABORT: WriteBoundaries = WriteBoundaries {
    before: JournalBoundary::AbortBeforeWrite,
    file_sync: JournalBoundary::AbortAfterFileSync,
    rename: JournalBoundary::AbortAfterRename,
    dir_sync: JournalBoundary::AbortAfterDirectorySync,
};
const OWNER_DEATH: WriteBoundaries = WriteBoundaries {
    before: JournalBoundary::OwnerDeathBeforeWrite,
    file_sync: JournalBoundary::OwnerDeathAfterFileSync,
    rename: JournalBoundary::OwnerDeathAfterRename,
    dir_sync: JournalBoundary::OwnerDeathAfterDirectorySync,
};
const RECOVERY: WriteBoundaries = WriteBoundaries {
    before: JournalBoundary::RecoveryBeforeWrite,
    file_sync: JournalBoundary::RecoveryAfterFileSync,
    rename: JournalBoundary::RecoveryAfterRename,
    dir_sync: JournalBoundary::RecoveryAfterDirectorySync,
};

pub fn initialize_published_root(
    paths: &JournalPaths,
    root: Sha256Digest,
) -> Result<RootPublicationReceipt, JournalError> {
    initialize_published_root_with_fault(paths, root, &mut FaultPlan::none())
}
pub fn initialize_published_root_with_fault(
    paths: &JournalPaths,
    root: Sha256Digest,
    fault: &mut FaultPlan,
) -> Result<RootPublicationReceipt, JournalError> {
    let expected = RootPublicationReceipt::initial(root)?;
    if let Some(existing) = read_optional::<RootPublicationReceipt>(
        paths.root_record(),
        JournalFailureCode::RootMissing,
        ROOT_DOMAIN,
    )? {
        existing.value.validate()?;
        if existing.value == expected {
            return Ok(existing.value);
        }
        return Err(JournalError::new(
            JournalFailureCode::RootMismatch,
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

pub fn prepare_journal(
    paths: &JournalPaths,
    binding: JournalBinding,
) -> Result<ContinuationCartridge, JournalError> {
    prepare_journal_with_fault(paths, binding, &mut FaultPlan::none())
}
pub fn prepare_journal_with_fault(
    paths: &JournalPaths,
    binding: JournalBinding,
    fault: &mut FaultPlan,
) -> Result<ContinuationCartridge, JournalError> {
    prepare_bound_journal(paths, binding, fault)
}
/// Five-term commit surface: prepare a journal transaction whose binding
/// carries nonce, protected scope, and lease alongside the root/epoch pair
/// and the session/ledger owner identity.
pub fn prepare_lease_journal(
    paths: &JournalPaths,
    binding: JournalLeaseBinding,
) -> Result<ContinuationLeaseCartridge, JournalError> {
    prepare_bound_journal(paths, binding, &mut FaultPlan::none())
}
pub fn prepare_lease_journal_with_fault(
    paths: &JournalPaths,
    binding: JournalLeaseBinding,
    fault: &mut FaultPlan,
) -> Result<ContinuationLeaseCartridge, JournalError> {
    prepare_bound_journal(paths, binding, fault)
}
fn prepare_bound_journal<B: JournalBindingLike>(
    paths: &JournalPaths,
    binding: B,
    fault: &mut FaultPlan,
) -> Result<ContinuationCartridgeRecord<B>, JournalError> {
    binding.binding_validate()?;
    if let Some(expires_at_unix_ns) = binding.lease_expires_at_unix_ns()
        && now_unix_ns() >= expires_at_unix_ns
    {
        return Err(JournalError::new(
            JournalFailureCode::LeaseExpired,
            "binding lease has expired before prepare",
        ));
    }
    // The commit surface is a compare-and-swap on the parent root: the
    // preregistered old root must still be current before anything else is
    // consulted, so a second writer observes RootMismatch (never a terminal
    // journal confusion) when the first writer moved the root.
    let root = read_root(paths)?;
    if root.root_digest != binding.old_root() {
        return Err(JournalError::new(
            JournalFailureCode::RootMismatch,
            "prepare requires the preregistered old root",
        ));
    }
    if read_optional::<RecoveryReceipt>(
        paths.recovery_receipt(),
        JournalFailureCode::JournalMissing,
        RECOVERY_DOMAIN,
    )?
    .is_some()
    {
        return Err(JournalError::new(
            JournalFailureCode::AlreadyTerminal,
            "recovered transaction cannot be prepared again",
        ));
    }
    if let Some(existing) = read_optional::<DurableJournalRecord<B>>(
        paths.journal_record(),
        JournalFailureCode::JournalMissing,
        B::record_domain(),
    )? {
        existing.value.validate()?;
        if existing.value.state == JournalState::Prepared && existing.value.binding == binding {
            let cartridge = read_cartridge::<B>(paths)?;
            cartridge.validate_against(&binding)?;
            if cartridge.prepared_record_digest != existing.digest {
                return Err(JournalError::new(
                    JournalFailureCode::CartridgeMismatch,
                    "cartridge does not bind the persisted prepared record",
                ));
            }
            return Ok(cartridge);
        }
        return Err(JournalError::new(
            JournalFailureCode::AlreadyTerminal,
            "another or terminal transaction already occupies this journal",
        ));
    }
    let prepared = DurableJournalRecord::<B>::prepared(binding.clone());
    let prepared_digest = prepared.digest()?;
    let cartridge = ContinuationCartridgeRecord::new(&binding, prepared_digest)?;
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

pub fn commit_journal(
    paths: &JournalPaths,
    cartridge: &ContinuationCartridge,
) -> Result<RecoveryReceipt, JournalError> {
    commit_journal_with_fault(paths, cartridge, &mut FaultPlan::none())
}
pub fn commit_journal_with_fault(
    paths: &JournalPaths,
    cartridge: &ContinuationCartridge,
    fault: &mut FaultPlan,
) -> Result<RecoveryReceipt, JournalError> {
    commit_bound_journal(paths, cartridge, fault)
}
/// Five-term commit surface: publish the new root only while the persisted
/// root still equals the binding's parent root (exact compare-and-swap).
pub fn commit_lease_journal(
    paths: &JournalPaths,
    cartridge: &ContinuationLeaseCartridge,
) -> Result<RecoveryReceipt, JournalError> {
    commit_bound_journal(paths, cartridge, &mut FaultPlan::none())
}
pub fn commit_lease_journal_with_fault(
    paths: &JournalPaths,
    cartridge: &ContinuationLeaseCartridge,
    fault: &mut FaultPlan,
) -> Result<RecoveryReceipt, JournalError> {
    commit_bound_journal(paths, cartridge, fault)
}
fn commit_bound_journal<B: JournalBindingLike>(
    paths: &JournalPaths,
    cartridge: &ContinuationCartridgeRecord<B>,
    fault: &mut FaultPlan,
) -> Result<RecoveryReceipt, JournalError> {
    if let Some(existing) = existing_recovery::<B>(paths, None)? {
        if existing.binding_digest == cartridge.binding_digest {
            return Ok(existing);
        }
        return Err(JournalError::new(
            JournalFailureCode::ImmutableReceiptConflict,
            "recovery receipt and continuation cartridge bindings differ",
        ));
    }
    let journal = read_journal::<B>(paths)?;
    journal.value.validate()?;
    cartridge.validate_against(&journal.value.binding)?;
    let binding = &journal.value.binding;
    match journal.value.state {
        JournalState::Aborted => {
            return Err(JournalError::new(
                JournalFailureCode::AlreadyTerminal,
                "aborted journal cannot commit",
            ));
        }
        JournalState::Committed => {
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
                    RecoveryOutcome::AlreadyCommitted,
                    owner_death_digest::<B>(paths, binding)?,
                )?,
                fault,
            );
        }
        JournalState::Prepared => {}
    }
    if journal.digest != cartridge.prepared_record_digest {
        return Err(JournalError::new(
            JournalFailureCode::CartridgeMismatch,
            "prepared record digest differs from continuation cartridge",
        ));
    }
    // Explicit authority: Prepared -> Committed is the single commit path.
    // Any other state was rejected above. This transition is fail-closed via
    // domain digest binding checks (cartridge vs journal, root vs binding).
    debug_assert!(
        journal
            .value
            .state
            .can_transition_to(JournalState::Committed)
    );
    let root = read_root(paths)?;
    if root.root_digest == binding.old_root() {
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
    let committed = DurableJournalRecord::<B>::committed(&journal.value, journal.digest);
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
            binding.new_root(),
            RecoveryOutcome::NewRootCommitted,
            owner_death_digest::<B>(paths, binding)?,
        )?,
        fault,
    )
}

pub fn abort_journal(
    paths: &JournalPaths,
    cartridge: &ContinuationCartridge,
) -> Result<RecoveryReceipt, JournalError> {
    abort_journal_with_fault(paths, cartridge, &mut FaultPlan::none())
}
pub fn abort_journal_with_fault(
    paths: &JournalPaths,
    cartridge: &ContinuationCartridge,
    fault: &mut FaultPlan,
) -> Result<RecoveryReceipt, JournalError> {
    abort_bound_journal(paths, cartridge, fault)
}
/// Five-term commit surface: abort refuses once the root has moved past the
/// binding's parent root, so a consumed lease can never abort a successor.
pub fn abort_lease_journal(
    paths: &JournalPaths,
    cartridge: &ContinuationLeaseCartridge,
) -> Result<RecoveryReceipt, JournalError> {
    abort_bound_journal(paths, cartridge, &mut FaultPlan::none())
}
pub fn abort_lease_journal_with_fault(
    paths: &JournalPaths,
    cartridge: &ContinuationLeaseCartridge,
    fault: &mut FaultPlan,
) -> Result<RecoveryReceipt, JournalError> {
    abort_bound_journal(paths, cartridge, fault)
}
fn abort_bound_journal<B: JournalBindingLike>(
    paths: &JournalPaths,
    cartridge: &ContinuationCartridgeRecord<B>,
    fault: &mut FaultPlan,
) -> Result<RecoveryReceipt, JournalError> {
    if let Some(existing) = existing_recovery::<B>(paths, None)? {
        if existing.binding_digest == cartridge.binding_digest {
            return Ok(existing);
        }
        return Err(JournalError::new(
            JournalFailureCode::ImmutableReceiptConflict,
            "recovery receipt and continuation cartridge bindings differ",
        ));
    }
    let journal = read_journal::<B>(paths)?;
    journal.value.validate()?;
    cartridge.validate_against(&journal.value.binding)?;
    let binding = &journal.value.binding;
    match journal.value.state {
        JournalState::Committed => {
            return Err(JournalError::new(
                JournalFailureCode::AlreadyTerminal,
                "committed journal cannot abort",
            ));
        }
        JournalState::Aborted => {
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
                    RecoveryOutcome::AlreadyAborted,
                    owner_death_digest::<B>(paths, binding)?,
                )?,
                fault,
            );
        }
        JournalState::Prepared => {}
    }
    if journal.digest != cartridge.prepared_record_digest {
        return Err(JournalError::new(
            JournalFailureCode::CartridgeMismatch,
            "prepared record digest differs from continuation cartridge",
        ));
    }
    // Explicit authority: Prepared -> Aborted is the single abort path.
    debug_assert!(journal.value.state.can_transition_to(JournalState::Aborted));
    let root = read_root(paths)?;
    verify_old(&root, binding)?;
    let aborted = DurableJournalRecord::<B>::aborted(
        &journal.value,
        journal.digest,
        AbortReason::ExplicitAbort,
    );
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
            binding.old_root(),
            RecoveryOutcome::OldRootAborted,
            owner_death_digest::<B>(paths, binding)?,
        )?,
        fault,
    )
}

pub fn record_owner_death(
    paths: &JournalPaths,
    owner: Sha256Digest,
    observed_at_unix_ns: u64,
) -> Result<OwnerDeathReceipt, JournalError> {
    record_owner_death_with_fault(paths, owner, observed_at_unix_ns, &mut FaultPlan::none())
}
pub fn record_owner_death_with_fault(
    paths: &JournalPaths,
    owner: Sha256Digest,
    observed_at_unix_ns: u64,
    fault: &mut FaultPlan,
) -> Result<OwnerDeathReceipt, JournalError> {
    record_bound_owner_death::<JournalBinding>(paths, owner, observed_at_unix_ns, fault)
}
pub fn record_lease_owner_death(
    paths: &JournalPaths,
    owner: Sha256Digest,
    observed_at_unix_ns: u64,
) -> Result<OwnerDeathReceipt, JournalError> {
    record_lease_owner_death_with_fault(paths, owner, observed_at_unix_ns, &mut FaultPlan::none())
}
pub fn record_lease_owner_death_with_fault(
    paths: &JournalPaths,
    owner: Sha256Digest,
    observed_at_unix_ns: u64,
    fault: &mut FaultPlan,
) -> Result<OwnerDeathReceipt, JournalError> {
    record_bound_owner_death::<JournalLeaseBinding>(paths, owner, observed_at_unix_ns, fault)
}
fn record_bound_owner_death<B: JournalBindingLike>(
    paths: &JournalPaths,
    owner: Sha256Digest,
    observed_at_unix_ns: u64,
    fault: &mut FaultPlan,
) -> Result<OwnerDeathReceipt, JournalError> {
    let journal = read_journal::<B>(paths)?;
    journal.value.validate()?;
    if journal.value.binding.owner_identity_digest() != owner {
        return Err(JournalError::new(
            JournalFailureCode::OwnerIdentityMismatch,
            "owner-death identity differs from prepared binding",
        ));
    }
    if journal.value.state != JournalState::Prepared {
        return Err(JournalError::new(
            JournalFailureCode::AlreadyTerminal,
            "terminal journal does not require owner-death recovery",
        ));
    }
    let root = read_root(paths)?;
    let receipt = OwnerDeathReceipt {
        schema_version: DURABLE_RECEIPT_SCHEMA_VERSION,
        binding_digest: journal.value.binding.binding_digest()?,
        prepared_record_digest: journal.digest,
        owner_identity_digest: owner,
        observed_journal_state: journal.value.state,
        observed_root: root.root_digest,
        observed_at_unix_ns,
        failure_code: JournalFailureCode::OwnerDeath,
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

pub fn recover_journal(
    paths: &JournalPaths,
    expected: &JournalBinding,
) -> Result<RecoveryReceipt, JournalError> {
    recover_journal_with_fault(paths, expected, &mut FaultPlan::none())
}
pub fn recover_journal_with_fault(
    paths: &JournalPaths,
    expected: &JournalBinding,
    fault: &mut FaultPlan,
) -> Result<RecoveryReceipt, JournalError> {
    recover_bound_journal(paths, expected, fault)
}
/// Five-term commit surface: recovery completes a prepared five-term
/// transaction (abort on the old root, commit on the new root) and refuses
/// every other journal/root pairing loudly.
pub fn recover_lease_journal(
    paths: &JournalPaths,
    expected: &JournalLeaseBinding,
) -> Result<RecoveryReceipt, JournalError> {
    recover_bound_journal(paths, expected, &mut FaultPlan::none())
}
pub fn recover_lease_journal_with_fault(
    paths: &JournalPaths,
    expected: &JournalLeaseBinding,
    fault: &mut FaultPlan,
) -> Result<RecoveryReceipt, JournalError> {
    recover_bound_journal(paths, expected, fault)
}
fn recover_bound_journal<B: JournalBindingLike>(
    paths: &JournalPaths,
    expected: &B,
    fault: &mut FaultPlan,
) -> Result<RecoveryReceipt, JournalError> {
    expected.binding_validate()?;
    if let Some(existing) = existing_recovery(paths, Some(expected))? {
        return Ok(existing);
    }
    let root = read_root(paths)?;
    let Some(journal) = read_optional::<DurableJournalRecord<B>>(
        paths.journal_record(),
        JournalFailureCode::JournalMissing,
        B::record_domain(),
    )?
    else {
        if root.root_digest != expected.old_root() {
            return Err(disagreement("missing journal accompanies a non-old root"));
        }
        let Some(cartridge) = read_optional::<ContinuationCartridgeRecord<B>>(
            paths.cartridge(),
            JournalFailureCode::CartridgeMismatch,
            CARTRIDGE_DOMAIN,
        )?
        else {
            return persist_recovery(
                paths,
                make_recovery(
                    expected,
                    Sha256Digest::ZERO,
                    None,
                    expected.old_root(),
                    RecoveryOutcome::NotStartedOldRoot,
                    None,
                )?,
                fault,
            );
        };
        cartridge.value.validate_against(expected)?;
        let prepared = DurableJournalRecord::<B>::prepared(expected.clone());
        let prepared_digest = prepared.digest()?;
        if cartridge.value.prepared_record_digest != prepared_digest {
            return Err(JournalError::new(
                JournalFailureCode::CartridgeMismatch,
                "continuation cartridge does not bind the reconstructable prepared record",
            ));
        }
        let aborted = DurableJournalRecord::<B>::aborted(
            &prepared,
            prepared_digest,
            AbortReason::RecoveryObservedOldRoot,
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
                expected.old_root(),
                RecoveryOutcome::OldRootAborted,
                None,
            )?,
            fault,
        );
    };
    journal.value.validate()?;
    if journal.value.binding.binding_digest()? != expected.binding_digest()? {
        return Err(JournalError::new(
            JournalFailureCode::InvalidBinding,
            "persisted journal binding differs from recovery expectation",
        ));
    }
    let prepared = prepared_digest(&journal.value, journal.digest)?;
    let owner_death = owner_death_digest::<B>(paths, expected)?;
    match journal.value.state {
        JournalState::Prepared if root.root_digest == expected.old_root() => {
            let reason = if owner_death.is_some() {
                AbortReason::OwnerDeathObservedOldRoot
            } else {
                AbortReason::RecoveryObservedOldRoot
            };
            let aborted =
                DurableJournalRecord::<B>::aborted(&journal.value, journal.digest, reason);
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
                    expected.old_root(),
                    RecoveryOutcome::OldRootAborted,
                    owner_death,
                )?,
                fault,
            )
        }
        JournalState::Prepared if root.root_digest == expected.new_root() => {
            verify_new(&root, expected, prepared)?;
            let committed = DurableJournalRecord::<B>::committed(&journal.value, journal.digest);
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
                    expected.new_root(),
                    RecoveryOutcome::NewRootCommitted,
                    owner_death,
                )?,
                fault,
            )
        }
        JournalState::Prepared => Err(disagreement(
            "prepared journal accompanies neither preregistered root",
        )),
        JournalState::Committed => {
            verify_new(&root, expected, prepared)?;
            persist_recovery(
                paths,
                make_recovery(
                    expected,
                    prepared,
                    Some(journal.digest),
                    expected.new_root(),
                    RecoveryOutcome::AlreadyCommitted,
                    owner_death,
                )?,
                fault,
            )
        }
        JournalState::Aborted => {
            verify_old(&root, expected)?;
            persist_recovery(
                paths,
                make_recovery(
                    expected,
                    prepared,
                    Some(journal.digest),
                    expected.old_root(),
                    RecoveryOutcome::AlreadyAborted,
                    owner_death,
                )?,
                fault,
            )
        }
    }
}

pub fn read_published_root(paths: &JournalPaths) -> Result<RootPublicationReceipt, JournalError> {
    read_root(paths)
}
pub fn read_journal_record(paths: &JournalPaths) -> Result<DurableJournal, JournalError> {
    Ok(read_journal::<JournalBinding>(paths)?.value)
}
pub fn read_continuation_cartridge(
    paths: &JournalPaths,
) -> Result<ContinuationCartridge, JournalError> {
    read_cartridge::<JournalBinding>(paths)
}
/// Read the committed five-term journal record. The returned record carries
/// the full provenance binding (roots, session/ledger owner, nonce, protected
/// scope, lease) exactly as it was committed.
pub fn read_lease_journal_record(
    paths: &JournalPaths,
) -> Result<DurableLeaseJournal, JournalError> {
    Ok(read_journal::<JournalLeaseBinding>(paths)?.value)
}
pub fn read_lease_continuation_cartridge(
    paths: &JournalPaths,
) -> Result<ContinuationLeaseCartridge, JournalError> {
    read_cartridge::<JournalLeaseBinding>(paths)
}
/// Read-side verification of a committed five-term binding: the persisted
/// record and its recovery receipt must correspond to the expected session /
/// ledger identity and carry the same binding digest, or the read fails
/// loudly. This is how reads verify the provenance binding of a commit.
pub fn verify_committed_lease_binding(
    paths: &JournalPaths,
    expected: &JournalLeaseBinding,
) -> Result<RecoveryReceipt, JournalError> {
    expected.binding_validate()?;
    let journal = read_journal::<JournalLeaseBinding>(paths)?;
    journal.value.validate()?;
    if journal.value.binding.binding_digest()? != expected.binding_digest()? {
        return Err(JournalError::new(
            JournalFailureCode::InvalidBinding,
            "committed record does not bind the expected identity",
        ));
    }
    if journal.value.state != JournalState::Committed {
        return Err(JournalError::new(
            JournalFailureCode::AlreadyTerminal,
            "verify_committed_binding requires a committed journal record",
        ));
    }
    existing_recovery::<JournalLeaseBinding>(paths, Some(expected))?.ok_or_else(|| {
        JournalError::new(
            JournalFailureCode::JournalMissing,
            "committed record lacks a recovery receipt",
        )
    })
}

fn make_recovery<B: JournalBindingLike>(
    binding: &B,
    prepared: Sha256Digest,
    terminal: Option<Sha256Digest>,
    root: Sha256Digest,
    outcome: RecoveryOutcome,
    owner: Option<Sha256Digest>,
) -> Result<RecoveryReceipt, JournalError> {
    Ok(RecoveryReceipt {
        schema_version: DURABLE_RECEIPT_SCHEMA_VERSION,
        binding_digest: binding.binding_digest()?,
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
fn existing_recovery<B: JournalBindingLike>(
    paths: &JournalPaths,
    expected: Option<&B>,
) -> Result<Option<RecoveryReceipt>, JournalError> {
    let Some(read) = read_optional::<RecoveryReceipt>(
        paths.recovery_receipt(),
        JournalFailureCode::JournalMissing,
        RECOVERY_DOMAIN,
    )?
    else {
        return Ok(None);
    };
    read.value.canonical_bytes()?;
    if let Some(binding) = expected
        && read.value.binding_digest != binding.binding_digest()?
    {
        return Err(JournalError::new(
            JournalFailureCode::ImmutableReceiptConflict,
            "existing recovery receipt belongs to another binding",
        ));
    }
    Ok(Some(read.value))
}
fn persist_recovery(
    paths: &JournalPaths,
    receipt: RecoveryReceipt,
    fault: &mut FaultPlan,
) -> Result<RecoveryReceipt, JournalError> {
    // Recovery receipt is immutable and typed: same path for v1 and v2, but
    // the binding digest is the authority. Generic check ensures we do not
    // silently alias a v1 receipt for a v2 binding.
    if let Some(existing) = read_optional::<RecoveryReceipt>(
        paths.recovery_receipt(),
        JournalFailureCode::JournalMissing,
        RECOVERY_DOMAIN,
    )? {
        existing.value.canonical_bytes()?;
        if existing.value.binding_digest == receipt.binding_digest {
            return Ok(existing.value);
        }
        return Err(JournalError::new(
            JournalFailureCode::ImmutableReceiptConflict,
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
fn owner_death_digest<B: JournalBindingLike>(
    paths: &JournalPaths,
    binding: &B,
) -> Result<Option<Sha256Digest>, JournalError> {
    let Some(read) = read_optional::<OwnerDeathReceipt>(
        paths.owner_death_receipt(),
        JournalFailureCode::JournalMissing,
        OWNER_DEATH_DOMAIN,
    )?
    else {
        return Ok(None);
    };
    read.value.canonical_bytes()?;
    if read.value.binding_digest != binding.binding_digest()?
        || read.value.owner_identity_digest != binding.owner_identity_digest()
    {
        return Err(JournalError::new(
            JournalFailureCode::OwnerIdentityMismatch,
            "owner-death receipt differs from journal binding",
        ));
    }
    Ok(Some(read.digest))
}
fn verify_old<B: JournalBindingLike>(
    root: &RootPublicationReceipt,
    binding: &B,
) -> Result<(), JournalError> {
    root.validate()?;
    if root.root_digest == binding.old_root() {
        Ok(())
    } else {
        Err(disagreement("aborted journal does not accompany old root"))
    }
}
fn verify_new<B: JournalBindingLike>(
    root: &RootPublicationReceipt,
    binding: &B,
    prepared: Sha256Digest,
) -> Result<(), JournalError> {
    root.validate()?;
    if root.root_digest == binding.new_root()
        && root.transaction_id == binding.transaction_id()
        && root.prepared_record_digest == prepared
    {
        Ok(())
    } else {
        Err(disagreement(
            "committed journal does not accompany its bound new root",
        ))
    }
}
fn prepared_digest<B: JournalBindingLike>(
    record: &DurableJournalRecord<B>,
    digest: Sha256Digest,
) -> Result<Sha256Digest, JournalError> {
    match record.state {
        JournalState::Prepared => Ok(digest),
        _ => record.predecessor_record_digest.ok_or_else(|| {
            JournalError::new(
                JournalFailureCode::SequenceMismatch,
                "terminal journal lacks prepared predecessor",
            )
        }),
    }
}
fn disagreement(detail: &str) -> JournalError {
    JournalError::new(JournalFailureCode::JournalRootDisagreement, detail)
}

struct CanonicalRead<T> {
    value: T,
    digest: Sha256Digest,
}
fn read_root(paths: &JournalPaths) -> Result<RootPublicationReceipt, JournalError> {
    let read = read_canonical::<RootPublicationReceipt>(
        paths.root_record(),
        JournalFailureCode::RootMissing,
        ROOT_DOMAIN,
    )?;
    read.value.validate()?;
    Ok(read.value)
}
fn read_journal<B: JournalBindingLike>(
    paths: &JournalPaths,
) -> Result<CanonicalRead<DurableJournalRecord<B>>, JournalError> {
    read_canonical(
        paths.journal_record(),
        JournalFailureCode::JournalMissing,
        B::record_domain(),
    )
}
fn read_cartridge<B: JournalBindingLike>(
    paths: &JournalPaths,
) -> Result<ContinuationCartridgeRecord<B>, JournalError> {
    Ok(read_canonical::<ContinuationCartridgeRecord<B>>(
        paths.cartridge(),
        JournalFailureCode::CartridgeMismatch,
        CARTRIDGE_DOMAIN,
    )?
    .value)
}
fn read_optional<T>(
    path: &Path,
    missing: JournalFailureCode,
    domain: &'static [u8],
) -> Result<Option<CanonicalRead<T>>, JournalError>
where
    T: DeserializeOwned + Serialize,
{
    match read_canonical(path, missing, domain) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.code == missing => Ok(None),
        Err(error) => Err(error),
    }
}
fn read_canonical<T>(
    path: &Path,
    missing: JournalFailureCode,
    domain: &'static [u8],
) -> Result<CanonicalRead<T>, JournalError>
where
    T: DeserializeOwned + Serialize,
{
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(JournalError::new(missing, "required record is absent"));
        }
        Err(error) => {
            return Err(JournalError::new(
                JournalFailureCode::IoBeforePublish,
                format!("record stat failed: {error}"),
            ));
        }
    };
    if !metadata.file_type().is_file() {
        return Err(JournalError::new(
            JournalFailureCode::TornOrNoncanonicalRecord,
            "record is not a regular file",
        ));
    }
    if metadata.len() > DURABLE_JOURNAL_MAX_RECORD_BYTES {
        return Err(JournalError::new(
            JournalFailureCode::RecordTooLarge,
            "record exceeds the frozen byte bound",
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        JournalError::new(
            JournalFailureCode::IoBeforePublish,
            format!("record read failed: {error}"),
        )
    })?;
    let value: T = serde_json::from_slice(&bytes).map_err(|error| {
        JournalError::new(
            JournalFailureCode::TornOrNoncanonicalRecord,
            format!("record decode failed: {error}"),
        )
    })?;
    let canonical = canonical_bytes(&value)?;
    if canonical != bytes {
        return Err(JournalError::new(
            JournalFailureCode::TornOrNoncanonicalRecord,
            "record bytes are not canonical JSON",
        ));
    }
    Ok(CanonicalRead {
        value,
        digest: domain_digest(domain, &canonical),
    })
}
fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, JournalError> {
    let value = serde_json::to_value(value).map_err(|error| {
        JournalError::new(
            JournalFailureCode::InvalidBinding,
            format!("record serialization failed: {error}"),
        )
    })?;
    let bytes = canonical_json(&value).into_bytes();
    if bytes.len() as u64 > DURABLE_JOURNAL_MAX_RECORD_BYTES {
        return Err(JournalError::new(
            JournalFailureCode::RecordTooLarge,
            "canonical record exceeds the frozen byte bound",
        ));
    }
    Ok(bytes)
}
fn domain_digest(domain: &[u8], bytes: &[u8]) -> Sha256Digest {
    let mut bound = Vec::with_capacity(domain.len() + 8 + bytes.len());
    bound.extend_from_slice(domain);
    bound.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    bound.extend_from_slice(bytes);
    Sha256Digest::from_bytes(sha256(&bound))
}
fn write_once(
    path: &Path,
    bytes: &[u8],
    boundaries: WriteBoundaries,
    fault: &mut FaultPlan,
) -> Result<(), JournalError> {
    match fs::read(path) {
        Ok(existing) if existing == bytes => return Ok(()),
        Ok(_) => {
            return Err(JournalError::new(
                JournalFailureCode::ImmutableReceiptConflict,
                "immutable record already exists with different bytes",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(JournalError::new(
                JournalFailureCode::IoBeforePublish,
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
    fault: &mut FaultPlan,
) -> Result<(), JournalError> {
    if bytes.len() as u64 > DURABLE_JOURNAL_MAX_RECORD_BYTES {
        return Err(JournalError::new(
            JournalFailureCode::RecordTooLarge,
            "write exceeds the frozen record byte bound",
        ));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| io_before(boundaries.before, error))?;
    let file_name = path.file_name().ok_or_else(|| {
        JournalError::new(
            JournalFailureCode::InvalidBinding,
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
            JournalError::at(
                JournalFailureCode::DirectorySyncFailedAfterPublish,
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
fn now_unix_ns() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
}
fn io_before(boundary: JournalBoundary, error: io::Error) -> JournalError {
    JournalError::at(
        JournalFailureCode::IoBeforePublish,
        boundary,
        false,
        format!("durable write failed before publication: {error}"),
    )
}

/// Machine-readable contract summary used by conformance generators.
pub fn durable_journal_contract() -> serde_json::Value {
    json!({
        "schema_version": DURABLE_JOURNAL_SCHEMA_VERSION,
        "journal_schema_version": DURABLE_JOURNAL_SCHEMA_VERSION,
        "lease_journal_schema_version": DURABLE_LEASE_JOURNAL_SCHEMA_VERSION,
        "binding_schema_version": DURABLE_BINDING_SCHEMA_VERSION,
        "lease_binding_schema_version": DURABLE_LEASE_BINDING_SCHEMA_VERSION,
        "lease_schema_version": DURABLE_LEASE_SCHEMA_VERSION,
        "five_term_binding": ["old_root", "new_root", "transaction_id",
            "owner_identity_digest", "nonce", "protected_scope", "lease"],
        "receipt_schema_version": DURABLE_RECEIPT_SCHEMA_VERSION,
        "max_record_bytes": DURABLE_JOURNAL_MAX_RECORD_BYTES,
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
            "journal_root_disagreement", "already_terminal", "lease_expired",
            "immutable_receipt_conflict", "io_before_publish",
            "directory_sync_failed_after_publish", "injected_crash", "owner_death",
            "indeterminate"]
    })
}
