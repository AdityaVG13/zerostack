//! Append-only ZeroKernel event publication.
//!
//! A model-visible result is valid only after its event object and append-only
//! session-log entry are durable. Failure returns no publication receipt, so a
//! caller cannot expose bytes that are absent from the log.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zero_abi::{ZeroHandle, ZeroKernelEvent};

use crate::gc_lock::{LOCK_DEADLINE, StoreLock};
use crate::{ZeroCas, ZeroCasError};

pub const EVENT_LOG_DIR: &str = "events";
pub const EVENT_LOG_BYTE_LIMIT: u64 = 64 * 1024 * 1024;
pub const EVENT_RECORD_BYTE_LIMIT: usize = 16 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum EventLogError {
    #[error("invalid ZeroKernel event: {0}")]
    Invalid(String),
    #[error("event CAS: {0}")]
    Cas(#[from] ZeroCasError),
    #[error("event log I/O: {0}")]
    Io(String),
}

fn io(context: &str, error: impl std::fmt::Display) -> EventLogError {
    EventLogError::Io(format!("{context}: {error}"))
}

fn digest_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EventLogRecord {
    pub event: ZeroHandle,
    pub model_visible_digest: String,
    pub cell_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventPublication {
    pub event: ZeroHandle,
    pub model_visible_digest: String,
}

#[derive(Clone, Debug)]
pub struct EventLog {
    root: PathBuf,
    cas: ZeroCas,
}

impl EventLog {
    pub fn open(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            cas: ZeroCas::open(root.clone()),
            root,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn session_log_path(&self, session_id: &str) -> PathBuf {
        let session_digest = digest_hex(session_id.as_bytes());
        self.root
            .join(EVENT_LOG_DIR)
            .join(format!("{session_digest}.jsonl"))
    }

    pub fn publish(
        &self,
        event: &ZeroKernelEvent,
        model_visible_bytes: &[u8],
    ) -> Result<EventPublication, EventLogError> {
        event
            .validate()
            .map_err(|error| EventLogError::Invalid(error.to_string()))?;
        let visible_digest = digest_hex(model_visible_bytes);
        if event.model_visible_digest != visible_digest {
            return Err(EventLogError::Invalid(format!(
                "model-visible digest mismatch: event {}, bytes {visible_digest}",
                event.model_visible_digest
            )));
        }
        let event_bytes =
            serde_json::to_vec(event).map_err(|error| EventLogError::Invalid(error.to_string()))?;
        if event_bytes.len() > EVENT_RECORD_BYTE_LIMIT {
            return Err(EventLogError::Invalid(format!(
                "event is {} bytes, limit is {EVENT_RECORD_BYTE_LIMIT}",
                event_bytes.len()
            )));
        }
        let event_handle = self.cas.put(&event_bytes)?;
        let record = EventLogRecord {
            event: event_handle.clone(),
            model_visible_digest: visible_digest.clone(),
            cell_id: event.cell_id.clone(),
        };
        let mut record_bytes = serde_json::to_vec(&record)
            .map_err(|error| EventLogError::Invalid(error.to_string()))?;
        record_bytes.push(b'\n');

        let _guard = StoreLock::publish(&self.root, LOCK_DEADLINE)
            .map_err(|error| io("acquire event append lock", error))?;
        let path = self.session_log_path(&event.session_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| io("create event log directory", error))?;
        }
        let current_len = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        if current_len.saturating_add(record_bytes.len() as u64) > EVENT_LOG_BYTE_LIMIT {
            return Err(EventLogError::Invalid(format!(
                "session event log would exceed {EVENT_LOG_BYTE_LIMIT} bytes"
            )));
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| io("open event log", error))?;
        file.write_all(&record_bytes)
            .map_err(|error| io("append event log", error))?;
        file.sync_all()
            .map_err(|error| io("sync event log", error))?;

        Ok(EventPublication {
            event: event_handle,
            model_visible_digest: visible_digest,
        })
    }

    pub fn records(&self, session_id: &str) -> Result<Vec<EventLogRecord>, EventLogError> {
        let path = self.session_log_path(session_id);
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io("open event log for replay", error)),
        };
        if file
            .metadata()
            .map_err(|error| io("stat event log", error))?
            .len()
            > EVENT_LOG_BYTE_LIMIT
        {
            return Err(EventLogError::Invalid(
                "session event log exceeds policy".into(),
            ));
        }
        let mut records = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|error| io("read event log", error))?;
            if line.len() > EVENT_RECORD_BYTE_LIMIT {
                return Err(EventLogError::Invalid(
                    "event log record exceeds policy".into(),
                ));
            }
            let record: EventLogRecord = serde_json::from_str(&line)
                .map_err(|error| EventLogError::Invalid(error.to_string()))?;
            // Resolve every event while replaying. A missing/corrupt event
            // invalidates the log instead of silently skipping history.
            let bytes = self.cas.get(&record.event)?;
            let event: ZeroKernelEvent = serde_json::from_slice(&bytes)
                .map_err(|error| EventLogError::Invalid(error.to_string()))?;
            event
                .validate()
                .map_err(|error| EventLogError::Invalid(error.to_string()))?;
            if event.cell_id != record.cell_id
                || event.model_visible_digest != record.model_visible_digest
            {
                return Err(EventLogError::Invalid(
                    "event log record does not match its event object".into(),
                ));
            }
            records.push(record);
        }
        Ok(records)
    }
}
