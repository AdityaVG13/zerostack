//! Identity-kernel runtime wiring (ZS-KERNEL-003/006/008, V6-R6).
//!
//! The W1 identity kernel types ([`EventLogV1`], [`ProjectSuccessorCasV1`],
//! [`PayloadFormationReceiptV1`]) are verified library contracts consumed
//! nowhere outside zero-abi. This module is the hub-side runtime mechanism
//! that enforces them at join points:
//!
//! - **ZS-KERNEL-006**: [`KernelEventJournalV1`] persists the parent-rooted
//!   event chain through a [`JournalStore`] (append-only, one canonical JSON
//!   record per line), replays fail-closed on open (torn tail, missing or
//!   reordered records, tampered lines), and verifies the replayed head
//!   against a persisted sealed head. The typed boundary is
//!   [`EventClassV1`]: all nine authoritative event classes -- state
//!   transitions, evidence observations, cache decisions, executions,
//!   verification, authority issuance, commits, rollbacks, and resource
//!   charges -- are the only classes a journal can append.
//! - **ZS-KERNEL-008**: [`ProjectRootGateV1`] drives [`ProjectSuccessorCasV1`]
//!   through the verify -> authorize -> commit phases. Verify and authorize
//!   are pure observations; commit is the ONLY mutation and emits a
//!   [`SuccessorRecordV1`] receipt plus a `Commit` journal event. Stale
//!   handles and double-commits fail loud with an unchanged-successor
//!   receipt. [`RootGateFaultV1`] injects runtime faults around
//!   authorize/commit so crash boundaries are exercised mechanically.
//! - **ZS-KERNEL-003**: [`CacheAdmissionGateV1`] gates cache admission on a
//!   [`PayloadFormationReceiptV1`]: the offered receipt must be the exact
//!   rooted object it claims to be, the payload/contract bindings must hold,
//!   and the current dependency roots must still exactly match the
//!   formation-time set ([`PayloadFormationReceiptV1::verify_against`]).
//!   Dependency mutation revokes reuse; every decision is sealed as a
//!   [`CacheAdmissionRecordV1`] and journaled as a `CacheDecision` event.
//!
//! All roots are content-derived (sha256 over canonical bytes), never
//! wall-clock. Invariant violations return loud [`KernelRuntimeError`]s and,
//! where a decision is involved, a sealed receipt.
//!
//! **Residual wiring (next wave, not this one):** zero-store
//! `session_wal`/`durable_journal` and zero-gate `transaction.rs` adopt
//! [`KernelEventJournalV1`] + [`ProjectRootGateV1`]; zero-store's
//! `cache_entry` admission path adopts [`CacheAdmissionGateV1`]. This module
//! stays library-level with in-memory and file-backed fixtures so the
//! semantics are proven before the store/gate join points are cut.

use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use zero_abi::identity::EventClassV1;
use zero_abi::{
    DigestV1, EventLogV1, EventRecordV1, IdentityErrorV1, ObjectClassV1,
    PayloadFormationReceiptV1, ProjectSuccessorCasV1, ROOTED_ABI_VERSION,
    SuccessorOutcomeV1, SuccessorRecordV1, SuccessorUnchangedReasonV1, canonical_json,
    event_log_genesis, verify_object_root,
};
use zero_abi::cache_entry::CacheKeyV1;

/// Runtime mechanism version domain for the identity kernel.
pub const KERNEL_RUNTIME_VERSION_V1: &str = "zerostack.kernel-runtime.v1";
/// File name for the append-only records of a [`FileEventJournalStore`].
pub const EVENT_JOURNAL_RECORDS_FILE_V1: &str = "records.jsonl";
/// File name for the sealed-head marker of a [`FileEventJournalStore`].
pub const EVENT_JOURNAL_SEALED_HEAD_FILE_V1: &str = "sealed_head";
/// Domain tag bound into every [`CacheAdmissionRecordV1`] root.
pub const CACHE_ADMISSION_DOMAIN_V1: &[u8] = b"zerostack.cache-admission.v1\0";

