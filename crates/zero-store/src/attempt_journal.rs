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
//! 1. `prepare_attempt_v1` persists the write-once Prepared entry.
//! 2. Effect admission happens only after prepare returns.
//! 3. `mark_dispatch_crossed_v1` persists the dispatch boundary immediately
//!    before the effect is dispatched.
//! 4. The effect runs; the caller persists `mark_succeeded_v1`,
//!    `mark_failed_v1`, or `mark_indeterminate_v1`, or crashes.
//!
//! Entries are immutable: each sequence number is a distinct write-once file
//! (`attempt-<sequence>.json`) inside the caller-supplied directory, so a
//! terminal entry can never be replaced. Terminal entries carry canonical
//! evidence: completion receipts, failure receipts, or an abort reason.
//! Receipt bodies live in higher layers — zero-gate depends on zero-store, so
//! this crate cannot name gate receipt types — and the journal binds their
//! canonical digests, which are authoritative once persisted.
//!
//! Recovery law (`recover_attempt_v1`):
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
//! Aborted is produced only by an explicit caller abort (`abort_attempt_v1`);
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
use zero_abi::{DigestV1, EffectClass, canonical_json, sha256};

use crate::fs_replace::{replace_file, sync_dir};
use crate::{DurableProfileIdV1, DurableProfileV1};

pub const ATTEMPT_JOURNAL_SCHEMA_VERSION_V1: u16 = 1;
pub const ATTEMPT_BINDING_SCHEMA_VERSION_V1: u16 = 1;
pub const ATTEMPT_RECEIPT_SCHEMA_VERSION_V1: u16 = 1;
pub const ATTEMPT_JOURNAL_MAX_RECORD_BYTES_V1: u64 = 64 * 1024;
pub const ATTEMPT_JOURNAL_MAX_ENTRIES_V1: u64 = 8;

