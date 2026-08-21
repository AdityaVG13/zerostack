use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use zero_abi::{
    EngineError, EngineErrorKind, EngineInvocation, FileEffectReceipt, FileEffectRequest,
    FileEngine, FileLease, FileMetadata, FileReadRequest, ReadOptions, ZeroHandle,
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
                return Err(TransactionError::RecoveryRequired(
                    record.rollback_errors.clone(),
                ));
            }
            recovered.push(path);
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

impl Transaction {
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
