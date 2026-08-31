use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use zero_abi::{
    EngineError, EngineErrorKind, EngineInvocation, FileEffectKind, FileEffectReceipt,
    FileEffectRequest, FileEngine, FileLease, FileMetadata, FileReadRequest, ReadOptions,
    ZeroHandle,
};
use zero_store::{LOCK_DEADLINE, StoreLock, ZeroCas, atomic_write_file, replace_file, sync_dir};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static CELL_SEQUENCE_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionState {
    Prepared,
    Applying,
    Committed,
    Compensating,
    RolledBack,
    RecoveryRequired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedEffect {
    pub request: FileEffectRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<ZeroHandle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_metadata: Option<FileMetadata>,
    pub preparation: ZeroHandle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<FileEffectReceipt>,
    pub restored: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionRecord {
    pub session_id: String,
    pub cell_id: String,
    pub trace_id: String,
    pub state: TransactionState,
    pub effects: Vec<PreparedEffect>,
    #[serde(default)]
    pub rollback_errors: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum TransactionError {
    #[error("transaction engine: {0}")]
    Engine(#[from] EngineError),
    #[error("transaction store: {0}")]
    Store(String),
    #[error("transaction rollback incomplete: {0:?}")]
    RecoveryRequired(Vec<String>),
}

#[derive(Clone)]
pub struct TransactionCoordinator {
    root: PathBuf,
    cas: ZeroCas,
    files: Arc<dyn FileEngine>,
}

impl TransactionCoordinator {
    pub fn new(root: impl Into<PathBuf>, cas: ZeroCas, files: Arc<dyn FileEngine>) -> Self {
        Self {
            root: root.into(),
            cas,
            files,
        }
    }

    pub fn highest_cell_sequence(&self, session_id: &str) -> Result<u64, TransactionError> {
        let directory = self.session_directory(session_id);
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => {
                return Err(TransactionError::Store(format!(
                    "read transaction directory: {error}"
                )));
            }
        };
        let mut highest = 0;
        for entry in entries {
            let entry = entry.map_err(|error| {
                TransactionError::Store(format!("read transaction entry: {error}"))
            })?;
            if !entry
                .file_type()
                .map_err(|error| TransactionError::Store(error.to_string()))?
                .is_file()
            {
                continue;
            }
            if ignored_transaction_entry(&entry.path()) {
                continue;
            }
            let bytes = fs::read(entry.path()).map_err(|error| {
                TransactionError::Store(format!("read transaction record: {error}"))
            })?;
            let record: TransactionRecord = serde_json::from_slice(&bytes)
                .map_err(|error| TransactionError::Store(error.to_string()))?;
            if record.session_id != session_id {
                return Err(TransactionError::Store(
                    "transaction record session does not match its directory".into(),
                ));
            }
            highest = highest.max(cell_sequence(&record.cell_id).unwrap_or(0));
        }
        Ok(highest)
    }

    pub fn allocate_cell_sequence(
        &self,
        session_id: &str,
        minimum: u64,
    ) -> Result<u64, TransactionError> {
        let _process_guard = CELL_SEQUENCE_MUTEX
            .lock()
            .map_err(|_| TransactionError::Store("cell sequence mutex is poisoned".into()))?;
        let session = blake3::hash(session_id.as_bytes()).to_hex().to_string();
        let lock_root = self.root.join("cell-sequence-lock").join(&session);
        let _guard = StoreLock::publish(&lock_root, LOCK_DEADLINE).map_err(|error| {
            TransactionError::Store(format!("acquire cell sequence lock: {error}"))
        })?;
        let path = self
            .root
            .join("cell-sequences")
            .join(format!("{session}.txt"));
        let stored = match fs::read_to_string(&path) {
            Ok(value) => value.trim().parse::<u64>().map_err(|error| {
                TransactionError::Store(format!("invalid durable cell sequence: {error}"))
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => {
                return Err(TransactionError::Store(format!(
                    "read durable cell sequence: {error}"
                )));
            }
        };
        let next = stored
            .max(minimum)
            .checked_add(1)
            .ok_or_else(|| TransactionError::Store("cell sequence overflow".into()))?;
        atomic_write_file(&path, next.to_string().as_bytes()).map_err(|error| {
            TransactionError::Store(format!("publish durable cell sequence: {error}"))
        })?;
        Ok(next)
    }

    pub fn begin(&self, invocation: EngineInvocation) -> Result<Transaction, TransactionError> {
        // Fail-closed on binding mismatch: if a journal already exists, check that it
        // binds the same session/cell/trace. Different binding at same path is a
        // loud store error, not a silent reuse.
        let lease = self.files.lease(&invocation)?;
        let path = self.record_path(&invocation.context.session_id, &invocation.context.cell_id);
        if path.exists() {
            // Read existing record to decide if this is an idempotent retry (same trace)
            // or a conflicting binding.
            let bytes = fs::read(&path).map_err(|error| {
                TransactionError::Store(format!("read existing transaction record: {error}"))
            })?;
            let existing: TransactionRecord = serde_json::from_slice(&bytes)
                .map_err(|error| TransactionError::Store(error.to_string()))?;
            if existing.session_id == invocation.context.session_id
                && existing.cell_id == invocation.context.cell_id
                && existing.trace_id == invocation.context.trace_id
                && matches!(
                    existing.state,
                    TransactionState::Prepared | TransactionState::Applying
                )
            {
                // Idempotent retry of an already-prepared transaction (e.g. caller retried
                // begin after a crash before first apply). Return handle to existing.
                return Ok(Transaction {
                    coordinator: self.clone(),
                    invocation,
                    path,
                    record: existing,
                    settled: false,
                    _lease: lease,
                });
            }
            return Err(TransactionError::Store(format!(
                "transaction journal already exists at {} (state={:?} trace={})",
                path.display(),
                existing.state,
                existing.trace_id
            )));
        }
        let record = TransactionRecord {
            session_id: invocation.context.session_id.clone(),
            cell_id: invocation.context.cell_id.clone(),
            trace_id: invocation.context.trace_id.clone(),
            state: TransactionState::Prepared,
            effects: Vec::new(),
            rollback_errors: Vec::new(),
        };
        persist_record(&path, &record)?;
        Ok(Transaction {
            coordinator: self.clone(),
            invocation,
            path,
            record,
            settled: false,
            _lease: lease,
        })
    }

    pub fn reconcile(
        &self,
        invocation: &EngineInvocation,
    ) -> Result<Vec<PathBuf>, TransactionError> {
        let _lease = self.files.lease(invocation)?;
        let directory = self.session_directory(&invocation.context.session_id);
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(TransactionError::Store(format!(
                    "read transaction directory: {error}"
                )));
            }
        };
        let mut recovered = Vec::new();
        let mut recovery_details: Vec<String> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                TransactionError::Store(format!("read transaction entry: {error}"))
            })?;
            if !entry
                .file_type()
                .map_err(|error| TransactionError::Store(error.to_string()))?
                .is_file()
            {
                continue;
            }
            let path = entry.path();
            if ignored_transaction_entry(&path) {
                continue;
            }
            let bytes = fs::read(&path).map_err(|error| {
                TransactionError::Store(format!("read transaction record: {error}"))
            })?;
            // Torn or non-canonical record is fail-closed: quarantine and report typed recovery.
            let mut record: TransactionRecord = match serde_json::from_slice(&bytes) {
                Ok(record) => record,
                Err(error) => {
                    let poisoned = poisoned_journal_path(&path);
                    let _ = fs::rename(&path, &poisoned);
                    recovery_details.push(format!(
                        "quarantined torn transaction record {}: {error}",
                        path.display()
                    ));
                    recovery_details.push(format!(
                        "quarantined poisoned journal {}",
                        poisoned.display()
                    ));
                    continue;
                }
            };
            if record.session_id != invocation.context.session_id {
                // Foreign session binding mismatch is fail-closed: quarantine this entry only.
                let poisoned = poisoned_journal_path(&path);
                let _ = fs::rename(&path, &poisoned);
                recovery_details.push(format!(
                    "quarantined foreign session binding {} (expected {})",
                    record.session_id, invocation.context.session_id
                ));
                continue;
            }
            if matches!(
                record.state,
                TransactionState::Committed | TransactionState::RolledBack
            ) {
                continue;
            }
            // Every terminal path goes through rollback_record exactly once. RecoveryRequired
            // is the typed state that proves quarantine.
            rollback_record(&*self.files, invocation, &mut record);
            if record.state == TransactionState::RecoveryRequired {
                // Preserve the exact original journal for forensic recovery.
                // Persisting the mutated RecoveryRequired record first would
                // overwrite the evidence that explains the failed rollback.
                let poisoned = poisoned_journal_path(&path);
                fs::rename(&path, &poisoned).map_err(|error| {
                    TransactionError::Store(format!(
                        "quarantine poisoned transaction journal: {error}"
                    ))
                })?;
                recovery_details.extend(record.rollback_errors.clone());
                recovery_details.push(format!(
                    "quarantined poisoned journal {}",
                    poisoned.display()
                ));
                continue;
            }
            // A directory-sync failure makes terminal publication ambiguous and requires recovery.
            if let Err(error) = persist_record(&path, &record) {
                let is_recovery = matches!(error, TransactionError::RecoveryRequired(_));
                if is_recovery {
                    if let TransactionError::RecoveryRequired(details) = error {
                        recovery_details.extend(details);
                    }
                    let poisoned = poisoned_journal_path(&path);
                    let _ = fs::rename(&path, &poisoned);
                    recovery_details.push(format!(
                        "quarantined ambiguous transaction record {}",
                        poisoned.display()
                    ));
                    continue;
                }
                return Err(error);
            }
            recovered.push(path);
        }
        if !recovery_details.is_empty() {
            let mut details = vec![format!(
                "{} poisoned journal(s) quarantined this pass",
                recovery_details.len()
            )];
            details.extend(recovery_details);
            return Err(TransactionError::RecoveryRequired(details));
        }
        Ok(recovered)
    }

    fn session_directory(&self, session_id: &str) -> PathBuf {
        let session = blake3::hash(session_id.as_bytes()).to_hex().to_string();
        self.root.join("transactions").join(session)
    }

    fn record_path(&self, session_id: &str, cell_id: &str) -> PathBuf {
        let cell = blake3::hash(cell_id.as_bytes()).to_hex().to_string();
        self.session_directory(session_id)
            .join(format!("{cell}.json"))
    }
}

