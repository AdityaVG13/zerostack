//! Append-only ZeroKernel event publication. A model-visible result is valid only after its event
//! object and append-only session-log entry are durable. Failure returns no publication receipt, so
//! a caller cannot expose bytes that are absent from the log.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zero_abi::{ProviderUsageObservation, ZeroHandle, ZeroKernelEvent};

use crate::gc_lock::{LOCK_DEADLINE, StoreLock};
use crate::{ZeroCas, ZeroCasError};

pub const EVENT_LOG_DIR: &str = "events";
pub const EVENT_LOG_BYTE_LIMIT: u64 = 64 * 1024 * 1024;
pub const EVENT_RECORD_BYTE_LIMIT: usize = 16 * 1024;
pub const USAGE_LOG_DIR: &str = "usage";

#[derive(Debug, thiserror::Error)]
pub enum EventLogError {
    #[error("invalid ZeroKernel event: {0}")]
    Invalid(String),
    #[error("invalid provider usage: {0}")]
    UsageInvalid(String),
    #[error("provider usage conflict: {0}")]
    UsageConflict(String),
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderUsageLogRecord {
    pub kernel_event: ZeroHandle,
    pub request_id: String,
    pub observation: ZeroHandle,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderUsagePublication {
    pub kernel_event: ZeroHandle,
    pub request_id: String,
    pub observation: ZeroHandle,
    pub observation_digest: String,
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

    fn usage_log_path(&self, session_id: &str) -> PathBuf {
        let session_digest = digest_hex(session_id.as_bytes());
        self.root
            .join(USAGE_LOG_DIR)
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

    pub fn publish_provider_usage(
        &self,
        session_id: &str,
        kernel_event_handle: &ZeroHandle,
        observation: ProviderUsageObservation,
    ) -> Result<ProviderUsagePublication, EventLogError> {
        observation
            .validate()
            .map_err(|error| EventLogError::UsageInvalid(error.to_string()))?;
        let event_bytes = self.cas.get(kernel_event_handle)?;
        let event: ZeroKernelEvent = serde_json::from_slice(&event_bytes)
            .map_err(|error| EventLogError::UsageInvalid(error.to_string()))?;
        event
            .validate()
            .map_err(|error| EventLogError::UsageInvalid(error.to_string()))?;
        if event.session_id != session_id {
            return Err(EventLogError::UsageInvalid(format!(
                "kernel event session {} does not match session {session_id}",
                event.session_id
            )));
        }
        let observation_bytes = serde_json::to_vec(&observation)
            .map_err(|error| EventLogError::UsageInvalid(error.to_string()))?;
        if observation_bytes.len() > EVENT_RECORD_BYTE_LIMIT {
            return Err(EventLogError::UsageInvalid(format!(
                "observation is {} bytes, limit is {EVENT_RECORD_BYTE_LIMIT}",
                observation_bytes.len()
            )));
        }
        let observation_handle = self.cas.put(&observation_bytes)?;
        let record = ProviderUsageLogRecord {
            kernel_event: kernel_event_handle.clone(),
            request_id: observation.request_id.clone(),
            observation: observation_handle.clone(),
        };
        let mut record_bytes = serde_json::to_vec(&record)
            .map_err(|error| EventLogError::UsageInvalid(error.to_string()))?;
        record_bytes.push(b'\n');

        let _guard = StoreLock::publish(&self.root, LOCK_DEADLINE)
            .map_err(|error| io("acquire usage append lock", error))?;
        if !self
            .records(session_id)?
            .iter()
            .any(|record| record.event == *kernel_event_handle)
        {
            return Err(EventLogError::UsageInvalid(format!(
                "kernel event {kernel_event_handle} is absent from the durable session log"
            )));
        }
        let path = self.usage_log_path(session_id);
        for existing in self.existing_usage_records(&path)? {
            if existing.kernel_event != *kernel_event_handle
                || existing.request_id != observation.request_id
            {
                continue;
            }
            if existing.observation == observation_handle {
                return Ok(ProviderUsagePublication {
                    kernel_event: kernel_event_handle.clone(),
                    request_id: observation.request_id,
                    observation: observation_handle,
                    observation_digest: digest_hex(&observation_bytes),
                });
            }
            return Err(EventLogError::UsageConflict(format!(
                "kernel event {kernel_event_handle} already recorded for request {} with a different observation",
                observation.request_id
            )));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| io("create usage log directory", error))?;
        }
        let current_len = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        if current_len.saturating_add(record_bytes.len() as u64) > EVENT_LOG_BYTE_LIMIT {
            return Err(EventLogError::UsageInvalid(format!(
                "session usage log would exceed {EVENT_LOG_BYTE_LIMIT} bytes"
            )));
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| io("open usage log", error))?;
        file.write_all(&record_bytes)
            .map_err(|error| io("append usage log", error))?;
        file.sync_all()
            .map_err(|error| io("sync usage log", error))?;

        Ok(ProviderUsagePublication {
            kernel_event: kernel_event_handle.clone(),
            request_id: observation.request_id,
            observation: observation_handle,
            observation_digest: digest_hex(&observation_bytes),
        })
    }

    fn existing_usage_records(
        &self,
        path: &Path,
    ) -> Result<Vec<ProviderUsageLogRecord>, EventLogError> {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io("open usage log", error)),
        };
        if file
            .metadata()
            .map_err(|error| io("stat usage log", error))?
            .len()
            > EVENT_LOG_BYTE_LIMIT
        {
            return Err(EventLogError::UsageInvalid(
                "session usage log exceeds policy".into(),
            ));
        }
        let mut records = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|error| io("read usage log", error))?;
            if line.len() > EVENT_RECORD_BYTE_LIMIT {
                return Err(EventLogError::UsageInvalid(
                    "usage log record exceeds policy".into(),
                ));
            }
            let record: ProviderUsageLogRecord = serde_json::from_str(&line)
                .map_err(|error| EventLogError::UsageInvalid(error.to_string()))?;
            records.push(record);
        }
        Ok(records)
    }

    pub fn provider_usage_records(
        &self,
        session_id: &str,
    ) -> Result<Vec<ProviderUsageLogRecord>, EventLogError> {
        let path = self.usage_log_path(session_id);
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io("open usage log for replay", error)),
        };
        if file
            .metadata()
            .map_err(|error| io("stat usage log", error))?
            .len()
            > EVENT_LOG_BYTE_LIMIT
        {
            return Err(EventLogError::UsageInvalid(
                "session usage log exceeds policy".into(),
            ));
        }
        let mut records = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|error| io("read usage log", error))?;
            if line.len() > EVENT_RECORD_BYTE_LIMIT {
                return Err(EventLogError::UsageInvalid(
                    "usage log record exceeds policy".into(),
                ));
            }
            let record: ProviderUsageLogRecord = serde_json::from_str(&line)
                .map_err(|error| EventLogError::UsageInvalid(error.to_string()))?;
            // Resolve every linked object while replaying. A missing/corrupt
            // event or observation invalidates the log instead of silently
            // skipping history.
            let event_bytes = self.cas.get(&record.kernel_event)?;
            let event: ZeroKernelEvent = serde_json::from_slice(&event_bytes)
                .map_err(|error| EventLogError::UsageInvalid(error.to_string()))?;
            event
                .validate()
                .map_err(|error| EventLogError::UsageInvalid(error.to_string()))?;
            if event.session_id != session_id {
                return Err(EventLogError::UsageInvalid(format!(
                    "usage record event session {} does not match log session {session_id}",
                    event.session_id
                )));
            }
            let observation_bytes = self.cas.get(&record.observation)?;
            let observation: ProviderUsageObservation = serde_json::from_slice(&observation_bytes)
                .map_err(|error| EventLogError::UsageInvalid(error.to_string()))?;
            observation
                .validate()
                .map_err(|error| EventLogError::UsageInvalid(error.to_string()))?;
            if observation.request_id != record.request_id {
                return Err(EventLogError::UsageInvalid(
                    "usage record does not match its observation object".into(),
                ));
            }
            records.push(record);
        }
        Ok(records)
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