const ATTEMPT_BINDING_DOMAIN_V1: &[u8] = b"zerostack.attempt_journal.binding.v1\0";
const ATTEMPT_ENTRY_DOMAIN_V1: &[u8] = b"zerostack.attempt_journal.entry.v1\0";
const ATTEMPT_RECOVERY_DOMAIN_V1: &[u8] = b"zerostack.attempt_journal.recovery.v1\0";
static ATTEMPT_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStateV1 {
    Prepared,
    DispatchCrossed,
    Succeeded,
    Failed,
    Indeterminate,
    SafeToRetry,
    Aborted,
}
impl AttemptStateV1 {
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
pub enum AttemptAbortReasonV1 {
    ExplicitAbort,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptRecoveryOutcomeV1 {
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
pub enum AttemptFailureCodeV1 {
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
impl AttemptFailureCodeV1 {
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
pub enum AttemptBoundaryV1 {
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
pub struct AttemptJournalErrorV1 {
    pub code: AttemptFailureCodeV1,
    pub boundary: Option<AttemptBoundaryV1>,
    pub entry_published: bool,
    pub detail: String,
}
impl AttemptJournalErrorV1 {
    fn new(code: AttemptFailureCodeV1, detail: impl Into<String>) -> Self {
        Self {
            code,
            boundary: None,
            entry_published: false,
            detail: detail.into(),
        }
    }
    fn at(
        code: AttemptFailureCodeV1,
        boundary: AttemptBoundaryV1,
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
impl std::fmt::Display for AttemptJournalErrorV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}
impl std::error::Error for AttemptJournalErrorV1 {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptJournalPathsV1 {
    directory: PathBuf,
}
impl AttemptJournalPathsV1 {
    pub fn new(directory: impl Into<PathBuf>) -> Result<Self, AttemptJournalErrorV1> {
        let directory = directory.into();
        if directory.as_os_str().is_empty() || directory.file_name().is_none() {
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::InvalidBinding,
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
pub struct AttemptBindingV1 {
    pub schema_version: u16,
    pub attempt_id: DigestV1,
    pub effect_digest: DigestV1,
    pub effect_class: EffectClass,
    pub admission_anchor_digest: DigestV1,
    pub durable_profile_id: DurableProfileIdV1,
    pub durable_profile_digest: DigestV1,
    pub owner_identity_digest: DigestV1,
}
impl AttemptBindingV1 {
    pub fn new(
        attempt_id: DigestV1,
        effect_digest: DigestV1,
        effect_class: EffectClass,
        admission_anchor_digest: DigestV1,
        durable_profile_id: DurableProfileIdV1,
        owner_identity_digest: DigestV1,
    ) -> Self {
        Self {
            schema_version: ATTEMPT_BINDING_SCHEMA_VERSION_V1,
            attempt_id,
            effect_digest,
            effect_class,
            admission_anchor_digest,
            durable_profile_id,
            durable_profile_digest: DurableProfileV1::new(durable_profile_id).digest(),
            owner_identity_digest,
        }
    }
    pub fn validate(&self) -> Result<(), AttemptJournalErrorV1> {
        if self.schema_version != ATTEMPT_BINDING_SCHEMA_VERSION_V1 {
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::SchemaVersionMismatch,
                "attempt binding schema version is not supported",
            ));
        }
        if [
            self.attempt_id,
            self.effect_digest,
            self.admission_anchor_digest,
            self.owner_identity_digest,
        ]
        .contains(&DigestV1::ZERO)
        {
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::InvalidBinding,
                "attempt binding digests must be nonzero",
            ));
        }
        if self.durable_profile_digest != DurableProfileV1::new(self.durable_profile_id).digest() {
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::ProfileSubstitution,
                "durable profile identity does not match its frozen digest",
            ));
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, AttemptJournalErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<DigestV1, AttemptJournalErrorV1> {
        Ok(domain_digest(
            ATTEMPT_BINDING_DOMAIN_V1,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum AttemptEvidenceV1 {
    Completion {
        receipt_digest: DigestV1,
        observed_at_unix_ns: u64,
    },
    Failure {
        failure_receipt_digest: DigestV1,
        observed_at_unix_ns: u64,
    },
}
impl AttemptEvidenceV1 {
    fn validate(&self) -> Result<(), AttemptJournalErrorV1> {
        match self {
            Self::Completion { receipt_digest, .. } if *receipt_digest == DigestV1::ZERO => {
                Err(AttemptJournalErrorV1::new(
                    AttemptFailureCodeV1::InvalidEvidence,
                    "completion receipt digest must be nonzero",
                ))
            }
            Self::Failure {
                failure_receipt_digest,
                ..
            } if *failure_receipt_digest == DigestV1::ZERO => Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::InvalidEvidence,
                "failure receipt digest must be nonzero",
            )),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptEntryV1 {
    pub schema_version: u16,
    pub binding: AttemptBindingV1,
    pub state: AttemptStateV1,
    pub sequence: u64,
    pub predecessor_entry_digest: Option<DigestV1>,
    pub crossed_at_unix_ns: Option<u64>,
    pub abort_reason: Option<AttemptAbortReasonV1>,
    pub evidence: Option<AttemptEvidenceV1>,
}
impl AttemptEntryV1 {
    fn prepared(binding: AttemptBindingV1, sequence: u64) -> Self {
        Self {
            schema_version: ATTEMPT_JOURNAL_SCHEMA_VERSION_V1,
            binding,
            state: AttemptStateV1::Prepared,
            sequence,
            predecessor_entry_digest: None,
            crossed_at_unix_ns: None,
            abort_reason: None,
            evidence: None,
        }
    }
    fn dispatch_crossed(
        prepared: &Self,
        prepared_digest: DigestV1,
        sequence: u64,
        crossed_at_unix_ns: u64,
    ) -> Self {
        Self {
            schema_version: ATTEMPT_JOURNAL_SCHEMA_VERSION_V1,
            binding: prepared.binding.clone(),
            state: AttemptStateV1::DispatchCrossed,
            sequence,
            predecessor_entry_digest: Some(prepared_digest),
            crossed_at_unix_ns: Some(crossed_at_unix_ns),
            abort_reason: None,
            evidence: None,
        }
    }
    fn succeeded(
        dispatch: &Self,
        dispatch_digest: DigestV1,
        sequence: u64,
        evidence: AttemptEvidenceV1,
    ) -> Self {
        Self {
            schema_version: ATTEMPT_JOURNAL_SCHEMA_VERSION_V1,
            binding: dispatch.binding.clone(),
            state: AttemptStateV1::Succeeded,
            sequence,
            predecessor_entry_digest: Some(dispatch_digest),
            crossed_at_unix_ns: None,
            abort_reason: None,
            evidence: Some(evidence),
        }
    }
    fn failed(
        dispatch: &Self,
        dispatch_digest: DigestV1,
        sequence: u64,
        evidence: AttemptEvidenceV1,
    ) -> Self {
        Self {
            schema_version: ATTEMPT_JOURNAL_SCHEMA_VERSION_V1,
            binding: dispatch.binding.clone(),
            state: AttemptStateV1::Failed,
            sequence,
            predecessor_entry_digest: Some(dispatch_digest),
            crossed_at_unix_ns: None,
            abort_reason: None,
            evidence: Some(evidence),
        }
    }
    fn indeterminate(dispatch: &Self, dispatch_digest: DigestV1, sequence: u64) -> Self {
        Self {
            schema_version: ATTEMPT_JOURNAL_SCHEMA_VERSION_V1,
            binding: dispatch.binding.clone(),
            state: AttemptStateV1::Indeterminate,
            sequence,
            predecessor_entry_digest: Some(dispatch_digest),
            crossed_at_unix_ns: None,
            abort_reason: None,
            evidence: None,
        }
    }
    fn safe_to_retry(prepared: &Self, prepared_digest: DigestV1, sequence: u64) -> Self {
        Self {
            schema_version: ATTEMPT_JOURNAL_SCHEMA_VERSION_V1,
            binding: prepared.binding.clone(),
            state: AttemptStateV1::SafeToRetry,
            sequence,
            predecessor_entry_digest: Some(prepared_digest),
            crossed_at_unix_ns: None,
            abort_reason: None,
            evidence: None,
        }
    }
    fn aborted(
        prepared: &Self,
        prepared_digest: DigestV1,
        sequence: u64,
        reason: AttemptAbortReasonV1,
    ) -> Self {
        Self {
            schema_version: ATTEMPT_JOURNAL_SCHEMA_VERSION_V1,
            binding: prepared.binding.clone(),
            state: AttemptStateV1::Aborted,
            sequence,
            predecessor_entry_digest: Some(prepared_digest),
            crossed_at_unix_ns: None,
            abort_reason: Some(reason),
            evidence: None,
        }
    }
    pub fn validate(&self) -> Result<(), AttemptJournalErrorV1> {
        if self.schema_version != ATTEMPT_JOURNAL_SCHEMA_VERSION_V1 {
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::SchemaVersionMismatch,
                "attempt entry schema version is not supported",
            ));
        }
        self.binding.validate()?;
        let structural = match self.state {
            AttemptStateV1::Prepared => {
                self.sequence == 1
                    && self.predecessor_entry_digest.is_none()
                    && self.crossed_at_unix_ns.is_none()
                    && self.abort_reason.is_none()
            }
            AttemptStateV1::DispatchCrossed => {
                self.sequence >= 2
                    && self.predecessor_entry_digest.is_some()
                    && self.crossed_at_unix_ns.is_some()
                    && self.abort_reason.is_none()
            }
            AttemptStateV1::Succeeded
            | AttemptStateV1::Failed
            | AttemptStateV1::Indeterminate
            | AttemptStateV1::SafeToRetry => {
                self.sequence >= 2
                    && self.predecessor_entry_digest.is_some()
                    && self.crossed_at_unix_ns.is_none()
                    && self.abort_reason.is_none()
            }
            AttemptStateV1::Aborted => {
                self.sequence >= 2
                    && self.predecessor_entry_digest.is_some()
                    && self.crossed_at_unix_ns.is_none()
                    && self.abort_reason.is_some()
            }
        };
        if !structural {
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::SequenceMismatch,
                "attempt state and sequence commitments disagree",
            ));
        }
        let paired = match self.state {
            AttemptStateV1::Succeeded => {
                matches!(self.evidence, Some(AttemptEvidenceV1::Completion { .. }))
            }
            AttemptStateV1::Failed => {
                matches!(self.evidence, Some(AttemptEvidenceV1::Failure { .. }))
            }
            AttemptStateV1::Prepared
            | AttemptStateV1::DispatchCrossed
            | AttemptStateV1::Indeterminate
            | AttemptStateV1::SafeToRetry
            | AttemptStateV1::Aborted => self.evidence.is_none(),
        };
        if !paired {
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::InvalidEvidence,
                "attempt evidence does not match the entry state",
            ));
        }
        if let Some(evidence) = &self.evidence {
            evidence.validate()?;
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, AttemptJournalErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<DigestV1, AttemptJournalErrorV1> {
        Ok(domain_digest(
            ATTEMPT_ENTRY_DOMAIN_V1,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptRecoveryReceiptV1 {
    pub schema_version: u16,
    pub binding_digest: DigestV1,
    pub outcome: AttemptRecoveryOutcomeV1,
    pub terminal_entry_digest: DigestV1,
    pub terminal_state: AttemptStateV1,
}
impl AttemptRecoveryReceiptV1 {
    pub fn validate(&self) -> Result<(), AttemptJournalErrorV1> {
        if self.schema_version != ATTEMPT_RECEIPT_SCHEMA_VERSION_V1 {
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::SchemaVersionMismatch,
                "attempt recovery receipt schema version is not supported",
            ));
        }
        if self.binding_digest == DigestV1::ZERO
            || self.terminal_entry_digest == DigestV1::ZERO
            || !self.terminal_state.is_terminal()
        {
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::InvalidBinding,
                "attempt recovery receipt is incomplete",
            ));
        }
        let paired = matches!(
            (self.outcome, self.terminal_state),
            (
                AttemptRecoveryOutcomeV1::AlreadySucceeded,
                AttemptStateV1::Succeeded
            ) | (
                AttemptRecoveryOutcomeV1::AlreadyFailed,
                AttemptStateV1::Failed
            ) | (
                AttemptRecoveryOutcomeV1::AlreadyIndeterminate,
                AttemptStateV1::Indeterminate
            ) | (
                AttemptRecoveryOutcomeV1::AlreadyAborted,
                AttemptStateV1::Aborted
            ) | (
                AttemptRecoveryOutcomeV1::AlreadySafeToRetry,
                AttemptStateV1::SafeToRetry
            ) | (
                AttemptRecoveryOutcomeV1::ClassifiedSucceeded,
                AttemptStateV1::Succeeded
            ) | (
                AttemptRecoveryOutcomeV1::ClassifiedFailed,
                AttemptStateV1::Failed
            ) | (
                AttemptRecoveryOutcomeV1::ClassifiedIndeterminate,
                AttemptStateV1::Indeterminate
            ) | (
                AttemptRecoveryOutcomeV1::ClassifiedSafeToRetry,
                AttemptStateV1::SafeToRetry
            )
        );
        if !paired {
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::InvalidBinding,
                "attempt recovery receipt outcome and terminal state disagree",
            ));
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, AttemptJournalErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }
    pub fn digest(&self) -> Result<DigestV1, AttemptJournalErrorV1> {
        Ok(domain_digest(
            ATTEMPT_RECOVERY_DOMAIN_V1,
            &self.canonical_bytes()?,
        ))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AttemptFaultPlanV1 {
    crash_at: Option<AttemptBoundaryV1>,
    fired: bool,
}
impl AttemptFaultPlanV1 {
    pub const fn none() -> Self {
        Self {
            crash_at: None,
            fired: false,
        }
    }
    pub const fn crash_at(boundary: AttemptBoundaryV1) -> Self {
        Self {
            crash_at: Some(boundary),
            fired: false,
        }
    }
    fn hit(
        &mut self,
        boundary: AttemptBoundaryV1,
        entry_published: bool,
    ) -> Result<(), AttemptJournalErrorV1> {
        if !self.fired && self.crash_at == Some(boundary) {
            self.fired = true;
            return Err(AttemptJournalErrorV1::at(
                AttemptFailureCodeV1::InjectedCrash,
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
    before: AttemptBoundaryV1,
    file_sync: AttemptBoundaryV1,
    rename: AttemptBoundaryV1,
    dir_sync: AttemptBoundaryV1,
}
const PREPARE_BOUNDARIES: WriteBoundaries = WriteBoundaries {
    before: AttemptBoundaryV1::PrepareBeforeWrite,
    file_sync: AttemptBoundaryV1::PrepareAfterFileSync,
    rename: AttemptBoundaryV1::PrepareAfterRename,
    dir_sync: AttemptBoundaryV1::PrepareAfterDirectorySync,
};
const DISPATCH_CROSS_BOUNDARIES: WriteBoundaries = WriteBoundaries {
    before: AttemptBoundaryV1::DispatchCrossBeforeWrite,
    file_sync: AttemptBoundaryV1::DispatchCrossAfterFileSync,
    rename: AttemptBoundaryV1::DispatchCrossAfterRename,
    dir_sync: AttemptBoundaryV1::DispatchCrossAfterDirectorySync,
};
const SUCCEED_BOUNDARIES: WriteBoundaries = WriteBoundaries {
    before: AttemptBoundaryV1::SucceedBeforeWrite,
    file_sync: AttemptBoundaryV1::SucceedAfterFileSync,
    rename: AttemptBoundaryV1::SucceedAfterRename,
    dir_sync: AttemptBoundaryV1::SucceedAfterDirectorySync,
};
const FAIL_BOUNDARIES: WriteBoundaries = WriteBoundaries {
    before: AttemptBoundaryV1::FailBeforeWrite,
    file_sync: AttemptBoundaryV1::FailAfterFileSync,
    rename: AttemptBoundaryV1::FailAfterRename,
    dir_sync: AttemptBoundaryV1::FailAfterDirectorySync,
};
const INDETERMINATE_BOUNDARIES: WriteBoundaries = WriteBoundaries {
    before: AttemptBoundaryV1::IndeterminateBeforeWrite,
    file_sync: AttemptBoundaryV1::IndeterminateAfterFileSync,
    rename: AttemptBoundaryV1::IndeterminateAfterRename,
    dir_sync: AttemptBoundaryV1::IndeterminateAfterDirectorySync,
};
const ABORT_BOUNDARIES: WriteBoundaries = WriteBoundaries {
    before: AttemptBoundaryV1::AbortBeforeWrite,
    file_sync: AttemptBoundaryV1::AbortAfterFileSync,
    rename: AttemptBoundaryV1::AbortAfterRename,
    dir_sync: AttemptBoundaryV1::AbortAfterDirectorySync,
};
const RECOVER_BOUNDARIES: WriteBoundaries = WriteBoundaries {
    before: AttemptBoundaryV1::RecoverBeforeWrite,
    file_sync: AttemptBoundaryV1::RecoverAfterFileSync,
    rename: AttemptBoundaryV1::RecoverAfterRename,
    dir_sync: AttemptBoundaryV1::RecoverAfterDirectorySync,
};

/// Persist the Prepared entry before effect admission. Idempotent for the
/// same binding; a journal that already crossed dispatch or is terminal is
/// rejected with `AlreadyTerminal`.
pub fn prepare_attempt_v1(
    paths: &AttemptJournalPathsV1,
    binding: AttemptBindingV1,
) -> Result<AttemptEntryV1, AttemptJournalErrorV1> {
    prepare_attempt_with_fault_v1(paths, binding, &mut AttemptFaultPlanV1::none())
}
pub fn prepare_attempt_with_fault_v1(
    paths: &AttemptJournalPathsV1,
    binding: AttemptBindingV1,
    fault: &mut AttemptFaultPlanV1,
) -> Result<AttemptEntryV1, AttemptJournalErrorV1> {
    binding.validate()?;
    let chain = read_chain(paths)?;
    if let Some(current) = chain.last() {
        if current.value.state == AttemptStateV1::Prepared {
            if current.value.binding == binding {
                return Ok(current.value.clone());
            }
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::ImmutableEntryConflict,
                "attempt journal is already prepared with a different binding",
            ));
        }
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::AlreadyTerminal,
            "attempt journal already crossed dispatch or is terminal",
        ));
    }
    let prepared = AttemptEntryV1::prepared(binding, 1);
    write_once_entry(paths, 1, &prepared, PREPARE_BOUNDARIES, fault)?;
    Ok(prepared)
}

/// Persist the dispatch boundary immediately before the effect is dispatched.
/// The caller must hold the digest of the persisted Prepared entry.
pub fn mark_dispatch_crossed_v1(
    paths: &AttemptJournalPathsV1,
    prepared_entry_digest: DigestV1,
    crossed_at_unix_ns: u64,
) -> Result<AttemptEntryV1, AttemptJournalErrorV1> {
    mark_dispatch_crossed_with_fault_v1(
        paths,
        prepared_entry_digest,
        crossed_at_unix_ns,
        &mut AttemptFaultPlanV1::none(),
    )
}
pub fn mark_dispatch_crossed_with_fault_v1(
    paths: &AttemptJournalPathsV1,
    prepared_entry_digest: DigestV1,
    crossed_at_unix_ns: u64,
    fault: &mut AttemptFaultPlanV1,
) -> Result<AttemptEntryV1, AttemptJournalErrorV1> {
    let chain = read_chain(paths)?;
    let Some(current) = chain.last() else {
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::JournalMissing,
            "no prepared attempt exists to dispatch",
        ));
    };
    match current.value.state {
        AttemptStateV1::Prepared => {}
        AttemptStateV1::DispatchCrossed => {
            if current.value.predecessor_entry_digest == Some(prepared_entry_digest) {
                return Ok(current.value.clone());
            }
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::ReceiptMismatch,
                "dispatch already crossed for a different prepared entry",
            ));
        }
        state => {
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::AlreadyTerminal,
                format!(
                    "terminal attempt cannot be redispatched (current state {})",
                    state.as_str()
                ),
            ));
        }
    }
    if current.digest != prepared_entry_digest {
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::ReceiptMismatch,
            "prepared entry digest does not match the persisted journal",
        ));
    }
    let sequence = chain.len() as u64 + 1;
    let crossed = AttemptEntryV1::dispatch_crossed(
        &current.value,
        current.digest,
        sequence,
        crossed_at_unix_ns,
    );
    write_once_entry(paths, sequence, &crossed, DISPATCH_CROSS_BOUNDARIES, fault)?;
    Ok(crossed)
}

/// Persist authoritative completion evidence as a terminal Succeeded entry.
pub fn mark_succeeded_v1(
    paths: &AttemptJournalPathsV1,
    dispatch_entry_digest: DigestV1,
    receipt_digest: DigestV1,
    observed_at_unix_ns: u64,
) -> Result<AttemptEntryV1, AttemptJournalErrorV1> {
    mark_succeeded_with_fault_v1(
        paths,
        dispatch_entry_digest,
        receipt_digest,
        observed_at_unix_ns,
        &mut AttemptFaultPlanV1::none(),
    )
}
pub fn mark_succeeded_with_fault_v1(
    paths: &AttemptJournalPathsV1,
    dispatch_entry_digest: DigestV1,
    receipt_digest: DigestV1,
    observed_at_unix_ns: u64,
    fault: &mut AttemptFaultPlanV1,
) -> Result<AttemptEntryV1, AttemptJournalErrorV1> {
    if receipt_digest == DigestV1::ZERO {
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::InvalidEvidence,
            "completion receipt digest must be nonzero",
        ));
    }
    let evidence = AttemptEvidenceV1::Completion {
        receipt_digest,
        observed_at_unix_ns,
    };
    mark_terminal_with_fault_v1(
        paths,
        dispatch_entry_digest,
        AttemptStateV1::Succeeded,
        Some(evidence),
        SUCCEED_BOUNDARIES,
        fault,
    )
}

