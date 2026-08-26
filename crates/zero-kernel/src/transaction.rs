use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use zero_abi::{
    EngineError, EngineErrorKind, EngineInvocation, FileEffectKind, FileEffectReceipt,
    FileEffectRequest, FileEngine, FileLease, FileMetadata, FileReadRequest, ReadOptions,
    ZeroHandle,
};
use zero_store::{SyncPolicy, ZeroCas, atomic_write_file_with_sync};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionState {
    Prepared,
    Applying,
    Committed,
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

    pub fn begin(&self, invocation: EngineInvocation) -> Result<Transaction, TransactionError> {
        let lease = self.files.lease(&invocation)?;
        let path = self.record_path(&invocation.context.session_id, &invocation.context.cell_id);
        if path.exists() {
            return Err(TransactionError::Store(format!(
                "transaction journal already exists at {}",
                path.display()
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
            // Quarantined poisoned journals are evidence only; skip them so future
            // cells recover. Filter on file name to avoid touching foreign roots.
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".poisoned.json"))
            {
                continue;
            }
            let bytes = fs::read(&path).map_err(|error| {
                TransactionError::Store(format!("read transaction record: {error}"))
            })?;
            let mut record: TransactionRecord = serde_json::from_slice(&bytes)
                .map_err(|error| TransactionError::Store(error.to_string()))?;
            if matches!(
                record.state,
                TransactionState::Committed | TransactionState::RolledBack
            ) {
                continue;
            }
            rollback_record(&*self.files, invocation, &mut record);
            persist_record(&path, &record)?;
            if record.state == TransactionState::RecoveryRequired {
                // Preserve evidence and quarantine so future reconciles recover.
                // Quarantine EVERY failing record in this pass rather than
                // aborting at the first: one poisoned journal must not hide
                // the rest nor cost one cell per journal.
                let poisoned = poisoned_journal_path(&path);
                // Best-effort rename; if it fails we still surface RecoveryRequired.
                let _ = fs::rename(&path, &poisoned);
                recovery_details.extend(record.rollback_errors.clone());
                recovery_details.push(format!(
                    "quarantined poisoned journal {}",
                    poisoned.display()
                ));
                continue;
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
            }),
            preparation,
            receipt: None,
            restored: false,
        };
        self.record.effects.push(effect);
        self.record.state = TransactionState::Applying;
        persist_record(&self.path, &self.record)?;

        match self.coordinator.files.apply(&self.invocation, request) {
            Ok(receipt) => {
                self.record
                    .effects
                    .last_mut()
                    .expect("prepared effect exists")
                    .receipt = Some(receipt.clone());
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
                    persist_record(&self.path, &self.record)?;
                    return Err(error.into());
                }
                rollback_record(&*self.coordinator.files, &self.invocation, &mut self.record);
                persist_record(&self.path, &self.record)?;
                self.settled = true;
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

    pub fn commit(mut self) -> Result<Vec<FileEffectReceipt>, TransactionError> {
        if self.settled {
            return Err(TransactionError::Store(
                "transaction is already settled".into(),
            ));
        }
        self.record.state = TransactionState::Committed;
        persist_record(&self.path, &self.record)?;
        self.settled = true;
        Ok(self
            .record
            .effects
            .iter()
            .filter_map(|effect| effect.receipt.clone())
            .collect())
    }

    pub fn rollback(mut self) -> Result<(), TransactionError> {
        rollback_record(&*self.coordinator.files, &self.invocation, &mut self.record);
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
    record.state = if record.rollback_errors.is_empty() {
        TransactionState::RolledBack
    } else {
        TransactionState::RecoveryRequired
    };
}

fn persist_record(path: &Path, record: &TransactionRecord) -> Result<(), TransactionError> {
    let bytes =
        serde_json::to_vec(record).map_err(|error| TransactionError::Store(error.to_string()))?;
    atomic_write_file_with_sync(path, &bytes, SyncPolicy::Required)
        .map_err(|error| TransactionError::Store(format!("publish transaction record: {error}")))
}

fn poisoned_journal_path(path: &Path) -> PathBuf {
    // "<name>.poisoned.json" preserves evidence; caller ensures original ends with .json.
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("journal.json");
    let stem = file_name.strip_suffix(".json").unwrap_or(file_name);
    let poisoned_name = format!("{stem}.poisoned.json");
    path.with_file_name(poisoned_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zero_abi::{
        EngineCallContext, EngineError, EngineErrorKind, EngineInvocation, FileEffectKind,
        FileEffectRequest, ZeroHandle,
    };
    use zero_store::ZeroCas;

    struct FailingFileEngine;
    impl FileLease for FailingFileEngine {}
    impl FileEngine for FailingFileEngine {
        fn lease(&self, _: &EngineInvocation) -> Result<Box<dyn FileLease>, EngineError> {
            Ok(Box::new(FailingFileEngine))
        }
        fn read(
            &self,
            _: &EngineInvocation,
            _: zero_abi::FileReadRequest,
        ) -> Result<zero_abi::FileSnapshot, EngineError> {
            Err(EngineError::new(
                EngineErrorKind::NotFound,
                "not found",
                false,
            ))
        }
        fn lookup(
            &self,
            _: &EngineInvocation,
            _: PathBuf,
            _: zero_abi::LookupOptions,
        ) -> Result<Vec<PathBuf>, EngineError> {
            Ok(Vec::new())
        }
        fn apply(
            &self,
            _: &EngineInvocation,
            _: FileEffectRequest,
        ) -> Result<zero_abi::FileEffectReceipt, EngineError> {
            Err(EngineError::new(EngineErrorKind::Internal, "apply", false))
        }
        fn restore(
            &self,
            _: &EngineInvocation,
            _: &zero_abi::FileEffectReceipt,
        ) -> Result<(), EngineError> {
            Err(EngineError::new(
                EngineErrorKind::Internal,
                "restore failed",
                false,
            ))
        }
        fn reconcile(&self, _: &EngineInvocation) -> Result<Vec<ZeroHandle>, EngineError> {
            Ok(Vec::new())
        }
    }

    fn test_invocation(session_id: &str, cell_id: &str) -> EngineInvocation {
        struct NoopCancel;
        impl zero_abi::CancellationProbe for NoopCancel {
            fn is_cancelled(&self) -> bool {
                false
            }
        }
        EngineInvocation {
            context: EngineCallContext {
                workspace_root: PathBuf::from("/tmp"),
                project_root: PathBuf::from("/tmp"),
                session_id: session_id.into(),
                cell_id: cell_id.into(),
                trace_id: format!("{session_id}-{cell_id}"),
                deadline_unix_ms: u64::MAX,
                budget: zero_abi::KernelBudget {
                    wall_ms: 1_000,
                    cpu_ms: 1_000,
                    memory_bytes: 1024 * 1024,
                    call_limit: 8,
                    task_limit: 2,
                    output_byte_limit: 4096,
                },
            },
            cancellation: Arc::new(NoopCancel),
        }
    }

    #[test]
    fn reconcile_quarantines_poisoned_journal_and_recovers_on_next_pass() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cas = ZeroCas::open(dir.path().join("cas"));
        let files: Arc<dyn FileEngine> = Arc::new(FailingFileEngine);
        let coordinator = TransactionCoordinator::new(dir.path().join("tx"), cas, files);
        let session_id = "sess-poison";
        let cell_id = "cell-1";
        let invocation = test_invocation(session_id, cell_id);
        let record_path = coordinator.record_path(session_id, cell_id);
        fs::create_dir_all(record_path.parent().unwrap()).expect("mkdir");
        let record = TransactionRecord {
            session_id: session_id.into(),
            cell_id: cell_id.into(),
            trace_id: invocation.context.trace_id.clone(),
            state: TransactionState::Prepared,
            effects: vec![PreparedEffect {
                request: FileEffectRequest {
                    kind: FileEffectKind::Write,
                    path: PathBuf::from("a.txt"),
                    content: Some(b"hi".to_vec()),
                    expected_preimage: None,
                    patch: None,
                    expect_absent: false,
                },
                before: None,
                before_metadata: None,
                preparation: ZeroHandle::from_digest(&"0".repeat(64)).unwrap(),
                receipt: None,
                restored: false,
            }],
            rollback_errors: Vec::new(),
        };
        persist_record(&record_path, &record).expect("persist");
        let original_bytes = fs::read(&record_path).expect("read original");
        let err = coordinator
            .reconcile(&invocation)
            .expect_err("first reconcile must error");
        let TransactionError::RecoveryRequired(messages) = err else {
            panic!("unexpected error variant");
        };
        assert!(!messages.is_empty(), "recovery must carry diagnostics");
        // Independent oracle: enumerate the transaction directory tree without
        // using the production quarantine-path helper. The active journal must
        // be gone and a single quarantine evidence file must preserve the
        // original record identity and content.
        assert!(!record_path.exists(), "active journal must be removed");
        let tx_root = dir.path().join("tx");
        let mut all_files = Vec::new();
        let mut stack = vec![tx_root.clone()];
        while let Some(dir_path) = stack.pop() {
            for entry in fs::read_dir(&dir_path).expect("read tx dir") {
                let entry = entry.expect("entry");
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    all_files.push(path);
                }
            }
        }
        // Exactly one file should remain: the quarantined evidence.
        assert_eq!(
            all_files.len(),
            1,
            "exactly one quarantined file expected, got {all_files:?}"
        );
        let quarantined_path = &all_files[0];
        assert_ne!(quarantined_path, &record_path);
        let quarantined_bytes = fs::read(quarantined_path).expect("read quarantined");
        assert_eq!(
            quarantined_bytes, original_bytes,
            "quarantined content must preserve original record"
        );
        let quarantined_record: TransactionRecord =
            serde_json::from_slice(&quarantined_bytes).expect("deserialize quarantined");
        assert_eq!(quarantined_record.session_id, session_id);
        assert_eq!(quarantined_record.cell_id, cell_id);
        assert_eq!(quarantined_record.effects, record.effects);
        // Active journals are those not quarantined; none should exist for this cell.
        assert!(!record_path.exists());
        let second = coordinator
            .reconcile(&invocation)
            .expect("second reconcile ok");
        assert!(
            second.is_empty(),
            "second reconcile should recover without retrying poisoned transaction"
        );
        // Quarantined evidence must still exist after recovery.
        assert!(quarantined_path.exists());
        let after_files: Vec<PathBuf> = {
            let mut v = Vec::new();
            let mut s = vec![tx_root];
            while let Some(d) = s.pop() {
                for e in fs::read_dir(&d).unwrap() {
                    let p = e.unwrap().path();
                    if p.is_dir() {
                        s.push(p);
                    } else {
                        v.push(p);
                    }
                }
            }
            v
        };
        assert_eq!(after_files.len(), 1);
        assert_eq!(fs::read(&after_files[0]).unwrap(), original_bytes);
    }
}