fn cell_sequence(cell_id: &str) -> Option<u64> {
    cell_id
        .strip_prefix("cell-")?
        .parse()
        .ok()
        .filter(|sequence| *sequence > 0)
}

pub struct Transaction {
    coordinator: TransactionCoordinator,
    invocation: EngineInvocation,
    path: PathBuf,
    record: TransactionRecord,
    settled: bool,
    _lease: Box<dyn FileLease>,
}

pub(crate) enum PendingFileContent<'a> {
    Present(&'a [u8]),
    Removed,
    Unavailable,
}

fn is_terminal(state: &TransactionState) -> bool {
    matches!(
        state,
        TransactionState::Committed
            | TransactionState::RolledBack
            | TransactionState::RecoveryRequired
    )
}

fn transition_allowed(from: &TransactionState, to: &TransactionState) -> bool {
    match (from, to) {
        (TransactionState::Prepared, TransactionState::Applying) => true,
        (TransactionState::Applying, TransactionState::Applying) => true,
        (TransactionState::Prepared, TransactionState::Committed) => true,
        (TransactionState::Applying, TransactionState::Committed) => true,
        (TransactionState::Prepared, TransactionState::RolledBack) => true,
        (TransactionState::Applying, TransactionState::RolledBack) => true,
        (TransactionState::Prepared, TransactionState::RecoveryRequired) => true,
        (TransactionState::Applying, TransactionState::RecoveryRequired) => true,
        // Idempotent re-commit / re-rollback are allowed only via their dedicated
        // authority paths (commit/rollback checking already-terminal), not via
        // arbitrary transition.
        (TransactionState::Committed, TransactionState::Committed) => true,
        (TransactionState::Committed, TransactionState::Compensating) => true,
        (TransactionState::Compensating, TransactionState::Compensating) => true,
        (TransactionState::Compensating, TransactionState::RolledBack) => true,
        (TransactionState::Compensating, TransactionState::RecoveryRequired) => true,
        (TransactionState::RolledBack, TransactionState::RolledBack) => true,
        (TransactionState::RecoveryRequired, TransactionState::RecoveryRequired) => true,
        _ => false,
    }
}