/// Loud, fail-closed error for identity-kernel runtime join points.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KernelRuntimeError {
    /// Underlying I/O failure (journal persistence).
    Io(String),
    /// Wrapped identity-kernel failure (rooting, chaining, validation).
    Identity(IdentityErrorV1),
    /// A persisted journal record cannot be parsed or does not chain.
    InvalidJournalRecord { seq: u64, detail: String },
    /// The last persisted line is a partial write: the process died mid-append.
    TornJournalTail { seq: u64 },
    /// The persisted chain replays to a head different from the sealed head.
    JournalHeadMismatch { sealed: DigestV1, replayed: DigestV1 },
    /// Cache admission refused; the sealed decision record is the receipt.
    AdmissionRefused { reason: String, record: CacheAdmissionRecordV1 },
    /// A mutation was attempted without an authorized session.
    Unauthorized { detail: String },
    /// The declared parent root no longer matches the current project root.
    /// The unchanged successor record is the receipt for the violation.
    StaleProjectHandle {
        declared_parent: DigestV1,
        current: DigestV1,
        receipt: SuccessorRecordV1,
    },
    /// The verified successor root equals the current root: nothing changed.
    NoVerifiedChange { receipt: SuccessorRecordV1 },
    /// A configured fault fired at the named phase (fault-injection tests).
    FaultInjected { phase: &'static str },
}

impl fmt::Display for KernelRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(detail) => write!(formatter, "kernel runtime io failure: {detail}"),
            Self::Identity(error) => write!(formatter, "identity kernel failure: {error}"),
            Self::InvalidJournalRecord { seq, detail } => {
                write!(formatter, "invalid journal record at seq {seq}: {detail}")
            }
            Self::TornJournalTail { seq } => write!(
                formatter,
                "torn journal tail at seq {seq}: partial record write (process killed mid-append)"
            ),
            Self::JournalHeadMismatch { sealed, replayed } => write!(
                formatter,
                "journal head mismatch: sealed {sealed}, replayed {replayed} (torn tail or tampered history)"
            ),
            Self::AdmissionRefused { reason, .. } => {
                write!(formatter, "cache admission refused: {reason}")
            }
            Self::Unauthorized { detail } => write!(formatter, "unauthorized: {detail}"),
            Self::StaleProjectHandle { declared_parent, current, .. } => write!(
                formatter,
                "stale project handle: declared parent {declared_parent}, current root {current}"
            ),
            Self::NoVerifiedChange { .. } => {
                write!(formatter, "no verified change: successor root equals current root")
            }
            Self::FaultInjected { phase } => {
                write!(formatter, "injected fault fired at phase {phase:?}")
            }
        }
    }
}

impl Error for KernelRuntimeError {}

impl From<IdentityErrorV1> for KernelRuntimeError {
    fn from(error: IdentityErrorV1) -> Self {
        Self::Identity(error)
    }
}

impl From<std::io::Error> for KernelRuntimeError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

// ---------------------------------------------------------------------------
// Journal persistence (ZS-KERNEL-006).
// ---------------------------------------------------------------------------

/// Persistence surface for the authoritative event chain. One writer per
/// journal; the single-writer law is enforced by the store implementations.
pub trait JournalStore {
    /// Load every persisted record, in append order.
    fn load_records(&self) -> Result<Vec<EventRecordV1>, KernelRuntimeError>;
    /// Persist one record durably (append + sync). This is the atomic
    /// durability point for an event: after this returns, a killed process
    /// replays the record.
    fn persist_record(&mut self, record: &EventRecordV1) -> Result<(), KernelRuntimeError>;
    /// Load the sealed-head marker, if one was written.
    fn load_sealed_head(&self) -> Result<Option<DigestV1>, KernelRuntimeError>;
    /// Persist the sealed-head marker.
    fn persist_sealed_head(&mut self, head: DigestV1) -> Result<(), KernelRuntimeError>;
}

/// In-memory journal store: fixtures and dry-run wiring.
#[derive(Clone, Debug, Default)]
pub struct InMemoryJournalStore {
    records: Vec<EventRecordV1>,
    sealed_head: Option<DigestV1>,
}

impl InMemoryJournalStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl JournalStore for InMemoryJournalStore {
    fn load_records(&self) -> Result<Vec<EventRecordV1>, KernelRuntimeError> {
        Ok(self.records.clone())
    }