/// Persist authoritative failure evidence as a terminal Failed entry.
pub fn mark_failed_v1(
    paths: &AttemptJournalPathsV1,
    dispatch_entry_digest: DigestV1,
    failure_receipt_digest: DigestV1,
    observed_at_unix_ns: u64,
) -> Result<AttemptEntryV1, AttemptJournalErrorV1> {
    mark_failed_with_fault_v1(
        paths,
        dispatch_entry_digest,
        failure_receipt_digest,
        observed_at_unix_ns,
        &mut AttemptFaultPlanV1::none(),
    )
}
pub fn mark_failed_with_fault_v1(
    paths: &AttemptJournalPathsV1,
    dispatch_entry_digest: DigestV1,
    failure_receipt_digest: DigestV1,
    observed_at_unix_ns: u64,
    fault: &mut AttemptFaultPlanV1,
) -> Result<AttemptEntryV1, AttemptJournalErrorV1> {
    if failure_receipt_digest == DigestV1::ZERO {
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::InvalidEvidence,
            "failure receipt digest must be nonzero",
        ));
    }
    let evidence = AttemptEvidenceV1::Failure {
        failure_receipt_digest,
        observed_at_unix_ns,
    };
    mark_terminal_with_fault_v1(
        paths,
        dispatch_entry_digest,
        AttemptStateV1::Failed,
        Some(evidence),
        FAIL_BOUNDARIES,
        fault,
    )
}