fn validate_transition(
    from: &TransactionState,
    to: &TransactionState,
) -> Result<(), TransactionError> {
    if transition_allowed(from, to) {
        Ok(())
    } else {
        Err(TransactionError::Store(format!(
            "transaction state transition {:?} -> {:?} is not allowed",
            from, to
        )))
    }
}

impl Transaction {
    pub(crate) fn pending_content(&self, path: &Path) -> Option<PendingFileContent<'_>> {
        let requested = logical_path(&self.invocation.context.project_root, path);
        let effect = self.record.effects.iter().rev().find(|effect| {
            let receipt_path = effect
                .receipt
                .as_ref()
                .map(|receipt| logical_path(&self.invocation.context.project_root, &receipt.path));
            receipt_path.as_ref() == Some(&requested)
                || logical_path(&self.invocation.context.project_root, &effect.request.path)
                    == requested
        })?;
        Some(match effect.request.kind {
            FileEffectKind::Remove => PendingFileContent::Removed,
            FileEffectKind::Write | FileEffectKind::Edit => effect
                .request
                .content
                .as_deref()
                .map(PendingFileContent::Present)
                .unwrap_or(PendingFileContent::Unavailable),
            FileEffectKind::Restore => PendingFileContent::Unavailable,
        })
    }

    /// The receipt facts of every applied effect, in dispatch order. Callers
    /// snapshot this before settlement (commit/rollback) so terminal events
    /// can bind the actual committed or rolled-back receipt coordinates.
    pub(crate) fn receipts(&self) -> Vec<FileEffectReceipt> {
        self.record
            .effects
            .iter()
            .filter_map(|effect| effect.receipt.clone())
            .collect()
    }

    pub fn apply(
        &mut self,
        mut request: FileEffectRequest,
    ) -> Result<FileEffectReceipt, TransactionError> {
        if self.settled {
            return Err(TransactionError::Store(
                "transaction is already settled".into(),
            ));
        }
        if is_terminal(&self.record.state) {
            return Err(TransactionError::Store(format!(
                "transaction already terminal in {:?}, cannot apply",
                self.record.state
            )));
        }
        // Fail-closed on cancellation: do not start work when cancelled.
        if self.invocation.cancellation.is_cancelled() {
            return Err(TransactionError::Engine(EngineError::new(
                EngineErrorKind::Cancelled,
                "transaction cancelled before apply",
                false,
            )));
        }
        let snapshot = match self.coordinator.files.read(
            &self.invocation,
            FileReadRequest {
                path: request.path.clone(),
                options: ReadOptions::default(),
            },
        ) {
            Ok(snapshot) => Some(snapshot),
            Err(error) if error.kind == EngineErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if request.expected_preimage.is_none() {
            request.expected_preimage = snapshot.as_ref().map(|snapshot| snapshot.content.clone());
        } else if let Some(expected) = &request.expected_preimage {
            // Fail-closed on binding mismatch: expected preimage must match current state
            // unless the file is being created (expect_absent). Mismatch is a Conflict.
            if let Some(snapshot) = &snapshot {
                if snapshot.content != *expected {
                    return Err(TransactionError::Engine(EngineError::new(
                        EngineErrorKind::Conflict,
                        format!(
                            "preimage mismatch for {}: expected {} got {}",
                            request.path.display(),
                            expected,
                            snapshot.content
                        ),
                        false,
                    )));
                }
            } else if !request.expect_absent {
                return Err(TransactionError::Engine(EngineError::new(
                    EngineErrorKind::Conflict,
                    format!(
                        "preimage mismatch for {}: expected {} but target is absent",
                        request.path.display(),
                        expected
                    ),
                    false,
                )));
            }
        }
        #[derive(Serialize)]
        struct Preparation<'a> {
            request: &'a FileEffectRequest,
            before: &'a Option<ZeroHandle>,
            trace_id: &'a str,
        }
        let before = snapshot.as_ref().map(|snapshot| snapshot.content.clone());
        let bytes = serde_json::to_vec(&Preparation {
            request: &request,
            before: &before,
            trace_id: &self.invocation.context.trace_id,
        })
        .map_err(|error| TransactionError::Store(error.to_string()))?;
        let preparation = self
            .coordinator
            .cas
            .put(&bytes)
            .map_err(|error| TransactionError::Store(error.to_string()))?;
        let effect = PreparedEffect {
            request: request.clone(),
            before,
            before_metadata: snapshot.as_ref().map(|snapshot| FileMetadata {
                mode: snapshot.mode,
                modified_unix_ns: snapshot.modified_unix_ns,
                symlink_target: snapshot.symlink_target.clone(),
                symlink_target_is_dir: snapshot.symlink_target_is_dir,
            }),
            preparation,
            receipt: None,
            restored: false,
        };
        self.record.effects.push(effect);
        let target_state = TransactionState::Applying;
        validate_transition(&self.record.state, &target_state)?;
        self.record.state = target_state;
        persist_record(&self.path, &self.record)?;

        // Check cancellation again before dispatching the effect: cancellation cannot commit.
        if self.invocation.cancellation.is_cancelled() {
            // Cancellation rolls back the persisted preparation without leaving a receipt.
            self.record.effects.pop();
            // An empty effect list returns the transaction to Prepared.
            if self.record.effects.is_empty() {
                self.record.state = TransactionState::Prepared;
            }
            persist_record(&self.path, &self.record)?;
            return Err(TransactionError::Engine(EngineError::new(
                EngineErrorKind::Cancelled,
                "transaction cancelled before effect dispatch",
                false,
            )));
        }

        match self.coordinator.files.apply(&self.invocation, request) {
            Ok(receipt) => {
                // The hub preparation and the engine journal are distinct
                // authorities. Bind the receipt to the requested path and
                // observed preimage; the engine owns its journal identity.
                let (expected_path, expected_before) = {
                    let prepared = self.record.effects.last().expect("prepared effect exists");
                    (prepared.request.path.clone(), prepared.before.clone())
                };
                let mismatch = if receipt.path != expected_path {
                    Some(format!(
                        "receipt path mismatch: expected {} got {}",
                        expected_path.display(),
                        receipt.path.display()
                    ))
                } else if receipt.before != expected_before {
                    Some(format!(
                        "receipt before mismatch for {}",
                        receipt.path.display()
                    ))
                } else {
                    None
                };
                if let Some(detail) = mismatch {
                    self.record
                        .effects
                        .last_mut()
                        .expect("prepared effect exists")
                        .receipt = Some(receipt.clone());
                    rollback_record(&*self.coordinator.files, &self.invocation, &mut self.record);
                    persist_record(&self.path, &self.record)?;
                    if self.record.state == TransactionState::RecoveryRequired {
                        return Err(TransactionError::RecoveryRequired(
                            self.record.rollback_errors.clone(),
                        ));
                    }
                    return Err(TransactionError::Store(detail));
                }
                self.record
                    .effects
                    .last_mut()
                    .expect("prepared effect exists")
                    .receipt = Some(receipt.clone());
                if self.invocation.cancellation.is_cancelled() {
                    rollback_record(&*self.coordinator.files, &self.invocation, &mut self.record);
                    persist_record(&self.path, &self.record)?;
                    if self.record.state == TransactionState::RecoveryRequired {
                        return Err(TransactionError::RecoveryRequired(
                            self.record.rollback_errors.clone(),
                        ));
                    }
                    return Err(TransactionError::Engine(EngineError::new(
                        EngineErrorKind::Cancelled,
                        "transaction cancelled after apply, rolled back",
                        false,
                    )));
                }
                persist_record(&self.path, &self.record)?;
                Ok(receipt)
            }
            Err(error) => {
                // NotFound and Conflict are precondition failures: the engine
                // must not mutate on either. Drop only the unapplied preparation
                // so guest try/catch can recover and the cell remains committable.
                if matches!(
                    error.kind,
                    EngineErrorKind::NotFound | EngineErrorKind::Conflict
                ) {
                    self.record.effects.pop();
                    if self.record.effects.is_empty() {
                        self.record.state = TransactionState::Prepared;
                    }
                    persist_record(&self.path, &self.record)?;
                    return Err(error.into());
                }
                // For other errors, check if cancellation caused it.
                if self.invocation.cancellation.is_cancelled() {
                    // Prefer typed cancellation over generic engine error.
                    self.record.effects.pop();
                    if self.record.effects.is_empty() {
                        self.record.state = TransactionState::Prepared;
                    }
                    persist_record(&self.path, &self.record)?;
                    return Err(TransactionError::Engine(EngineError::new(
                        EngineErrorKind::Cancelled,
                        format!("transaction cancelled: {error}"),
                        false,
                    )));
                }
                rollback_record(&*self.coordinator.files, &self.invocation, &mut self.record);
                // Persist the terminalization; ambiguous write becomes RecoveryRequired.
                let persist_result = persist_record(&self.path, &self.record);
                self.settled = true;
                if let Err(persist_error) = persist_result {
                    if let TransactionError::RecoveryRequired(details) = persist_error {
                        return Err(TransactionError::RecoveryRequired(details));
                    }
                    return Err(persist_error);
                }
                if self.record.state == TransactionState::RecoveryRequired {
                    Err(TransactionError::RecoveryRequired(
                        self.record.rollback_errors.clone(),
                    ))
                } else {
                    Err(error.into())
                }
            }
        }
    }

    pub fn commit(&mut self) -> Result<Vec<FileEffectReceipt>, TransactionError> {
        if self.settled {
            return Err(TransactionError::Store(
                "transaction is already settled".into(),
            ));
        }
        // Cancellation cannot commit: check before any state mutation.
        if self.invocation.cancellation.is_cancelled() {
            // Authority path for cancellation is rollback, not commit.
            rollback_record(&*self.coordinator.files, &self.invocation, &mut self.record);
            let persist_result = persist_record(&self.path, &self.record);
            self.settled = true;
            if let Err(error) = persist_result {
                return Err(error);
            }
            if self.record.state == TransactionState::RecoveryRequired {
                return Err(TransactionError::RecoveryRequired(
                    self.record.rollback_errors.clone(),
                ));
            }
            return Err(TransactionError::Engine(EngineError::new(
                EngineErrorKind::Cancelled,
                "transaction cancelled, commit refused",
                false,
            )));
        }
        // Idempotent re-commit: if already committed, prove committed receipts.
        if self.record.state == TransactionState::Committed {
            // Fail-closed on receipt mismatch: caller must see same receipts.
            let receipts = self
                .record
                .effects
                .iter()
                .filter_map(|effect| effect.receipt.clone())
                .collect::<Vec<_>>();
            // Every committed effect requires a receipt.
            if self
                .record
                .effects
                .iter()
                .any(|effect| effect.receipt.is_none())
            {
                return Err(TransactionError::RecoveryRequired(vec![format!(
                    "committed transaction {} missing receipts, cannot prove",
                    self.path.display()
                )]));
            }
            self.settled = true;
            return Ok(receipts);
        }
        if is_terminal(&self.record.state) {
            return Err(TransactionError::Store(format!(
                "commit disallowed from terminal state {:?}",
                self.record.state
            )));
        }
        // Fail-closed: all effects must have receipts (prepared but not applied is not committable).
        if self
            .record
            .effects
            .iter()
            .any(|effect| effect.receipt.is_none())
        {
            return Err(TransactionError::Store(
                "commit requires every effect to have a receipt; unapplied preparation exists"
                    .into(),
            ));
        }
        // Fail closed if a committed receipt no longer binds the request and
        // preimage recorded by the hub. The engine journal remains opaque.
        for effect in &self.record.effects {
            if let Some(receipt) = &effect.receipt {
                if receipt.path != effect.request.path {
                    return Err(TransactionError::Store(format!(
                        "commit receipt path mismatch for {}",
                        effect.request.path.display()
                    )));
                }
                if receipt.before != effect.before {
                    return Err(TransactionError::Store(format!(
                        "commit receipt before mismatch for {}",
                        receipt.path.display()
                    )));
                }
            }
        }
        validate_transition(&self.record.state, &TransactionState::Committed)?;
        self.record.state = TransactionState::Committed;
        // Durable publish: ambiguous write (dir sync failure) is RecoveryRequired, not success.
        persist_record(&self.path, &self.record)?;
        self.settled = true;
        Ok(self
            .record
            .effects
            .iter()
            .filter_map(|effect| effect.receipt.clone())
            .collect())
    }

    pub fn compensate_committed(&mut self) -> Result<(), TransactionError> {
        if self.record.state != TransactionState::Committed || !self.settled {
            return Err(TransactionError::Store(
                "compensation requires a durably committed transaction".into(),
            ));
        }
        validate_transition(&self.record.state, &TransactionState::Compensating)?;
        self.record.state = TransactionState::Compensating;
        // Publish compensation intent before restoring bytes. A crash after
        // this boundary is recovered by reconcile through the same receipts.
        if let Err(error) = persist_record(&self.path, &self.record) {
            self.record.state = TransactionState::Committed;
            return Err(error);
        }
        self.settled = false;
        rollback_record(&*self.coordinator.files, &self.invocation, &mut self.record);
        let persist_result = persist_record(&self.path, &self.record);
        self.settled = true;
        persist_result?;
        if self.record.state == TransactionState::RecoveryRequired {
            Err(TransactionError::RecoveryRequired(
                self.record.rollback_errors.clone(),
            ))
        } else {
            Ok(())
        }
    }

    pub fn rollback(mut self) -> Result<(), TransactionError> {
        // Terminal rollback states are idempotent even when apply already
        // settled the journal before returning its engine error.
        if self.record.state == TransactionState::RolledBack {
            self.settled = true;
            return Ok(());
        }
        if self.record.state == TransactionState::RecoveryRequired {
            self.settled = true;
            return Err(TransactionError::RecoveryRequired(
                self.record.rollback_errors.clone(),
            ));
        }
        if self.record.state == TransactionState::Committed {
            return Err(TransactionError::Store(
                "transaction already committed, cannot rollback".into(),
            ));
        }
        if self.settled {
            return Err(TransactionError::Store(
                "transaction is already settled".into(),
            ));
        }
        rollback_record(&*self.coordinator.files, &self.invocation, &mut self.record);
        // Validate transition to whatever rollback_record decided.
        // rollback_record sets RolledBack or RecoveryRequired explicitly.
        persist_record(&self.path, &self.record)?;
        self.settled = true;
        if self.record.state == TransactionState::RecoveryRequired {
            Err(TransactionError::RecoveryRequired(
                self.record.rollback_errors.clone(),
            ))
        } else {
            Ok(())
        }
    }
}
fn logical_path(root: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    if let Ok(canonical) = fs::canonicalize(&joined) {
        return canonical;
    }
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        // Drop authority path is always rollback, never commit. This ensures
        // cancellation or early exit cannot accidentally commit.
        rollback_record(&*self.coordinator.files, &self.invocation, &mut self.record);
        let _ = persist_record(&self.path, &self.record);
        self.settled = true;
    }
}