    fn persist_record(&mut self, record: &EventRecordV1) -> Result<(), KernelRuntimeError> {
        self.records.push(record.clone());
        Ok(())
    }

    fn load_sealed_head(&self) -> Result<Option<DigestV1>, KernelRuntimeError> {
        Ok(self.sealed_head)
    }

    fn persist_sealed_head(&mut self, head: DigestV1) -> Result<(), KernelRuntimeError> {
        self.sealed_head = Some(head);
        Ok(())
    }
}

/// File-backed journal store: `records.jsonl` (one canonical JSON record per
/// line) plus a `sealed_head` marker. A partial final line is a torn tail
/// from a killed process and fails closed; a malformed earlier line is an
/// invalid record. Records are synced on append.
#[derive(Clone, Debug)]
pub struct FileEventJournalStore {
    dir: PathBuf,
}

impl FileEventJournalStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn records_path(&self) -> PathBuf {
        self.dir.join(EVENT_JOURNAL_RECORDS_FILE_V1)
    }

    fn sealed_head_path(&self) -> PathBuf {
        self.dir.join(EVENT_JOURNAL_SEALED_HEAD_FILE_V1)
    }
}

impl JournalStore for FileEventJournalStore {
    fn load_records(&self) -> Result<Vec<EventRecordV1>, KernelRuntimeError> {
        let path = self.records_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&path)?;
        let reader = BufReader::new(file);
        let mut records = Vec::new();
        let mut lines = reader.lines().peekable();
        while let Some(line) = lines.next() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let is_last = lines.peek().is_none();
            match serde_json::from_str::<EventRecordV1>(&line) {
                Ok(record) => records.push(record),
                Err(error) => {
                    if is_last {
                        // Partial final write from a killed process.
                        return Err(KernelRuntimeError::TornJournalTail {
                            seq: records.len() as u64,
                        });
                    }
                    return Err(KernelRuntimeError::InvalidJournalRecord {
                        seq: records.len() as u64,
                        detail: error.to_string(),
                    });
                }
            }
        }
        Ok(records)
    }

    fn persist_record(&mut self, record: &EventRecordV1) -> Result<(), KernelRuntimeError> {
        fs::create_dir_all(&self.dir)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.records_path())?;
        let line = serde_json::to_string(record)
            .map_err(|error| KernelRuntimeError::Io(error.to_string()))?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        Ok(())
    }

    fn load_sealed_head(&self) -> Result<Option<DigestV1>, KernelRuntimeError> {
        let path = self.sealed_head_path();
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&path)?;
        let head = content.trim();
        if head.is_empty() {
            return Err(KernelRuntimeError::Io(
                "sealed head marker is empty".to_owned(),
            ));
        }
        DigestV1::from_hex(head)
            .map(Some)
            .map_err(|error| KernelRuntimeError::Io(error.to_string()))
    }

    fn persist_sealed_head(&mut self, head: DigestV1) -> Result<(), KernelRuntimeError> {
        fs::create_dir_all(&self.dir)?;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(self.sealed_head_path())?;
        file.write_all(head.to_hex().as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        Ok(())
    }
}

fn map_replay_error(error: IdentityErrorV1) -> KernelRuntimeError {
    match error {
        IdentityErrorV1::ReorderedEventLog {
            seq,
            expected_parent,
            actual_parent,
        } => KernelRuntimeError::InvalidJournalRecord {
            seq,
            detail: format!(
                "parent chaining broken: expected {expected_parent}, got {actual_parent}"
            ),
        },
        other => KernelRuntimeError::Identity(other),
    }
}

/// Durable, parent-rooted authoritative event journal (ZS-KERNEL-006).
///
/// The in-memory [`EventLogV1`] is the chain authority; every append is
/// persisted through the [`JournalStore`] BEFORE the in-memory chain is
/// updated, so a killed process never observes an event that was not
/// durably written. Opening replays all persisted records fail-closed and,
/// when a sealed head exists, verifies the replayed head against it.
#[derive(Clone, Debug)]
pub struct KernelEventJournalV1<S: JournalStore> {
    store: S,
    log: EventLogV1,
}

