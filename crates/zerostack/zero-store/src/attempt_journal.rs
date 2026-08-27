//! Durable per-effect attempt journal with an explicit dispatch boundary.
//!
//! One attempt journal tracks a single effect attempt across admission,
//! dispatch, and terminal outcome. It is deliberately separate from
//! [crate::durable_journal], which journals multi-record publication
//! transactions: attempts are admitted, crossed, and resolved one at a time,
//! and the recovery law is stricter.
//!
//! State machine (every edge is a persisted entry):
//!
//! ```text
//! Prepared ──dispatch──▶ DispatchCrossed ──▶ Succeeded | Failed | Indeterminate
//! Prepared ──abort─────▶ Aborted            (explicit user abort only)
//! Prepared ──retry─────▶ SafeToRetry        (recovery: never dispatched ⇒ safe to retry)
//! ```
//!
//! The caller owns the ordering contract:
//!
//! 1. `prepare_attempt` persists the write-once Prepared entry.
//! 2. Effect admission happens only after prepare returns.
//! 3. `mark_dispatch_crossed` persists the dispatch boundary immediately
//!    before the effect is dispatched.
//! 4. The effect runs; the caller persists `mark_succeeded`,
//!    `mark_failed`, or `mark_indeterminate`, or crashes.
//!
//! Entries are immutable: each sequence number is a distinct write-once file
//! (`attempt-<sequence>.json`) inside the caller-supplied directory, so a
//! terminal entry can never be replaced. Terminal entries carry canonical
//! evidence: completion receipts, failure receipts, or an abort reason.
//! Receipt bodies live in higher layers — zero-gate depends on zero-store, so
//! this crate cannot name gate receipt types — and the journal binds their
//! canonical digests, which are authoritative once persisted.
//!
//! Recovery law (`recover_attempt`):
//!
//! - Succeeded | Failed | Indeterminate | Aborted | SafeToRetry: returned
//!   unchanged.
//! - Prepared: classified SafeToRetry — the journal proves the effect never
//!   ran (dispatch never crossed), so a fresh attempt may be admitted. It is
//!   never executed and never silently aborted.
//! - DispatchCrossed: classified Succeeded only when authoritative evidence
//!   proves completion, Failed only when authoritative evidence proves safe
//!   rollback, and Indeterminate otherwise.
//!
//! Aborted is produced only by an explicit caller abort (`abort_attempt`);
//! recovery never writes an Aborted entry.
//!
//! Recovery never writes a DispatchCrossed entry and no API can redispatch a
//! recovered journal, so a crash can never cause an effect to run twice
//! through this journal. Supplied evidence is consulted only when the journal
//! is DispatchCrossed; for a Prepared or terminal journal the outcome is
//! already determined and the evidence is ignored.
//!
//! Concurrency: one writer per journal. Writes are unique-sibling temp +
//! fsync + atomic rename + directory fsync through `fs_replace` primitives.
//! A torn entry can never be observed; a crashed transition leaves either the
//! old or the new entry, never a mixture.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use zero_abi::{EffectClass, Sha256Digest, canonical_json, sha256};

use crate::fs_replace::{replace_file, sync_dir};
use crate::{DurableProfile, DurableProfileId};

pub const ATTEMPT_JOURNAL_SCHEMA_VERSION: u16 = 1;
pub const ATTEMPT_BINDING_SCHEMA_VERSION: u16 = 1;
pub const ATTEMPT_RECEIPT_SCHEMA_VERSION: u16 = 1;
pub const ATTEMPT_JOURNAL_MAX_RECORD_BYTES: u64 = 64 * 1024;
pub const ATTEMPT_JOURNAL_MAX_ENTRIES: u64 = 8;

const ATTEMPT_BINDING_DOMAIN: &[u8] = b"zerostack.attempt_journal.binding\0";
const ATTEMPT_ENTRY_DOMAIN: &[u8] = b"zerostack.attempt_journal.entry\0";
const ATTEMPT_RECOVERY_DOMAIN: &[u8] = b"zerostack.attempt_journal.recovery\0";
const LEGACY_ZBF_PROFILE_DOMAIN: &[u8] = b"zerostack.zbf_profile.v1\0";
const ATTEMPT_BINDING_DOMAIN_V1: &[u8] = b"zerostack.attempt_journal.binding.v1\0";
const ATTEMPT_ENTRY_DOMAIN_V1: &[u8] = b"zerostack.attempt_journal.entry.v1\0";