fn rollback_record(
    files: &dyn FileEngine,
    invocation: &EngineInvocation,
    record: &mut TransactionRecord,
) {
    // Explicit, deterministic reverse iteration: last effect first.
    let mut errors = Vec::new();
    for effect in record.effects.iter_mut().rev() {
        if effect.restored {
            continue;
        }
        let receipt = effect.receipt.clone().unwrap_or_else(|| FileEffectReceipt {
            kind: effect.request.kind.clone(),
            path: effect.request.path.clone(),
            before: effect.before.clone(),
            after: None,
            before_metadata: effect.before_metadata.clone(),
            journal: effect.preparation.clone(),
        });
        match files.restore(invocation, &receipt) {
            Ok(()) => effect.restored = true,
            Err(error) => errors.push(format!("{}: {error}", receipt.path.display())),
        }
    }
    record.rollback_errors = errors;
    let target = if record.rollback_errors.is_empty() {
        TransactionState::RolledBack
    } else {
        TransactionState::RecoveryRequired
    };
    // Ensure transition is allowed; if not, force RecoveryRequired.
    if validate_transition(&record.state, &target).is_ok() {
        record.state = target;
    } else {
        record.state = TransactionState::RecoveryRequired;
        if record.rollback_errors.is_empty() {
            record.rollback_errors.push(format!(
                "invalid transition {:?} -> {:?}",
                record.state, target
            ));
        }
    }
}