impl<S: JournalStore> KernelEventJournalV1<S> {
    /// Open a journal from its store. Fails closed on torn tails, malformed
    /// or reordered records, and sealed-head mismatches.
    pub fn open(store: S) -> Result<Self, KernelRuntimeError> {
        let records = store.load_records()?;
        // Replay first: missing/reordered/tampered records cannot chain.
        EventLogV1::replay(&records).map_err(map_replay_error)?;
        let log = EventLogV1::from_records(records);
        let journal = Self { store, log };
        if let Some(sealed) = journal.store.load_sealed_head()? {
            journal.log.verify_chain_against(sealed).map_err(|error| match error {
                IdentityErrorV1::TornEventLog {
                    seq: _,
                    expected,
                    actual,
                } => KernelRuntimeError::JournalHeadMismatch {
                    sealed: expected,
                    replayed: actual,
                },
                other => KernelRuntimeError::Identity(other),
            })?;
        }
        Ok(journal)
    }

    /// Append one typed event, chained to the current head. The record is
    /// persisted durably first; the in-memory chain is updated only after
    /// the write succeeded.
    pub fn append(
        &mut self,
        class: EventClassV1,
        payload_root: impl Into<String>,
        authority: impl Into<String>,
    ) -> Result<EventRecordV1, KernelRuntimeError> {
        let payload_root = payload_root.into();
        let authority = authority.into();
        let parent_root = self.log.head()?;
        let seq = self.log.records().len() as u64;
        let record =
            EventRecordV1::new(seq, parent_root, class.as_str(), payload_root, authority)?;
        self.store.persist_record(&record)?;
        let chained = self.log.append(class.as_str(), record.payload_root.clone(), record.authority.clone())?;
        if chained != record {
            return Err(KernelRuntimeError::Io(
                "journal chain diverged from persisted record".to_owned(),
            ));
        }
        Ok(record)
    }

    /// Persist the current head as the sealed head. A later open verifies the
    /// replayed chain against it, detecting torn tails.
    pub fn seal(&mut self) -> Result<DigestV1, KernelRuntimeError> {
        let head = self.log.head()?;
        self.store.persist_sealed_head(head)?;
        Ok(head)
    }

    /// The current chain head (genesis when empty).
    pub fn head(&self) -> Result<DigestV1, KernelRuntimeError> {
        Ok(self.log.head()?)
    }

    /// Verify the full chain; returns the replayed head. Fails closed on
    /// missing, reordered, or tampered records.
    pub fn verify_chain(&self) -> Result<DigestV1, KernelRuntimeError> {
        self.log.verify_chain().map_err(map_replay_error)
    }

    /// The chained records in append order.
    pub fn records(&self) -> &[EventRecordV1] {
        self.log.records()
    }

    /// The underlying chain authority.
    pub fn log(&self) -> &EventLogV1 {
        &self.log
    }

    /// Reconstruct the project root from the journal: the successor root of
    /// the last `Commit` event. This is the killed-process recovery path --
    /// the journal, not process memory, is the CAS state.
    pub fn current_project_root(&self) -> Result<Option<DigestV1>, KernelRuntimeError> {
        let mut root = None;
        for record in self.log.records() {
            if record.event_type != EventClassV1::Commit.as_str() {
                continue;
            }
            root = Some(DigestV1::from_hex(&record.payload_root).map_err(|error| {
                KernelRuntimeError::InvalidJournalRecord {
                    seq: record.seq,
                    detail: format!(
                        "commit payload_root is not a successor root: {error}"
                    ),
                }
            })?);
        }
        Ok(root)
    }

    /// Record a cache admission decision (admitted or refused) as a
    /// `CacheDecision` event whose payload is the sealed decision record root.
    pub fn append_cache_decision(
        &mut self,
        record: &CacheAdmissionRecordV1,
    ) -> Result<EventRecordV1, KernelRuntimeError> {
        self.append(EventClassV1::CacheDecision, record.record_root(), "cache-gate")
    }
}

// ---------------------------------------------------------------------------
// Project-root gate (ZS-KERNEL-008).
// ---------------------------------------------------------------------------