/// Persist a terminal Indeterminate entry: the effect may have run and no
/// authoritative evidence exists. The attempt is never redispatched.
pub fn mark_indeterminate_v1(
    paths: &AttemptJournalPathsV1,
    dispatch_entry_digest: DigestV1,
) -> Result<AttemptEntryV1, AttemptJournalErrorV1> {
    mark_indeterminate_with_fault_v1(
        paths,
        dispatch_entry_digest,
        &mut AttemptFaultPlanV1::none(),
    )
}
pub fn mark_indeterminate_with_fault_v1(
    paths: &AttemptJournalPathsV1,
    dispatch_entry_digest: DigestV1,
    fault: &mut AttemptFaultPlanV1,
) -> Result<AttemptEntryV1, AttemptJournalErrorV1> {
    mark_terminal_with_fault_v1(
        paths,
        dispatch_entry_digest,
        AttemptStateV1::Indeterminate,
        None,
        INDETERMINATE_BOUNDARIES,
        fault,
    )
}

fn mark_terminal_with_fault_v1(
    paths: &AttemptJournalPathsV1,
    dispatch_entry_digest: DigestV1,
    terminal_state: AttemptStateV1,
    evidence: Option<AttemptEvidenceV1>,
    boundaries: WriteBoundaries,
    fault: &mut AttemptFaultPlanV1,
) -> Result<AttemptEntryV1, AttemptJournalErrorV1> {
    let chain = read_chain(paths)?;
    let Some(current) = chain.last() else {
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::JournalMissing,
            "no dispatched attempt exists to resolve",
        ));
    };
    if current.value.state == terminal_state {
        let dispatch = &chain[chain.len() - 2];
        if dispatch.digest != dispatch_entry_digest {
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::ReceiptMismatch,
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
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::ImmutableEntryConflict,
            "terminal entry already exists with different evidence",
        ));
    }
    if current.value.state != AttemptStateV1::DispatchCrossed {
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::InvalidTransition,
            format!(
                "cannot transition from {} to {}",
                current.value.state.as_str(),
                terminal_state.as_str()
            ),
        ));
    }
    if current.digest != dispatch_entry_digest {
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::ReceiptMismatch,
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
    dispatch: &AttemptEntryV1,
    dispatch_digest: DigestV1,
    sequence: u64,
    state: AttemptStateV1,
    evidence: Option<AttemptEvidenceV1>,
) -> AttemptEntryV1 {
    match state {
        AttemptStateV1::Succeeded => AttemptEntryV1::succeeded(
            dispatch,
            dispatch_digest,
            sequence,
            evidence.expect("succeeded requires completion evidence"),
        ),
        AttemptStateV1::Failed => AttemptEntryV1::failed(
            dispatch,
            dispatch_digest,
            sequence,
            evidence.expect("failed requires failure evidence"),
        ),
        AttemptStateV1::Indeterminate => {
            AttemptEntryV1::indeterminate(dispatch, dispatch_digest, sequence)
        }
        _ => unreachable!("terminal_entry only builds succeeded, failed, indeterminate"),
    }
}