fn legacy_profile_digest(profile_id: DurableProfileId) -> Sha256Digest {
    let bytes = DurableProfile::new(profile_id).canonical_bytes();
    let mut bound = Vec::with_capacity(LEGACY_ZBF_PROFILE_DOMAIN.len() + bytes.len());
    bound.extend_from_slice(LEGACY_ZBF_PROFILE_DOMAIN);
    bound.extend_from_slice(&bytes);
    Sha256Digest::from_bytes(sha256(&bound))
}
static ATTEMPT_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
    Prepared,
    DispatchCrossed,
    Succeeded,
    Failed,
    Indeterminate,
    SafeToRetry,
    Aborted,
}
impl AttemptState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::DispatchCrossed => "dispatch_crossed",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Indeterminate => "indeterminate",
            Self::SafeToRetry => "safe_to_retry",
            Self::Aborted => "aborted",
        }
    }
    fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded
                | Self::Failed
                | Self::Indeterminate
                | Self::SafeToRetry
                | Self::Aborted
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptAbortReason {
    ExplicitAbort,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptRecoveryOutcome {
    AlreadySucceeded,
    AlreadyFailed,
    AlreadyIndeterminate,
    AlreadySafeToRetry,
    AlreadyAborted,
    ClassifiedSucceeded,
    ClassifiedFailed,
    ClassifiedIndeterminate,
    ClassifiedSafeToRetry,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptFailureCode {
    SchemaVersionMismatch,
    InvalidBinding,
    ProfileSubstitution,
    JournalMissing,
    EntryMissing,
    TornOrNoncanonicalRecord,
    RecordTooLarge,
    TooManyEntries,
    SequenceMismatch,
    InvalidTransition,
    InvalidEvidence,
    ReceiptMismatch,
    ImmutableEntryConflict,
    AlreadyTerminal,
    IoBeforePublish,
    DirectorySyncFailedAfterPublish,
    InjectedCrash,
}
impl AttemptFailureCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SchemaVersionMismatch => "schema_version_mismatch",
            Self::InvalidBinding => "invalid_binding",
            Self::ProfileSubstitution => "profile_substitution",
            Self::JournalMissing => "journal_missing",
            Self::EntryMissing => "entry_missing",
            Self::TornOrNoncanonicalRecord => "torn_or_noncanonical_record",
            Self::RecordTooLarge => "record_too_large",
            Self::TooManyEntries => "too_many_entries",
            Self::SequenceMismatch => "sequence_mismatch",
            Self::InvalidTransition => "invalid_transition",
            Self::InvalidEvidence => "invalid_evidence",
            Self::ReceiptMismatch => "receipt_mismatch",
            Self::ImmutableEntryConflict => "immutable_entry_conflict",
            Self::AlreadyTerminal => "already_terminal",
            Self::IoBeforePublish => "io_before_publish",
            Self::DirectorySyncFailedAfterPublish => "directory_sync_failed_after_publish",
            Self::InjectedCrash => "injected_crash",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptBoundary {
    PrepareBeforeWrite,
    PrepareAfterFileSync,
    PrepareAfterRename,
    PrepareAfterDirectorySync,
    DispatchCrossBeforeWrite,
    DispatchCrossAfterFileSync,
    DispatchCrossAfterRename,
    DispatchCrossAfterDirectorySync,
    SucceedBeforeWrite,
    SucceedAfterFileSync,
    SucceedAfterRename,
    SucceedAfterDirectorySync,
    FailBeforeWrite,
    FailAfterFileSync,
    FailAfterRename,
    FailAfterDirectorySync,
    IndeterminateBeforeWrite,
    IndeterminateAfterFileSync,
    IndeterminateAfterRename,
    IndeterminateAfterDirectorySync,
    AbortBeforeWrite,
    AbortAfterFileSync,
    AbortAfterRename,
    AbortAfterDirectorySync,
    RecoverBeforeWrite,
    RecoverAfterFileSync,
    RecoverAfterRename,
    RecoverAfterDirectorySync,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptJournalError {
    pub code: AttemptFailureCode,
    pub boundary: Option<AttemptBoundary>,
    pub entry_published: bool,
    pub detail: String,
}
impl AttemptJournalError {
    fn new(code: AttemptFailureCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            boundary: None,
            entry_published: false,
            detail: detail.into(),
        }
    }
    fn at(
        code: AttemptFailureCode,
        boundary: AttemptBoundary,
        entry_published: bool,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            code,
            boundary: Some(boundary),
            entry_published,
            detail: detail.into(),
        }
    }
}
impl std::fmt::Display for AttemptJournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}
impl std::error::Error for AttemptJournalError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptJournalPaths {
    directory: PathBuf,
}
impl AttemptJournalPaths {
    pub fn new(directory: impl Into<PathBuf>) -> Result<Self, AttemptJournalError> {
        let directory = directory.into();
        if directory.as_os_str().is_empty() || directory.file_name().is_none() {
            return Err(AttemptJournalError::new(
                AttemptFailureCode::InvalidBinding,
                "attempt journal directory must name a directory",
            ));
        }
        Ok(Self { directory })
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    fn entry_path(&self, sequence: u64) -> PathBuf {
        self.directory.join(format!("attempt-{sequence}.json"))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptBinding {
    pub schema_version: u16,
    pub attempt_id: Sha256Digest,
    pub effect_digest: Sha256Digest,
    pub effect_class: EffectClass,
    pub admission_anchor_digest: Sha256Digest,
    pub durable_profile_id: DurableProfileId,
    pub durable_profile_digest: Sha256Digest,
    pub owner_identity_digest: Sha256Digest,
}
impl AttemptBinding {
    pub fn new(
        attempt_id: Sha256Digest,
        effect_digest: Sha256Digest,
        effect_class: EffectClass,
        admission_anchor_digest: Sha256Digest,
        durable_profile_id: DurableProfileId,
        owner_identity_digest: Sha256Digest,
    ) -> Self {
        Self {
            schema_version: ATTEMPT_BINDING_SCHEMA_VERSION,
            attempt_id,
            effect_digest,
            effect_class,
            admission_anchor_digest,
            durable_profile_id,
            durable_profile_digest: DurableProfile::new(durable_profile_id).digest(),
            owner_identity_digest,
        }
    }
    pub fn validate(&self) -> Result<(), AttemptJournalError> {
        if self.schema_version != ATTEMPT_BINDING_SCHEMA_VERSION {
            return Err(AttemptJournalError::new(
                AttemptFailureCode::SchemaVersionMismatch,
                "attempt binding schema version is not supported",
            ));
        }
        if [
            self.attempt_id,
            self.effect_digest,
            self.admission_anchor_digest,
            self.owner_identity_digest,
        ]
        .contains(&Sha256Digest::ZERO)
        {
            return Err(AttemptJournalError::new(
                AttemptFailureCode::InvalidBinding,
                "attempt binding digests must be nonzero",
            ));
        }
        let current = DurableProfile::new(self.durable_profile_id).digest();
        let legacy = legacy_profile_digest(self.durable_profile_id);
        if self.durable_profile_digest != current && self.durable_profile_digest != legacy {
            return Err(AttemptJournalError::new(
                AttemptFailureCode::ProfileSubstitution,
                "durable profile identity does not match a known frozen digest",
            ));
        }
        Ok(())
    }
    fn uses_legacy_domains(&self) -> bool {
        self.durable_profile_digest == legacy_profile_digest(self.durable_profile_id)
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, AttemptJournalError> {
        self.validate()?;
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<Sha256Digest, AttemptJournalError> {
        let domain = if self.uses_legacy_domains() {
            ATTEMPT_BINDING_DOMAIN_V1
        } else {
            ATTEMPT_BINDING_DOMAIN
        };
        Ok(domain_digest(domain, &self.canonical_bytes()?))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum AttemptEvidence {
    Completion {
        receipt_digest: Sha256Digest,
        observed_at_unix_ns: u64,
    },
    Failure {
        failure_receipt_digest: Sha256Digest,
        observed_at_unix_ns: u64,
    },
}
impl AttemptEvidence {
    fn validate(&self) -> Result<(), AttemptJournalError> {
        match self {
            Self::Completion { receipt_digest, .. } if *receipt_digest == Sha256Digest::ZERO => {
                Err(AttemptJournalError::new(
                    AttemptFailureCode::InvalidEvidence,
                    "completion receipt digest must be nonzero",
                ))
            }
            Self::Failure {
                failure_receipt_digest,
                ..
            } if *failure_receipt_digest == Sha256Digest::ZERO => Err(AttemptJournalError::new(
                AttemptFailureCode::InvalidEvidence,
                "failure receipt digest must be nonzero",
            )),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptEntry {
    pub schema_version: u16,
    pub binding: AttemptBinding,
    pub state: AttemptState,
    pub sequence: u64,
    pub predecessor_entry_digest: Option<Sha256Digest>,
    pub crossed_at_unix_ns: Option<u64>,
    pub abort_reason: Option<AttemptAbortReason>,
    pub evidence: Option<AttemptEvidence>,
}
impl AttemptEntry {
    fn prepared(binding: AttemptBinding, sequence: u64) -> Self {
        Self {
            schema_version: ATTEMPT_JOURNAL_SCHEMA_VERSION,
            binding,
            state: AttemptState::Prepared,
            sequence,
            predecessor_entry_digest: None,
            crossed_at_unix_ns: None,
            abort_reason: None,
            evidence: None,
        }
    }
    fn dispatch_crossed(
        prepared: &Self,
        prepared_digest: Sha256Digest,
        sequence: u64,
        crossed_at_unix_ns: u64,
    ) -> Self {
        Self {
            schema_version: ATTEMPT_JOURNAL_SCHEMA_VERSION,
            binding: prepared.binding.clone(),
            state: AttemptState::DispatchCrossed,
            sequence,
            predecessor_entry_digest: Some(prepared_digest),
            crossed_at_unix_ns: Some(crossed_at_unix_ns),
            abort_reason: None,
            evidence: None,
        }
    }
    fn succeeded(
        dispatch: &Self,
        dispatch_digest: Sha256Digest,
        sequence: u64,
        evidence: AttemptEvidence,
    ) -> Self {
        Self {
            schema_version: ATTEMPT_JOURNAL_SCHEMA_VERSION,
            binding: dispatch.binding.clone(),
            state: AttemptState::Succeeded,
            sequence,
            predecessor_entry_digest: Some(dispatch_digest),
            crossed_at_unix_ns: None,
            abort_reason: None,
            evidence: Some(evidence),
        }
    }
    fn failed(
        dispatch: &Self,
        dispatch_digest: Sha256Digest,
        sequence: u64,
        evidence: AttemptEvidence,
    ) -> Self {
        Self {
            schema_version: ATTEMPT_JOURNAL_SCHEMA_VERSION,
            binding: dispatch.binding.clone(),
            state: AttemptState::Failed,
            sequence,
            predecessor_entry_digest: Some(dispatch_digest),
            crossed_at_unix_ns: None,
            abort_reason: None,
            evidence: Some(evidence),
        }
    }
    fn indeterminate(dispatch: &Self, dispatch_digest: Sha256Digest, sequence: u64) -> Self {
        Self {
            schema_version: ATTEMPT_JOURNAL_SCHEMA_VERSION,
            binding: dispatch.binding.clone(),
            state: AttemptState::Indeterminate,
            sequence,
            predecessor_entry_digest: Some(dispatch_digest),
            crossed_at_unix_ns: None,
            abort_reason: None,
            evidence: None,
        }
    }
    fn safe_to_retry(prepared: &Self, prepared_digest: Sha256Digest, sequence: u64) -> Self {
        Self {
            schema_version: ATTEMPT_JOURNAL_SCHEMA_VERSION,
            binding: prepared.binding.clone(),
            state: AttemptState::SafeToRetry,
            sequence,
            predecessor_entry_digest: Some(prepared_digest),
            crossed_at_unix_ns: None,
            abort_reason: None,
            evidence: None,
        }
    }
    fn aborted(
        prepared: &Self,
        prepared_digest: Sha256Digest,
        sequence: u64,
        reason: AttemptAbortReason,
    ) -> Self {
        Self {
            schema_version: ATTEMPT_JOURNAL_SCHEMA_VERSION,
            binding: prepared.binding.clone(),
            state: AttemptState::Aborted,
            sequence,
            predecessor_entry_digest: Some(prepared_digest),
            crossed_at_unix_ns: None,
            abort_reason: Some(reason),
            evidence: None,
        }
    }
    pub fn validate(&self) -> Result<(), AttemptJournalError> {
        if self.schema_version != ATTEMPT_JOURNAL_SCHEMA_VERSION {
            return Err(AttemptJournalError::new(
                AttemptFailureCode::SchemaVersionMismatch,
                "attempt entry schema version is not supported",
            ));
        }
        self.binding.validate()?;
        let structural = match self.state {
            AttemptState::Prepared => {
                self.sequence == 1
                    && self.predecessor_entry_digest.is_none()
                    && self.crossed_at_unix_ns.is_none()
                    && self.abort_reason.is_none()
            }
            AttemptState::DispatchCrossed => {
                self.sequence >= 2
                    && self.predecessor_entry_digest.is_some()
                    && self.crossed_at_unix_ns.is_some()
                    && self.abort_reason.is_none()
            }
            AttemptState::Succeeded
            | AttemptState::Failed
            | AttemptState::Indeterminate
            | AttemptState::SafeToRetry => {
                self.sequence >= 2
                    && self.predecessor_entry_digest.is_some()
                    && self.crossed_at_unix_ns.is_none()
                    && self.abort_reason.is_none()
            }
            AttemptState::Aborted => {
                self.sequence >= 2
                    && self.predecessor_entry_digest.is_some()
                    && self.crossed_at_unix_ns.is_none()
                    && self.abort_reason.is_some()
            }
        };
        if !structural {
            return Err(AttemptJournalError::new(
                AttemptFailureCode::SequenceMismatch,
                "attempt state and sequence commitments disagree",
            ));
        }
        let paired = match self.state {
            AttemptState::Succeeded => {
                matches!(self.evidence, Some(AttemptEvidence::Completion { .. }))
            }
            AttemptState::Failed => {
                matches!(self.evidence, Some(AttemptEvidence::Failure { .. }))
            }
            AttemptState::Prepared
            | AttemptState::DispatchCrossed
            | AttemptState::Indeterminate
            | AttemptState::SafeToRetry
            | AttemptState::Aborted => self.evidence.is_none(),
        };
        if !paired {
            return Err(AttemptJournalError::new(
                AttemptFailureCode::InvalidEvidence,
                "attempt evidence does not match the entry state",
            ));
        }
        if let Some(evidence) = &self.evidence {
            evidence.validate()?;
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, AttemptJournalError> {
        self.validate()?;
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<Sha256Digest, AttemptJournalError> {
        let domain = if self.binding.uses_legacy_domains() {
            ATTEMPT_ENTRY_DOMAIN_V1
        } else {
            ATTEMPT_ENTRY_DOMAIN
        };
        Ok(domain_digest(domain, &self.canonical_bytes()?))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptRecoveryReceipt {
    pub schema_version: u16,
    pub binding_digest: Sha256Digest,
    pub outcome: AttemptRecoveryOutcome,
    pub terminal_entry_digest: Sha256Digest,
    pub terminal_state: AttemptState,
}
impl AttemptRecoveryReceipt {
    pub fn validate(&self) -> Result<(), AttemptJournalError> {
        if self.schema_version != ATTEMPT_RECEIPT_SCHEMA_VERSION {
            return Err(AttemptJournalError::new(
                AttemptFailureCode::SchemaVersionMismatch,
                "attempt recovery receipt schema version is not supported",
            ));
        }
        if self.binding_digest == Sha256Digest::ZERO
            || self.terminal_entry_digest == Sha256Digest::ZERO
            || !self.terminal_state.is_terminal()
        {
            return Err(AttemptJournalError::new(
                AttemptFailureCode::InvalidBinding,
                "attempt recovery receipt is incomplete",
            ));
        }
        let paired = matches!(
            (self.outcome, self.terminal_state),
            (
                AttemptRecoveryOutcome::AlreadySucceeded,
                AttemptState::Succeeded
            ) | (AttemptRecoveryOutcome::AlreadyFailed, AttemptState::Failed)
                | (
                    AttemptRecoveryOutcome::AlreadyIndeterminate,
                    AttemptState::Indeterminate
                )
                | (
                    AttemptRecoveryOutcome::AlreadyAborted,
                    AttemptState::Aborted
                )
                | (
                    AttemptRecoveryOutcome::AlreadySafeToRetry,
                    AttemptState::SafeToRetry
                )
                | (
                    AttemptRecoveryOutcome::ClassifiedSucceeded,
                    AttemptState::Succeeded
                )
                | (
                    AttemptRecoveryOutcome::ClassifiedFailed,
                    AttemptState::Failed
                )
                | (
                    AttemptRecoveryOutcome::ClassifiedIndeterminate,
                    AttemptState::Indeterminate
                )
                | (
                    AttemptRecoveryOutcome::ClassifiedSafeToRetry,
                    AttemptState::SafeToRetry
                )
        );
        if !paired {
            return Err(AttemptJournalError::new(
                AttemptFailureCode::InvalidBinding,
                "attempt recovery receipt outcome and terminal state disagree",
            ));
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, AttemptJournalError> {
        self.validate()?;
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<Sha256Digest, AttemptJournalError> {
        Ok(domain_digest(
            ATTEMPT_RECOVERY_DOMAIN,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AttemptFaultPlan {
    crash_at: Option<AttemptBoundary>,
    fired: bool,
}
impl AttemptFaultPlan {
    pub const fn none() -> Self {
        Self {
            crash_at: None,
            fired: false,
        }
    }
    pub const fn crash_at(boundary: AttemptBoundary) -> Self {
        Self {
            crash_at: Some(boundary),
            fired: false,
        }
    }
    fn hit(
        &mut self,
        boundary: AttemptBoundary,
        entry_published: bool,
    ) -> Result<(), AttemptJournalError> {
        if !self.fired && self.crash_at == Some(boundary) {
            self.fired = true;
            return Err(AttemptJournalError::at(
                AttemptFailureCode::InjectedCrash,
                boundary,
                entry_published,
                "preregistered crash boundary reached",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct WriteBoundaries {
    before: AttemptBoundary,
    file_sync: AttemptBoundary,
    rename: AttemptBoundary,
    dir_sync: AttemptBoundary,
}
const PREPARE_BOUNDARIES: WriteBoundaries = WriteBoundaries {
    before: AttemptBoundary::PrepareBeforeWrite,
    file_sync: AttemptBoundary::PrepareAfterFileSync,
    rename: AttemptBoundary::PrepareAfterRename,
    dir_sync: AttemptBoundary::PrepareAfterDirectorySync,
};
const DISPATCH_CROSS_BOUNDARIES: WriteBoundaries = WriteBoundaries {
    before: AttemptBoundary::DispatchCrossBeforeWrite,
    file_sync: AttemptBoundary::DispatchCrossAfterFileSync,
    rename: AttemptBoundary::DispatchCrossAfterRename,
    dir_sync: AttemptBoundary::DispatchCrossAfterDirectorySync,
};
const SUCCEED_BOUNDARIES: WriteBoundaries = WriteBoundaries {
    before: AttemptBoundary::SucceedBeforeWrite,
    file_sync: AttemptBoundary::SucceedAfterFileSync,
    rename: AttemptBoundary::SucceedAfterRename,
    dir_sync: AttemptBoundary::SucceedAfterDirectorySync,
};
const FAIL_BOUNDARIES: WriteBoundaries = WriteBoundaries {
    before: AttemptBoundary::FailBeforeWrite,
    file_sync: AttemptBoundary::FailAfterFileSync,
    rename: AttemptBoundary::FailAfterRename,
    dir_sync: AttemptBoundary::FailAfterDirectorySync,
};
const INDETERMINATE_BOUNDARIES: WriteBoundaries = WriteBoundaries {
    before: AttemptBoundary::IndeterminateBeforeWrite,
    file_sync: AttemptBoundary::IndeterminateAfterFileSync,
    rename: AttemptBoundary::IndeterminateAfterRename,
    dir_sync: AttemptBoundary::IndeterminateAfterDirectorySync,
};
const ABORT_BOUNDARIES: WriteBoundaries = WriteBoundaries {
    before: AttemptBoundary::AbortBeforeWrite,
    file_sync: AttemptBoundary::AbortAfterFileSync,
    rename: AttemptBoundary::AbortAfterRename,
    dir_sync: AttemptBoundary::AbortAfterDirectorySync,
};
const RECOVER_BOUNDARIES: WriteBoundaries = WriteBoundaries {
    before: AttemptBoundary::RecoverBeforeWrite,
    file_sync: AttemptBoundary::RecoverAfterFileSync,
    rename: AttemptBoundary::RecoverAfterRename,
    dir_sync: AttemptBoundary::RecoverAfterDirectorySync,
};

/// Persist the Prepared entry before effect admission. Idempotent for the
/// same binding; a journal that already crossed dispatch or is terminal is
/// rejected with `AlreadyTerminal`.
pub fn prepare_attempt(
    paths: &AttemptJournalPaths,
    binding: AttemptBinding,
) -> Result<AttemptEntry, AttemptJournalError> {
    prepare_attempt_with_fault(paths, binding, &mut AttemptFaultPlan::none())
}
pub fn prepare_attempt_with_fault(
    paths: &AttemptJournalPaths,
    binding: AttemptBinding,
    fault: &mut AttemptFaultPlan,
) -> Result<AttemptEntry, AttemptJournalError> {
    binding.validate()?;
    let chain = read_chain(paths)?;
    if let Some(current) = chain.last() {
        if current.value.state == AttemptState::Prepared {
            if current.value.binding == binding {
                return Ok(current.value.clone());
            }
            return Err(AttemptJournalError::new(
                AttemptFailureCode::ImmutableEntryConflict,
                "attempt journal is already prepared with a different binding",
            ));
        }
        return Err(AttemptJournalError::new(
            AttemptFailureCode::AlreadyTerminal,
            "attempt journal already crossed dispatch or is terminal",
        ));
    }
    let prepared = AttemptEntry::prepared(binding, 1);
    write_once_entry(paths, 1, &prepared, PREPARE_BOUNDARIES, fault)?;
    Ok(prepared)
}

/// Persist the dispatch boundary immediately before the effect is dispatched.
/// The caller must hold the digest of the persisted Prepared entry.
pub fn mark_dispatch_crossed(
    paths: &AttemptJournalPaths,
    prepared_entry_digest: Sha256Digest,
    crossed_at_unix_ns: u64,
) -> Result<AttemptEntry, AttemptJournalError> {
    mark_dispatch_crossed_with_fault(
        paths,
        prepared_entry_digest,
        crossed_at_unix_ns,
        &mut AttemptFaultPlan::none(),
    )
}
pub fn mark_dispatch_crossed_with_fault(
    paths: &AttemptJournalPaths,
    prepared_entry_digest: Sha256Digest,
    crossed_at_unix_ns: u64,
    fault: &mut AttemptFaultPlan,
) -> Result<AttemptEntry, AttemptJournalError> {
    let chain = read_chain(paths)?;
    let Some(current) = chain.last() else {
        return Err(AttemptJournalError::new(
            AttemptFailureCode::JournalMissing,
            "no prepared attempt exists to dispatch",
        ));
    };
    match current.value.state {
        AttemptState::Prepared => {}
        AttemptState::DispatchCrossed => {
            if current.value.predecessor_entry_digest == Some(prepared_entry_digest) {
                return Ok(current.value.clone());
            }
            return Err(AttemptJournalError::new(
                AttemptFailureCode::ReceiptMismatch,
                "dispatch already crossed for a different prepared entry",
            ));
        }
        state => {
            return Err(AttemptJournalError::new(
                AttemptFailureCode::AlreadyTerminal,
                format!(
                    "terminal attempt cannot be redispatched (current state {})",
                    state.as_str()
                ),
            ));
        }
    }
    if current.digest != prepared_entry_digest {
        return Err(AttemptJournalError::new(
            AttemptFailureCode::ReceiptMismatch,
            "prepared entry digest does not match the persisted journal",
        ));
    }
    let sequence = chain.len() as u64 + 1;
    let crossed = AttemptEntry::dispatch_crossed(
        &current.value,
        current.digest,
        sequence,
        crossed_at_unix_ns,
    );
    write_once_entry(paths, sequence, &crossed, DISPATCH_CROSS_BOUNDARIES, fault)?;
    Ok(crossed)
}

/// Persist authoritative completion evidence as a terminal Succeeded entry.
pub fn mark_succeeded(
    paths: &AttemptJournalPaths,
    dispatch_entry_digest: Sha256Digest,
    receipt_digest: Sha256Digest,
    observed_at_unix_ns: u64,
) -> Result<AttemptEntry, AttemptJournalError> {
    mark_succeeded_with_fault(
        paths,
        dispatch_entry_digest,
        receipt_digest,
        observed_at_unix_ns,
        &mut AttemptFaultPlan::none(),
    )
}
pub fn mark_succeeded_with_fault(
    paths: &AttemptJournalPaths,
    dispatch_entry_digest: Sha256Digest,
    receipt_digest: Sha256Digest,
    observed_at_unix_ns: u64,
    fault: &mut AttemptFaultPlan,
) -> Result<AttemptEntry, AttemptJournalError> {
    if receipt_digest == Sha256Digest::ZERO {
        return Err(AttemptJournalError::new(
            AttemptFailureCode::InvalidEvidence,
            "completion receipt digest must be nonzero",
        ));
    }
    let evidence = AttemptEvidence::Completion {
        receipt_digest,
        observed_at_unix_ns,
    };
    mark_terminal_with_fault(
        paths,
        dispatch_entry_digest,
        AttemptState::Succeeded,
        Some(evidence),
        SUCCEED_BOUNDARIES,
        fault,
    )
}

/// Persist authoritative failure evidence as a terminal Failed entry.
pub fn mark_failed(
    paths: &AttemptJournalPaths,
    dispatch_entry_digest: Sha256Digest,
    failure_receipt_digest: Sha256Digest,
    observed_at_unix_ns: u64,
) -> Result<AttemptEntry, AttemptJournalError> {
    mark_failed_with_fault(
        paths,
        dispatch_entry_digest,
        failure_receipt_digest,
        observed_at_unix_ns,
        &mut AttemptFaultPlan::none(),
    )
}
pub fn mark_failed_with_fault(
    paths: &AttemptJournalPaths,
    dispatch_entry_digest: Sha256Digest,
    failure_receipt_digest: Sha256Digest,
    observed_at_unix_ns: u64,
    fault: &mut AttemptFaultPlan,
) -> Result<AttemptEntry, AttemptJournalError> {
    if failure_receipt_digest == Sha256Digest::ZERO {
        return Err(AttemptJournalError::new(
            AttemptFailureCode::InvalidEvidence,
            "failure receipt digest must be nonzero",
        ));
    }
    let evidence = AttemptEvidence::Failure {
        failure_receipt_digest,
        observed_at_unix_ns,
    };
    mark_terminal_with_fault(
        paths,
        dispatch_entry_digest,
        AttemptState::Failed,
        Some(evidence),
        FAIL_BOUNDARIES,
        fault,
    )
}

/// Persist a terminal Indeterminate entry: the effect may have run and no
/// authoritative evidence exists. The attempt is never redispatched.
pub fn mark_indeterminate(
    paths: &AttemptJournalPaths,
    dispatch_entry_digest: Sha256Digest,
) -> Result<AttemptEntry, AttemptJournalError> {
    mark_indeterminate_with_fault(paths, dispatch_entry_digest, &mut AttemptFaultPlan::none())
}
pub fn mark_indeterminate_with_fault(
    paths: &AttemptJournalPaths,
    dispatch_entry_digest: Sha256Digest,
    fault: &mut AttemptFaultPlan,
) -> Result<AttemptEntry, AttemptJournalError> {
    mark_terminal_with_fault(
        paths,
        dispatch_entry_digest,
        AttemptState::Indeterminate,
        None,
        INDETERMINATE_BOUNDARIES,
        fault,
    )
}

fn mark_terminal_with_fault(
    paths: &AttemptJournalPaths,
    dispatch_entry_digest: Sha256Digest,
    terminal_state: AttemptState,
    evidence: Option<AttemptEvidence>,
    boundaries: WriteBoundaries,
    fault: &mut AttemptFaultPlan,
) -> Result<AttemptEntry, AttemptJournalError> {
    let chain = read_chain(paths)?;
    let Some(current) = chain.last() else {
        return Err(AttemptJournalError::new(
            AttemptFailureCode::JournalMissing,
            "no dispatched attempt exists to resolve",
        ));
    };
    if current.value.state == terminal_state {
        let dispatch = &chain[chain.len() - 2];
        if dispatch.digest != dispatch_entry_digest {
            return Err(AttemptJournalError::new(
                AttemptFailureCode::ReceiptMismatch,
                "dispatch entry digest does not match the persisted journal",
            ));
        }
        let candidate = terminal_entry(
            &dispatch.value,
            dispatch.digest,
            chain.len() as u64,
            terminal_state,
            evidence,
        );
        if candidate == current.value {
            return Ok(current.value.clone());
        }
        return Err(AttemptJournalError::new(
            AttemptFailureCode::ImmutableEntryConflict,
            "terminal entry already exists with different evidence",
        ));
    }
    if current.value.state != AttemptState::DispatchCrossed {
        return Err(AttemptJournalError::new(
            AttemptFailureCode::InvalidTransition,
            format!(
                "cannot transition from {} to {}",
                current.value.state.as_str(),
                terminal_state.as_str()
            ),
        ));
    }
    if current.digest != dispatch_entry_digest {
        return Err(AttemptJournalError::new(
            AttemptFailureCode::ReceiptMismatch,
            "dispatch entry digest does not match the persisted journal",
        ));
    }
    let sequence = chain.len() as u64 + 1;
    let terminal = terminal_entry(
        &current.value,
        current.digest,
        sequence,
        terminal_state,
        evidence,
    );
    write_once_entry(paths, sequence, &terminal, boundaries, fault)?;
    Ok(terminal)
}

fn terminal_entry(
    dispatch: &AttemptEntry,
    dispatch_digest: Sha256Digest,
    sequence: u64,
    state: AttemptState,
    evidence: Option<AttemptEvidence>,
) -> AttemptEntry {
    match state {
        AttemptState::Succeeded => AttemptEntry::succeeded(
            dispatch,
            dispatch_digest,
            sequence,
            evidence.expect("succeeded requires completion evidence"),
        ),
        AttemptState::Failed => AttemptEntry::failed(
            dispatch,
            dispatch_digest,
            sequence,
            evidence.expect("failed requires failure evidence"),
        ),
        AttemptState::Indeterminate => {
            AttemptEntry::indeterminate(dispatch, dispatch_digest, sequence)
        }
        _ => unreachable!("terminal_entry only builds succeeded, failed, indeterminate"),
    }
}

/// Abort a Prepared attempt before dispatch. Terminal Aborted entries are
/// immutable; aborting an already-aborted attempt is idempotent.
pub fn abort_attempt(
    paths: &AttemptJournalPaths,
    prepared_entry_digest: Sha256Digest,
) -> Result<AttemptEntry, AttemptJournalError> {
    abort_attempt_with_fault(paths, prepared_entry_digest, &mut AttemptFaultPlan::none())
}
pub fn abort_attempt_with_fault(
    paths: &AttemptJournalPaths,
    prepared_entry_digest: Sha256Digest,
    fault: &mut AttemptFaultPlan,
) -> Result<AttemptEntry, AttemptJournalError> {
    let chain = read_chain(paths)?;
    let Some(current) = chain.last() else {
        return Err(AttemptJournalError::new(
            AttemptFailureCode::JournalMissing,
            "no prepared attempt exists to abort",
        ));
    };
    match current.value.state {
        AttemptState::Prepared => {
            if current.digest != prepared_entry_digest {
                return Err(AttemptJournalError::new(
                    AttemptFailureCode::ReceiptMismatch,
                    "prepared entry digest does not match the persisted journal",
                ));
            }
            let sequence = chain.len() as u64 + 1;
            let aborted = AttemptEntry::aborted(
                &current.value,
                current.digest,
                sequence,
                AttemptAbortReason::ExplicitAbort,
            );
            write_once_entry(paths, sequence, &aborted, ABORT_BOUNDARIES, fault)?;
            Ok(aborted)
        }
        AttemptState::Aborted => {
            let prepared = &chain[chain.len() - 2];
            if prepared.digest != prepared_entry_digest {
                return Err(AttemptJournalError::new(
                    AttemptFailureCode::ReceiptMismatch,
                    "prepared entry digest does not match the persisted journal",
                ));
            }
            Ok(current.value.clone())
        }
        state => Err(AttemptJournalError::new(
            AttemptFailureCode::InvalidTransition,
            format!("cannot transition from {} to aborted", state.as_str()),
        )),
    }
}

/// Recover an attempt journal.
///
/// Terminals are returned unchanged. A Prepared journal is classified
/// SafeToRetry: the journal proves the effect never ran (dispatch never
/// crossed), so a fresh attempt may be admitted but this journal can never
/// dispatch. A DispatchCrossed journal is classified Succeeded or Failed
/// only when authoritative evidence proves completion or safe rollback, and
/// Indeterminate otherwise. Recovery never writes a DispatchCrossed entry,
/// so no recovered attempt can be redispatched.
pub fn recover_attempt(
    paths: &AttemptJournalPaths,
    expected: &AttemptBinding,
    evidence: Option<AttemptEvidence>,
) -> Result<AttemptRecoveryReceipt, AttemptJournalError> {
    recover_attempt_with_fault(paths, expected, evidence, &mut AttemptFaultPlan::none())
}
pub fn recover_attempt_with_fault(
    paths: &AttemptJournalPaths,
    expected: &AttemptBinding,
    evidence: Option<AttemptEvidence>,
    fault: &mut AttemptFaultPlan,
) -> Result<AttemptRecoveryReceipt, AttemptJournalError> {
    expected.validate()?;
    let chain = read_chain(paths)?;
    let Some(current) = chain.last() else {
        return Err(AttemptJournalError::new(
            AttemptFailureCode::JournalMissing,
            "no attempt journal exists to recover",
        ));
    };
    if chain[0].value.binding != *expected {
        return Err(AttemptJournalError::new(
            AttemptFailureCode::InvalidBinding,
            "persisted attempt binding differs from recovery expectation",
        ));
    }
    let binding_digest = expected.digest()?;
    match current.value.state {
        AttemptState::Succeeded => make_recovery_receipt(
            binding_digest,
            AttemptRecoveryOutcome::AlreadySucceeded,
            &current.value,
        ),
        AttemptState::Failed => make_recovery_receipt(
            binding_digest,
            AttemptRecoveryOutcome::AlreadyFailed,
            &current.value,
        ),
        AttemptState::Indeterminate => make_recovery_receipt(
            binding_digest,
            AttemptRecoveryOutcome::AlreadyIndeterminate,
            &current.value,
        ),
        AttemptState::SafeToRetry => make_recovery_receipt(
            binding_digest,
            AttemptRecoveryOutcome::AlreadySafeToRetry,
            &current.value,
        ),
        AttemptState::Aborted => make_recovery_receipt(
            binding_digest,
            AttemptRecoveryOutcome::AlreadyAborted,
            &current.value,
        ),
        AttemptState::Prepared => {
            // Dispatch never crossed, so the journal proves the effect never
            // ran: classify SafeToRetry and terminate the journal. A fresh
            // attempt may be admitted elsewhere; this journal can never
            // dispatch. Supplied evidence is ignored — an attempt that never
            // ran has no outcome evidence.
            let sequence = chain.len() as u64 + 1;
            let safe_to_retry =
                AttemptEntry::safe_to_retry(&current.value, current.digest, sequence);
            write_once_entry(paths, sequence, &safe_to_retry, RECOVER_BOUNDARIES, fault)?;
            make_recovery_receipt(
                binding_digest,
                AttemptRecoveryOutcome::ClassifiedSafeToRetry,
                &safe_to_retry,
            )
        }
        AttemptState::DispatchCrossed => {
            let sequence = chain.len() as u64 + 1;
            let (outcome, terminal) = match evidence {
                Some(evidence @ AttemptEvidence::Completion { .. }) => (
                    AttemptRecoveryOutcome::ClassifiedSucceeded,
                    AttemptEntry::succeeded(&current.value, current.digest, sequence, evidence),
                ),
                Some(evidence @ AttemptEvidence::Failure { .. }) => (
                    AttemptRecoveryOutcome::ClassifiedFailed,
                    AttemptEntry::failed(&current.value, current.digest, sequence, evidence),
                ),
                None => (
                    AttemptRecoveryOutcome::ClassifiedIndeterminate,
                    AttemptEntry::indeterminate(&current.value, current.digest, sequence),
                ),
            };
            write_once_entry(paths, sequence, &terminal, RECOVER_BOUNDARIES, fault)?;
            make_recovery_receipt(binding_digest, outcome, &terminal)
        }
    }
}

/// Read the current (latest) entry, if any. Validates the whole chain; a torn
/// or non-contiguous journal fails loudly.
pub fn read_current_attempt(
    paths: &AttemptJournalPaths,
) -> Result<Option<AttemptEntry>, AttemptJournalError> {
    Ok(read_chain(paths)?.last().map(|entry| entry.value.clone()))
}

/// Read a single entry by sequence without chain validation.
pub fn read_attempt_entry(
    paths: &AttemptJournalPaths,
    sequence: u64,
) -> Result<AttemptEntry, AttemptJournalError> {
    Ok(read_canonical_entry(paths, sequence)?.value)
}

fn make_recovery_receipt(
    binding_digest: Sha256Digest,
    outcome: AttemptRecoveryOutcome,
    terminal: &AttemptEntry,
) -> Result<AttemptRecoveryReceipt, AttemptJournalError> {
    let receipt = AttemptRecoveryReceipt {
        schema_version: ATTEMPT_RECEIPT_SCHEMA_VERSION,
        binding_digest,
        outcome,
        terminal_entry_digest: terminal.digest()?,
        terminal_state: terminal.state,
    };
    receipt.validate()?;
    Ok(receipt)
}

fn is_valid_transition(from: AttemptState, to: AttemptState) -> bool {
    matches!(
        (from, to),
        (AttemptState::Prepared, AttemptState::DispatchCrossed)
            | (AttemptState::Prepared, AttemptState::Aborted)
            | (AttemptState::Prepared, AttemptState::SafeToRetry)
            | (AttemptState::DispatchCrossed, AttemptState::Succeeded)
            | (AttemptState::DispatchCrossed, AttemptState::Failed)
            | (AttemptState::DispatchCrossed, AttemptState::Indeterminate)
    )
}

struct CanonicalRead<T> {
    value: T,
    digest: Sha256Digest,
}

fn read_chain(
    paths: &AttemptJournalPaths,
) -> Result<Vec<CanonicalRead<AttemptEntry>>, AttemptJournalError> {
    let mut entries: Vec<CanonicalRead<AttemptEntry>> = Vec::new();
    let mut sequence: u64 = 1;
    while sequence <= ATTEMPT_JOURNAL_MAX_ENTRIES {
        match read_canonical_entry(paths, sequence) {
            Ok(entry) => entries.push(entry),
            Err(error) if error.code == AttemptFailureCode::EntryMissing => break,
            Err(error) => return Err(error),
        }
        sequence += 1;
    }
    if entries.is_empty() {
        return Ok(entries);
    }
    for probe in sequence..=ATTEMPT_JOURNAL_MAX_ENTRIES {
        if read_canonical_entry(paths, probe).is_ok() {
            return Err(AttemptJournalError::new(
                AttemptFailureCode::SequenceMismatch,
                "attempt entry chain is not contiguous",
            ));
        }
    }
    for (index, entry) in entries.iter().enumerate() {
        if entry.value.sequence != index as u64 + 1 {
            return Err(AttemptJournalError::new(
                AttemptFailureCode::SequenceMismatch,
                "attempt entry sequence does not match its chain position",
            ));
        }
        if index > 0 {
            let previous = &entries[index - 1];
            if entry.value.predecessor_entry_digest != Some(previous.digest) {
                return Err(AttemptJournalError::new(
                    AttemptFailureCode::SequenceMismatch,
                    "attempt entry predecessor digest does not chain",
                ));
            }
            if !is_valid_transition(previous.value.state, entry.value.state) {
                return Err(AttemptJournalError::new(
                    AttemptFailureCode::InvalidTransition,
                    format!(
                        "attempt chain contains a forbidden transition from {} to {}",
                        previous.value.state.as_str(),
                        entry.value.state.as_str(),
                    ),
                ));
            }
        }
    }
    Ok(entries)
}

fn read_canonical_entry(
    paths: &AttemptJournalPaths,
    sequence: u64,
) -> Result<CanonicalRead<AttemptEntry>, AttemptJournalError> {
    let mut read = read_canonical::<AttemptEntry>(
        &paths.entry_path(sequence),
        AttemptFailureCode::EntryMissing,
        ATTEMPT_ENTRY_DOMAIN,
    )?;
    read.value.validate()?;
    // The 2026-08 profile-domain cutover changed both profile and journal
    // domains without changing the numeric schema. Recompute from the
    // validated binding so old chains retain their original predecessor IDs.
    read.digest = read.value.digest()?;
    Ok(read)
}

fn read_canonical<T>(
    path: &Path,
    missing: AttemptFailureCode,
    domain: &[u8],
) -> Result<CanonicalRead<T>, AttemptJournalError>
where
    T: DeserializeOwned + Serialize,
{
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(AttemptJournalError::new(
                missing,
                "required entry is absent",
            ));
        }
        Err(error) => {
            return Err(AttemptJournalError::new(
                AttemptFailureCode::IoBeforePublish,
                format!("entry stat failed: {error}"),
            ));
        }
    };
    if !metadata.file_type().is_file() {
        return Err(AttemptJournalError::new(
            AttemptFailureCode::TornOrNoncanonicalRecord,
            "entry is not a regular file",
        ));
    }
    if metadata.len() > ATTEMPT_JOURNAL_MAX_RECORD_BYTES {
        return Err(AttemptJournalError::new(
            AttemptFailureCode::RecordTooLarge,
            "entry exceeds the frozen byte bound",
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        AttemptJournalError::new(
            AttemptFailureCode::IoBeforePublish,
            format!("entry read failed: {error}"),
        )
    })?;
    let value: T = serde_json::from_slice(&bytes).map_err(|error| {
        AttemptJournalError::new(
            AttemptFailureCode::TornOrNoncanonicalRecord,
            format!("entry decode failed: {error}"),
        )
    })?;
    let canonical = canonical_bytes(&value)?;
    if canonical != bytes {
        return Err(AttemptJournalError::new(
            AttemptFailureCode::TornOrNoncanonicalRecord,
            "entry bytes are not canonical JSON",
        ));
    }
    Ok(CanonicalRead {
        value,
        digest: domain_digest(domain, &canonical),
    })
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, AttemptJournalError> {
    let value = serde_json::to_value(value).map_err(|error| {
        AttemptJournalError::new(
            AttemptFailureCode::InvalidBinding,
            format!("entry serialization failed: {error}"),
        )
    })?;
    let bytes = canonical_json(&value).into_bytes();
    if bytes.len() as u64 > ATTEMPT_JOURNAL_MAX_RECORD_BYTES {
        return Err(AttemptJournalError::new(
            AttemptFailureCode::RecordTooLarge,
            "canonical entry exceeds the frozen byte bound",
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

fn write_once_entry(
    paths: &AttemptJournalPaths,
    sequence: u64,
    entry: &AttemptEntry,
    boundaries: WriteBoundaries,
    fault: &mut AttemptFaultPlan,
) -> Result<(), AttemptJournalError> {
    if sequence > ATTEMPT_JOURNAL_MAX_ENTRIES {
        return Err(AttemptJournalError::new(
            AttemptFailureCode::TooManyEntries,
            "attempt journal entry bound exceeded",
        ));
    }
    let bytes = entry.canonical_bytes()?;
    let path = paths.entry_path(sequence);
    match fs::read(&path) {
        Ok(existing) if existing == bytes => return Ok(()),
        Ok(_) => {
            return Err(AttemptJournalError::new(
                AttemptFailureCode::ImmutableEntryConflict,
                "immutable entry already exists with different bytes",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(AttemptJournalError::new(
                AttemptFailureCode::IoBeforePublish,
                format!("immutable entry read failed: {error}"),
            ));
        }
    }
    durable_replace(&path, &bytes, boundaries, fault)
}

fn durable_replace(
    path: &Path,
    bytes: &[u8],
    boundaries: WriteBoundaries,
    fault: &mut AttemptFaultPlan,
) -> Result<(), AttemptJournalError> {
    if bytes.len() as u64 > ATTEMPT_JOURNAL_MAX_RECORD_BYTES {
        return Err(AttemptJournalError::new(
            AttemptFailureCode::RecordTooLarge,
            "write exceeds the frozen record byte bound",
        ));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| io_before(boundaries.before, error))?;
    let file_name = path.file_name().ok_or_else(|| {
        AttemptJournalError::new(
            AttemptFailureCode::InvalidBinding,
            "entry path has no file name",
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
            AttemptJournalError::at(
                AttemptFailureCode::DirectorySyncFailedAfterPublish,
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
        let sequence = ATTEMPT_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut name = OsString::from(".");
        name.push(file_name);
        name.push(format!(".attempt-tmp-{}-{sequence}", std::process::id()));
        let path = parent.join(name);
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

fn io_before(boundary: AttemptBoundary, error: io::Error) -> AttemptJournalError {
    AttemptJournalError::at(
        AttemptFailureCode::IoBeforePublish,
        boundary,
        false,
        format!("durable write failed before publication: {error}"),
    )
}

/// Machine-readable contract summary used by conformance generators.
pub fn attempt_journal_contract() -> serde_json::Value {
    json!({
        "schema_version": ATTEMPT_JOURNAL_SCHEMA_VERSION,
        "binding_schema_version": ATTEMPT_BINDING_SCHEMA_VERSION,
        "receipt_schema_version": ATTEMPT_RECEIPT_SCHEMA_VERSION,
        "max_record_bytes": ATTEMPT_JOURNAL_MAX_RECORD_BYTES,
        "max_entries": ATTEMPT_JOURNAL_MAX_ENTRIES,
        "entry_file": "attempt-<sequence>.json",
        "states": ["prepared", "dispatch_crossed", "succeeded", "failed",
            "indeterminate", "safe_to_retry", "aborted"],
        "transitions": {
            "prepared": ["dispatch_crossed", "aborted", "safe_to_retry"],
            "dispatch_crossed": ["succeeded", "failed", "indeterminate"],
            "succeeded": [],
            "failed": [],
            "indeterminate": [],
            "safe_to_retry": [],
            "aborted": []
        },
        "prepared_persisted_before": "effect admission",
        "dispatch_crossed_persisted_before": "effect dispatch",
        "recovery": "prepared classifies safe_to_retry (never dispatched, never executed); dispatch_crossed classifies by authoritative evidence else indeterminate; terminals return unchanged; aborted only by explicit abort",
        "no_replay": "recovery never writes dispatch_crossed; no api dispatches a recovered journal",
        "immutable": "write-once entries; terminal entries never replaced",
        "typed_failure_codes": ["schema_version_mismatch", "invalid_binding",
            "profile_substitution", "journal_missing", "entry_missing",
            "torn_or_noncanonical_record", "record_too_large", "too_many_entries",
            "sequence_mismatch", "invalid_transition", "invalid_evidence",
            "receipt_mismatch", "immutable_entry_conflict", "already_terminal",
            "io_before_publish", "directory_sync_failed_after_publish", "injected_crash"]
    })
}