/// Runtime fault injection around the verify -> authorize -> commit loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootGateFaultV1 {
    /// `authorize` always fails; the session can never reach commit.
    AuthorizationRefused,
    /// `commit` dies before any mutation: CAS unchanged, no journal event.
    CrashBeforeCommit,
}

/// One verify/authorize/commit session. Created only by
/// [`ProjectRootGateV1::verify`]; consumed by value on commit, so a session
/// cannot be committed twice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootGateSessionV1 {
    declared_parent_root: DigestV1,
    verified_successor_root: DigestV1,
    authorized: bool,
}

impl RootGateSessionV1 {
    pub fn declared_parent_root(&self) -> DigestV1 {
        self.declared_parent_root
    }
    pub fn verified_successor_root(&self) -> DigestV1 {
        self.verified_successor_root
    }
    pub fn is_authorized(&self) -> bool {
        self.authorized
    }
}

/// Runtime driver for the project successor CAS (ZS-KERNEL-008).
///
/// Verify and authorize are pure observations that never mutate the CAS.
/// Commit is the ONLY mutation and the only place a journal `Commit` event
/// is emitted. A crash at any point before commit leaves the old root; a
/// commit whose event was durably journaled leaves the complete new root
/// recoverable from the journal.
#[derive(Clone, Debug)]
pub struct ProjectRootGateV1 {
    cas: ProjectSuccessorCasV1,
    authority: String,
    fault: Option<RootGateFaultV1>,
}

impl ProjectRootGateV1 {
    pub fn new(genesis: DigestV1, authority: impl Into<String>) -> Result<Self, KernelRuntimeError> {
        let authority = authority.into();
        if authority.is_empty() {
            return Err(KernelRuntimeError::Unauthorized {
                detail: "gate authority must be nonempty".to_owned(),
            });
        }
        Ok(Self {
            cas: ProjectSuccessorCasV1::new(genesis),
            authority,
            fault: None,
        })
    }

    /// Rebuild the gate from a journal: the current root is the last
    /// committed successor root, or genesis when the journal has no commit.
    pub fn from_journal<S: JournalStore>(
        journal: &KernelEventJournalV1<S>,
        authority: impl Into<String>,
    ) -> Result<Self, KernelRuntimeError> {
        let current = journal.current_project_root()?.unwrap_or_else(event_log_genesis);
        Self::new(current, authority)
    }

    /// Install a runtime fault (fault-injection tests only).
    pub fn with_fault(mut self, fault: RootGateFaultV1) -> Self {
        self.fault = Some(fault);
        self
    }

    pub fn current(&self) -> DigestV1 {
        self.cas.current()
    }

    pub fn authority(&self) -> &str {
        &self.authority
    }

    fn unchanged_receipt(
        &self,
        declared_parent_root: DigestV1,
        verified_successor_root: DigestV1,
    ) -> Result<SuccessorRecordV1, KernelRuntimeError> {
        SuccessorRecordV1::new(
            declared_parent_root,
            verified_successor_root,
            false,
            &self.authority,
        )
        .map_err(KernelRuntimeError::Identity)
    }

    /// Phase 1 -- verify. Pure observation: checks the declared parent
    /// against the current root and the successor root for verified change.
    /// Violations are loud errors carrying an unchanged-successor receipt.
    pub fn verify(
        &self,
        declared_parent_root: DigestV1,
        verified_successor_root: DigestV1,
    ) -> Result<RootGateSessionV1, KernelRuntimeError> {
        if declared_parent_root != self.cas.current() {
            return Err(KernelRuntimeError::StaleProjectHandle {
                declared_parent: declared_parent_root,
                current: self.cas.current(),
                receipt: self.unchanged_receipt(declared_parent_root, verified_successor_root)?,
            });
        }
        if verified_successor_root == self.cas.current() {
            return Err(KernelRuntimeError::NoVerifiedChange {
                receipt: self.unchanged_receipt(declared_parent_root, verified_successor_root)?,
            });
        }
        Ok(RootGateSessionV1 {
            declared_parent_root,
            verified_successor_root,
            authorized: false,
        })
    }