/// Abort a Prepared attempt before dispatch. Terminal Aborted entries are
/// immutable; aborting an already-aborted attempt is idempotent.
pub fn abort_attempt_v1(
    paths: &AttemptJournalPathsV1,
    prepared_entry_digest: DigestV1,
) -> Result<AttemptEntryV1, AttemptJournalErrorV1> {
    abort_attempt_with_fault_v1(
        paths,
        prepared_entry_digest,
        &mut AttemptFaultPlanV1::none(),
    )
}
pub fn abort_attempt_with_fault_v1(
    paths: &AttemptJournalPathsV1,
    prepared_entry_digest: DigestV1,
    fault: &mut AttemptFaultPlanV1,
) -> Result<AttemptEntryV1, AttemptJournalErrorV1> {
    let chain = read_chain(paths)?;
    let Some(current) = chain.last() else {
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::JournalMissing,
            "no prepared attempt exists to abort",
        ));
    };
    match current.value.state {
        AttemptStateV1::Prepared => {
            if current.digest != prepared_entry_digest {
                return Err(AttemptJournalErrorV1::new(
                    AttemptFailureCodeV1::ReceiptMismatch,
                    "prepared entry digest does not match the persisted journal",
                ));
            }
            let sequence = chain.len() as u64 + 1;
            let aborted = AttemptEntryV1::aborted(
                &current.value,
                current.digest,
                sequence,
                AttemptAbortReasonV1::ExplicitAbort,
            );
            write_once_entry(paths, sequence, &aborted, ABORT_BOUNDARIES, fault)?;
            Ok(aborted)
        }
        AttemptStateV1::Aborted => {
            let prepared = &chain[chain.len() - 2];
            if prepared.digest != prepared_entry_digest {
                return Err(AttemptJournalErrorV1::new(
                    AttemptFailureCodeV1::ReceiptMismatch,
                    "prepared entry digest does not match the persisted journal",
                ));
            }
            Ok(current.value.clone())
        }
        state => Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::InvalidTransition,
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
pub fn recover_attempt_v1(
    paths: &AttemptJournalPathsV1,
    expected: &AttemptBindingV1,
    evidence: Option<AttemptEvidenceV1>,
) -> Result<AttemptRecoveryReceiptV1, AttemptJournalErrorV1> {
    recover_attempt_with_fault_v1(paths, expected, evidence, &mut AttemptFaultPlanV1::none())
}
pub fn recover_attempt_with_fault_v1(
    paths: &AttemptJournalPathsV1,
    expected: &AttemptBindingV1,
    evidence: Option<AttemptEvidenceV1>,
    fault: &mut AttemptFaultPlanV1,
) -> Result<AttemptRecoveryReceiptV1, AttemptJournalErrorV1> {
    expected.validate()?;
    let chain = read_chain(paths)?;
    let Some(current) = chain.last() else {
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::JournalMissing,
            "no attempt journal exists to recover",
        ));
    };
    if chain[0].value.binding != *expected {
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::InvalidBinding,
            "persisted attempt binding differs from recovery expectation",
        ));
    }
    let binding_digest = expected.digest()?;
    match current.value.state {
        AttemptStateV1::Succeeded => make_recovery_receipt(
            binding_digest,
            AttemptRecoveryOutcomeV1::AlreadySucceeded,
            &current.value,
        ),
        AttemptStateV1::Failed => make_recovery_receipt(
            binding_digest,
            AttemptRecoveryOutcomeV1::AlreadyFailed,
            &current.value,
        ),
        AttemptStateV1::Indeterminate => make_recovery_receipt(
            binding_digest,
            AttemptRecoveryOutcomeV1::AlreadyIndeterminate,
            &current.value,
        ),
        AttemptStateV1::SafeToRetry => make_recovery_receipt(
            binding_digest,
            AttemptRecoveryOutcomeV1::AlreadySafeToRetry,
            &current.value,
        ),
        AttemptStateV1::Aborted => make_recovery_receipt(
            binding_digest,
            AttemptRecoveryOutcomeV1::AlreadyAborted,
            &current.value,
        ),
        AttemptStateV1::Prepared => {
            // Dispatch never crossed, so the journal proves the effect never
            // ran: classify SafeToRetry and terminate the journal. A fresh
            // attempt may be admitted elsewhere; this journal can never
            // dispatch. Supplied evidence is ignored — an attempt that never
            // ran has no outcome evidence.
            let sequence = chain.len() as u64 + 1;
            let safe_to_retry =
                AttemptEntryV1::safe_to_retry(&current.value, current.digest, sequence);
            write_once_entry(paths, sequence, &safe_to_retry, RECOVER_BOUNDARIES, fault)?;
            make_recovery_receipt(
                binding_digest,
                AttemptRecoveryOutcomeV1::ClassifiedSafeToRetry,
                &safe_to_retry,
            )
        }
        AttemptStateV1::DispatchCrossed => {
            let sequence = chain.len() as u64 + 1;
            let (outcome, terminal) = match evidence {
                Some(evidence @ AttemptEvidenceV1::Completion { .. }) => (
                    AttemptRecoveryOutcomeV1::ClassifiedSucceeded,
                    AttemptEntryV1::succeeded(&current.value, current.digest, sequence, evidence),
                ),
                Some(evidence @ AttemptEvidenceV1::Failure { .. }) => (
                    AttemptRecoveryOutcomeV1::ClassifiedFailed,
                    AttemptEntryV1::failed(&current.value, current.digest, sequence, evidence),
                ),
                None => (
                    AttemptRecoveryOutcomeV1::ClassifiedIndeterminate,
                    AttemptEntryV1::indeterminate(&current.value, current.digest, sequence),
                ),
            };
            write_once_entry(paths, sequence, &terminal, RECOVER_BOUNDARIES, fault)?;
            make_recovery_receipt(binding_digest, outcome, &terminal)
        }
    }
}