fn persist_record(path: &Path, record: &TransactionRecord) -> Result<(), TransactionError> {
    let bytes =
        serde_json::to_vec(record).map_err(|error| TransactionError::Store(error.to_string()))?;
    // Use durable file publish that distinguishes "present but not durably synced".
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| TransactionError::Store(format!("create transaction dir: {error}")))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| TransactionError::Store("transaction path has no file name".into()))?;
    let (mut file, temp) = open_unique_temp(parent, file_name)
        .map_err(|error| TransactionError::Store(format!("open transaction temp: {error}")))?;
    let publish: Result<(), TransactionError> = (|| {
        file.write_all(&bytes).map_err(|error| {
            TransactionError::Store(format!("write transaction record: {error}"))
        })?;
        file.sync_all().map_err(|error| {
            TransactionError::Store(format!("sync transaction record: {error}"))
        })?;
        drop(file);
        replace_file(&temp, path).map_err(|error| {
            TransactionError::Store(format!("publish transaction record: {error}"))
        })
    })();
    if let Err(error) = publish {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    // Directory sync failure after publish is ambiguous: the new bytes are visible
    // but not proven durable. Recovery must not guess; surface typed RecoveryRequired.
    if let Err(error) = sync_dir(parent) {
        return Err(TransactionError::RecoveryRequired(vec![format!(
            "transaction record directory sync failed after publish {}: {error}",
            path.display()
        )]));
    }
    Ok(())
}

fn open_unique_temp(
    parent: &Path,
    file_name: &std::ffi::OsStr,
) -> std::io::Result<(File, PathBuf)> {
    loop {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut name = std::ffi::OsString::from(".");
        name.push(file_name);
        name.push(format!(".txn-tmp-{}-{sequence}", std::process::id()));
        let path = parent.join(name);
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

fn ignored_transaction_entry(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".poisoned.json") || name.contains(".txn-tmp-"))
}

fn poisoned_journal_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("journal.json");
    let stem = file_name.strip_suffix(".json").unwrap_or(file_name);
    let poisoned_name = format!("{stem}.poisoned.json");
    path.with_file_name(poisoned_name)
}