    /// Phase 2 -- authorize. Pure observation: marks the session authorized;
    /// never mutates the CAS. Refuses under `AuthorizationRefused` faults.
    pub fn authorize(&mut self, session: &mut RootGateSessionV1) -> Result<(), KernelRuntimeError> {
        if self.fault == Some(RootGateFaultV1::AuthorizationRefused) {
            return Err(KernelRuntimeError::FaultInjected { phase: "authorize" });
        }
        if session.authorized {
            return Err(KernelRuntimeError::Unauthorized {
                detail: "session already authorized".to_owned(),
            });
        }
        session.authorized = true;
        Ok(())
    }

    /// Phase 3 -- commit. The ONLY mutation. Requires an authorized session;
    /// advances the CAS and emits the `Commit` journal event with the new
    /// project root as payload. Returns the sealed successor receipt.
    pub fn commit<S: JournalStore>(
        &mut self,
        session: RootGateSessionV1,
        journal: &mut KernelEventJournalV1<S>,
    ) -> Result<SuccessorRecordV1, KernelRuntimeError> {
        if !session.authorized {
            return Err(KernelRuntimeError::Unauthorized {
                detail: "commit requires an authorized session (verify -> authorize -> commit)"
                    .to_owned(),
            });
        }
        if self.fault == Some(RootGateFaultV1::CrashBeforeCommit) {
            return Err(KernelRuntimeError::FaultInjected { phase: "commit" });
        }
        match self.cas.try_advance(session.declared_parent_root, session.verified_successor_root) {
            SuccessorOutcomeV1::Advanced { new_current_root } => {
                let receipt = SuccessorRecordV1::new(
                    session.declared_parent_root,
                    session.verified_successor_root,
                    true,
                    &self.authority,
                )
                .map_err(KernelRuntimeError::Identity)?;
                journal.append(EventClassV1::Commit, new_current_root.to_hex(), &self.authority)?;
                Ok(receipt)
            }
            SuccessorOutcomeV1::Unchanged { reason } => {
                let receipt = self.unchanged_receipt(
                    session.declared_parent_root,
                    session.verified_successor_root,
                )?;
                match reason {
                    SuccessorUnchangedReasonV1::DeclaredParentMismatch => {
                        Err(KernelRuntimeError::StaleProjectHandle {
                            declared_parent: session.declared_parent_root,
                            current: self.cas.current(),
                            receipt,
                        })
                    }
                    SuccessorUnchangedReasonV1::NoVerifiedChange => {
                        Err(KernelRuntimeError::NoVerifiedChange { receipt })
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cache admission gate (ZS-KERNEL-003).
// ---------------------------------------------------------------------------

/// Sealed record of one cache admission decision. `admitted == false`
/// carries a reason; the record root is content-derived and deterministic.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CacheAdmissionRecordV1 {
    pub record_version: u16,
    pub admitted: bool,
    pub reason: String,
    pub receipt_root: String,
    pub contract_root: String,
    pub payload_root: String,
    pub dependency_roots: Vec<String>,
    pub abi_version: String,
}

impl CacheAdmissionRecordV1 {
    fn new(
        admitted: bool,
        reason: impl Into<String>,
        receipt_root: DigestV1,
        contract_root: DigestV1,
        payload_root: &str,
        dependency_roots: &[String],
    ) -> Self {
        // Normalized set so the same decision inputs (dependency SET, any
        // order, duplicates) seal to the same record root.
        let mut dependency_roots: Vec<String> = dependency_roots.to_vec();
        dependency_roots.sort_unstable();
        dependency_roots.dedup();
        Self {
            record_version: 1,
            admitted,
            reason: reason.into(),
            receipt_root: receipt_root.to_hex(),
            contract_root: contract_root.to_hex(),
            payload_root: payload_root.to_owned(),
            dependency_roots,
            abi_version: ROOTED_ABI_VERSION.to_owned(),
        }
    }

    fn admitted(
        _receipt: &PayloadFormationReceiptV1,
        receipt_root: DigestV1,
        contract_root: DigestV1,
        payload_root: &str,
        dependency_roots: &[String],
    ) -> Self {
        Self::new(
            true,
            "",
            receipt_root,
            contract_root,
            payload_root,
            dependency_roots,
        )
    }

    fn refused(
        reason: impl Into<String>,
        _receipt: &PayloadFormationReceiptV1,
        receipt_root: DigestV1,
        contract_root: DigestV1,
        payload_root: &str,
        dependency_roots: &[String],
    ) -> Self {
        Self::new(
            false,
            reason,
            receipt_root,
            contract_root,
            payload_root,
            dependency_roots,
        )
    }

    /// Content-derived, deterministic decision root: sha256 over the
    /// domain-tagged canonical JSON. Same inputs, same root.
    pub fn record_root(&self) -> String {
        let value = serde_json::to_value(self).expect("cache admission record is JSON-serializable");
        let mut tagged = Vec::with_capacity(CACHE_ADMISSION_DOMAIN_V1.len() + 128);
        tagged.extend_from_slice(CACHE_ADMISSION_DOMAIN_V1);
        tagged.extend_from_slice(canonical_json(&value).as_bytes());
        zero_abi::sha256_hex(&tagged)
    }
}

/// Cache admission gate (ZS-KERNEL-003): no cache entry is admitted without
/// a [`PayloadFormationReceiptV1`] whose rooted identity, payload/contract
/// bindings, and formation-time dependency set all still hold.
#[derive(Clone, Copy, Debug, Default)]
pub struct CacheAdmissionGateV1;

impl CacheAdmissionGateV1 {
    /// Decide admission. Returns the sealed decision record; `admitted` is
    /// false with a `reason` for every refusal. Structural identity failures
    /// (noncanonical receipt, wrong ABI) are loud errors.
    ///
    /// `expected_receipt_root` is the root the cache authority sealed at
    /// formation time; an offered receipt whose canonical bytes do not match
    /// it is a tampered identity and is refused.
    pub fn decide(
        receipt: &PayloadFormationReceiptV1,
        expected_receipt_root: DigestV1,
        contract_root: DigestV1,
        payload_root: &str,
        current_dependency_roots: &[String],
    ) -> Result<CacheAdmissionRecordV1, KernelRuntimeError> {
        receipt.validate()?;
        let canonical = receipt.canonical_bytes()?;
        if !verify_object_root(
            ObjectClassV1::FormationReceipt,
            ROOTED_ABI_VERSION,
            &canonical,
            expected_receipt_root,
        ) {
            return Ok(CacheAdmissionRecordV1::refused(
                "receipt root mismatch: the offered receipt is not the sealed formation receipt",
                receipt,
                expected_receipt_root,
                contract_root,
                payload_root,
                current_dependency_roots,
            ));
        }
        if !receipt.verify_payload(contract_root, payload_root) {
            return Ok(CacheAdmissionRecordV1::refused(
                "payload or contract binding mismatch (relabeled payload)",
                receipt,
                expected_receipt_root,
                contract_root,
                payload_root,
                current_dependency_roots,
            ));
        }
        if !receipt.verify_against(current_dependency_roots) {
            return Ok(CacheAdmissionRecordV1::refused(
                "dependency roots mutated since formation",
                receipt,
                expected_receipt_root,
                contract_root,
                payload_root,
                current_dependency_roots,
            ));
        }
        Ok(CacheAdmissionRecordV1::admitted(
            receipt,
            expected_receipt_root,
            contract_root,
            payload_root,
            current_dependency_roots,
        ))
    }

    /// Convenience for the zero-store `cache_entry` path: maps the candidate
    /// [`CacheKeyV1`]'s minimum dependency roots onto the current dependency
    /// set for [`CacheAdmissionGateV1::decide`].
    pub fn decide_for_cache_key(
        receipt: &PayloadFormationReceiptV1,
        expected_receipt_root: DigestV1,
        contract_root: DigestV1,
        payload_root: &str,
        key: &CacheKeyV1,
    ) -> Result<CacheAdmissionRecordV1, KernelRuntimeError> {
        let dependency_roots: Vec<String> = key
            .minimum_dependency_roots()
            .iter()
            .map(|root| root.as_str().to_owned())
            .collect();
        Self::decide(
            receipt,
            expected_receipt_root,
            contract_root,
            payload_root,
            &dependency_roots,
        )
    }
}