/// Read the current (latest) entry, if any. Validates the whole chain; a torn
/// or non-contiguous journal fails loudly.
pub fn read_current_attempt_v1(
    paths: &AttemptJournalPathsV1,
) -> Result<Option<AttemptEntryV1>, AttemptJournalErrorV1> {
    Ok(read_chain(paths)?.last().map(|entry| entry.value.clone()))
}

/// Read a single entry by sequence without chain validation.
pub fn read_attempt_entry_v1(
    paths: &AttemptJournalPathsV1,
    sequence: u64,
) -> Result<AttemptEntryV1, AttemptJournalErrorV1> {
    Ok(read_canonical_entry(paths, sequence)?.value)
}

fn make_recovery_receipt(
    binding_digest: DigestV1,
    outcome: AttemptRecoveryOutcomeV1,
    terminal: &AttemptEntryV1,
) -> Result<AttemptRecoveryReceiptV1, AttemptJournalErrorV1> {
    let receipt = AttemptRecoveryReceiptV1 {
        schema_version: ATTEMPT_RECEIPT_SCHEMA_VERSION_V1,
        binding_digest,
        outcome,
        terminal_entry_digest: terminal.digest()?,
        terminal_state: terminal.state,
    };
    receipt.validate()?;
    Ok(receipt)
}

fn is_valid_transition(from: AttemptStateV1, to: AttemptStateV1) -> bool {
    matches!(
        (from, to),
        (AttemptStateV1::Prepared, AttemptStateV1::DispatchCrossed)
            | (AttemptStateV1::Prepared, AttemptStateV1::Aborted)
            | (AttemptStateV1::Prepared, AttemptStateV1::SafeToRetry)
            | (AttemptStateV1::DispatchCrossed, AttemptStateV1::Succeeded)
            | (AttemptStateV1::DispatchCrossed, AttemptStateV1::Failed)
            | (
                AttemptStateV1::DispatchCrossed,
                AttemptStateV1::Indeterminate
            )
    )
}

struct CanonicalRead<T> {
    value: T,
    digest: DigestV1,
}

fn read_chain(
    paths: &AttemptJournalPathsV1,
) -> Result<Vec<CanonicalRead<AttemptEntryV1>>, AttemptJournalErrorV1> {
    let mut entries: Vec<CanonicalRead<AttemptEntryV1>> = Vec::new();
    let mut sequence: u64 = 1;
    while sequence <= ATTEMPT_JOURNAL_MAX_ENTRIES_V1 {
        match read_canonical_entry(paths, sequence) {
            Ok(entry) => entries.push(entry),
            Err(error) if error.code == AttemptFailureCodeV1::EntryMissing => break,
            Err(error) => return Err(error),
        }
        sequence += 1;
    }
    if entries.is_empty() {
        return Ok(entries);
    }
    for probe in sequence..=ATTEMPT_JOURNAL_MAX_ENTRIES_V1 {
        if read_canonical_entry(paths, probe).is_ok() {
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::SequenceMismatch,
                "attempt entry chain is not contiguous",
            ));
        }
    }
    for (index, entry) in entries.iter().enumerate() {
        if entry.value.sequence != index as u64 + 1 {
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::SequenceMismatch,
                "attempt entry sequence does not match its chain position",
            ));
        }
        if index > 0 {
            let previous = &entries[index - 1];
            if entry.value.predecessor_entry_digest != Some(previous.digest) {
                return Err(AttemptJournalErrorV1::new(
                    AttemptFailureCodeV1::SequenceMismatch,
                    "attempt entry predecessor digest does not chain",
                ));
            }
            if !is_valid_transition(previous.value.state, entry.value.state) {
                return Err(AttemptJournalErrorV1::new(
                    AttemptFailureCodeV1::InvalidTransition,
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
    paths: &AttemptJournalPathsV1,
    sequence: u64,
) -> Result<CanonicalRead<AttemptEntryV1>, AttemptJournalErrorV1> {
    let read = read_canonical::<AttemptEntryV1>(
        &paths.entry_path(sequence),
        AttemptFailureCodeV1::EntryMissing,
        ATTEMPT_ENTRY_DOMAIN_V1,
    )?;
    read.value.validate()?;
    Ok(read)
}

fn read_canonical<T>(
    path: &Path,
    missing: AttemptFailureCodeV1,
    domain: &[u8],
) -> Result<CanonicalRead<T>, AttemptJournalErrorV1>
where
    T: DeserializeOwned + Serialize,
{
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(AttemptJournalErrorV1::new(
                missing,
                "required entry is absent",
            ));
        }
        Err(error) => {
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::IoBeforePublish,
                format!("entry stat failed: {error}"),
            ));
        }
    };
    if !metadata.file_type().is_file() {
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::TornOrNoncanonicalRecord,
            "entry is not a regular file",
        ));
    }
    if metadata.len() > ATTEMPT_JOURNAL_MAX_RECORD_BYTES_V1 {
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::RecordTooLarge,
            "entry exceeds the frozen byte bound",
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::IoBeforePublish,
            format!("entry read failed: {error}"),
        )
    })?;
    let value: T = serde_json::from_slice(&bytes).map_err(|error| {
        AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::TornOrNoncanonicalRecord,
            format!("entry decode failed: {error}"),
        )
    })?;
    let canonical = canonical_bytes(&value)?;
    if canonical != bytes {
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::TornOrNoncanonicalRecord,
            "entry bytes are not canonical JSON",
        ));
    }
    Ok(CanonicalRead {
        value,
        digest: domain_digest(domain, &canonical),
    })
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, AttemptJournalErrorV1> {
    let value = serde_json::to_value(value).map_err(|error| {
        AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::InvalidBinding,
            format!("entry serialization failed: {error}"),
        )
    })?;
    let bytes = canonical_json(&value).into_bytes();
    if bytes.len() as u64 > ATTEMPT_JOURNAL_MAX_RECORD_BYTES_V1 {
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::RecordTooLarge,
            "canonical entry exceeds the frozen byte bound",
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

fn write_once_entry(
    paths: &AttemptJournalPathsV1,
    sequence: u64,
    entry: &AttemptEntryV1,
    boundaries: WriteBoundaries,
    fault: &mut AttemptFaultPlanV1,
) -> Result<(), AttemptJournalErrorV1> {
    if sequence > ATTEMPT_JOURNAL_MAX_ENTRIES_V1 {
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::TooManyEntries,
            "attempt journal entry bound exceeded",
        ));
    }
    let bytes = entry.canonical_bytes()?;
    let path = paths.entry_path(sequence);
    match fs::read(&path) {
        Ok(existing) if existing == bytes => return Ok(()),
        Ok(_) => {
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::ImmutableEntryConflict,
                "immutable entry already exists with different bytes",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(AttemptJournalErrorV1::new(
                AttemptFailureCodeV1::IoBeforePublish,
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
    fault: &mut AttemptFaultPlanV1,
) -> Result<(), AttemptJournalErrorV1> {
    if bytes.len() as u64 > ATTEMPT_JOURNAL_MAX_RECORD_BYTES_V1 {
        return Err(AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::RecordTooLarge,
            "write exceeds the frozen record byte bound",
        ));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| io_before(boundaries.before, error))?;
    let file_name = path.file_name().ok_or_else(|| {
        AttemptJournalErrorV1::new(
            AttemptFailureCodeV1::InvalidBinding,
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
            AttemptJournalErrorV1::at(
                AttemptFailureCodeV1::DirectorySyncFailedAfterPublish,
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

fn io_before(boundary: AttemptBoundaryV1, error: io::Error) -> AttemptJournalErrorV1 {
    AttemptJournalErrorV1::at(
        AttemptFailureCodeV1::IoBeforePublish,
        boundary,
        false,
        format!("durable write failed before publication: {error}"),
    )
}

/// Machine-readable contract summary used by conformance generators.
pub fn attempt_journal_contract_v1() -> serde_json::Value {
    json!({
        "schema_version": ATTEMPT_JOURNAL_SCHEMA_VERSION_V1,
        "binding_schema_version": ATTEMPT_BINDING_SCHEMA_VERSION_V1,
        "receipt_schema_version": ATTEMPT_RECEIPT_SCHEMA_VERSION_V1,
        "max_record_bytes": ATTEMPT_JOURNAL_MAX_RECORD_BYTES_V1,
        "max_entries": ATTEMPT_JOURNAL_MAX_ENTRIES_V1,
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

#[cfg(test)]
#[path = "../../../tests/rust/zero-store/unit/attempt_journal.rs"]
mod tests;
