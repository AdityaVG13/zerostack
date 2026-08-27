use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use zero_abi::{
    STATE_KEY_BYTE_LIMIT, STATE_KEY_LIMIT, STATE_TOTAL_BYTE_LIMIT, STATE_VALUE_BYTE_LIMIT,
    ZeroHandle,
};
use zero_store::{
    LOCK_DEADLINE, StoreLock, SyncPolicy, ZERO_CAS_OBJECT_BYTE_LIMIT, ZeroCas,
    atomic_write_file_with_sync,
};

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("state store I/O: {0}")]
    Io(String),
    #[error("state root conflict: expected {expected:?}, current {current:?}")]
    Conflict {
        expected: Option<String>,
        current: Option<String>,
    },
    #[error("invalid state: {0}")]
    Invalid(String),
}

fn io(context: &str, error: impl std::fmt::Display) -> StateError {
    StateError::Io(format!("{context}: {error}"))
}

#[derive(Clone, Debug, Default)]
pub struct StateSnapshot {
    pub root: Option<ZeroHandle>,
    pub values: BTreeMap<String, Value>,
}

#[derive(Clone, Debug)]
pub struct StateStore {
    root: PathBuf,
    pointer: PathBuf,
    cas: ZeroCas,
}

impl StateStore {
    pub fn open(root: impl Into<PathBuf>, session_id: &str) -> Self {
        let root = root.into();
        let session_digest = blake3::hash(session_id.as_bytes()).to_hex().to_string();
        let pointer = root.join("state").join(format!("{session_digest}.root"));
        Self {
            cas: ZeroCas::open(root.clone()),
            root,
            pointer,
        }
    }

    pub fn current_root(&self) -> Result<Option<ZeroHandle>, StateError> {
        read_pointer(&self.pointer)
    }

    pub fn load(&self, expected: Option<&ZeroHandle>) -> Result<StateSnapshot, StateError> {
        let current = self.current_root()?;
        if let Some(expected) = expected
            && current.as_ref() != Some(expected)
        {
            return Err(StateError::Conflict {
                expected: Some(expected.to_string()),
                current: current.map(|handle| handle.to_string()),
            });
        }
        let Some(root) = current else {
            return Ok(StateSnapshot::default());
        };
        let bytes = self
            .cas
            .get(&root)
            .map_err(|error| io("read state object", error))?;
        let values: BTreeMap<String, Value> = serde_json::from_slice(&bytes)
            .map_err(|error| StateError::Invalid(error.to_string()))?;
        validate_values(&values)?;
        Ok(StateSnapshot {
            root: Some(root),
            values,
        })
    }

    pub fn commit(
        &self,
        expected: Option<&ZeroHandle>,
        values: &BTreeMap<String, Value>,
    ) -> Result<ZeroHandle, StateError> {
        validate_values(values)?;
        let bytes =
            serde_json::to_vec(values).map_err(|error| StateError::Invalid(error.to_string()))?;
        let guard = StoreLock::sweep(&self.root, LOCK_DEADLINE)
            .map_err(|error| io("acquire state commit lock", error))?;
        let current = read_pointer(&self.pointer)?;
        if current.as_ref() != expected {
            return Err(StateError::Conflict {
                expected: expected.map(ToString::to_string),
                current: current.map(|handle| handle.to_string()),
            });
        }
        let next = self
            .cas
            .put_in_lock(&bytes, ZERO_CAS_OBJECT_BYTE_LIMIT, &guard)
            .map_err(|error| io("publish state object", error))?;
        atomic_write_file_with_sync(
            &self.pointer,
            format!("{next}\n").as_bytes(),
            SyncPolicy::Required,
        )
        .map_err(|error| io("publish state root", error))?;
        Ok(next)
    }

    pub fn compare_and_set_root(
        &self,
        current: Option<&ZeroHandle>,
        target: Option<&ZeroHandle>,
    ) -> Result<(), StateError> {
        let _guard = StoreLock::sweep(&self.root, LOCK_DEADLINE)
            .map_err(|error| io("acquire state rollback lock", error))?;
        let observed = read_pointer(&self.pointer)?;
        if observed.as_ref() != current {
            return Err(StateError::Conflict {
                expected: current.map(ToString::to_string),
                current: observed.map(|handle| handle.to_string()),
            });
        }
        match target {
            Some(handle) => atomic_write_file_with_sync(
                &self.pointer,
                format!("{handle}\n").as_bytes(),
                SyncPolicy::Required,
            )
            .map_err(|error| io("restore state root", error)),
            None => match fs::remove_file(&self.pointer) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(io("remove state root", error)),
            },
        }
    }

    pub fn pointer_path(&self) -> &Path {
        &self.pointer
    }
}

fn read_pointer(path: &Path) -> Result<Option<ZeroHandle>, StateError> {
    let value = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io("read state root", error)),
    };
    ZeroHandle::parse(value.trim())
        .map(Some)
        .map_err(|error| StateError::Invalid(error.to_string()))
}

fn validate_values(values: &BTreeMap<String, Value>) -> Result<(), StateError> {
    if values.len() > STATE_KEY_LIMIT {
        return Err(StateError::Invalid(format!(
            "state has {} keys, limit is {STATE_KEY_LIMIT}",
            values.len()
        )));
    }
    let mut total = 0_usize;
    for (key, value) in values {
        if key.is_empty() || key.len() > STATE_KEY_BYTE_LIMIT {
            return Err(StateError::Invalid(format!(
                "state key must be 1..={STATE_KEY_BYTE_LIMIT} bytes"
            )));
        }
        let bytes =
            serde_json::to_vec(value).map_err(|error| StateError::Invalid(error.to_string()))?;
        if bytes.len() > STATE_VALUE_BYTE_LIMIT {
            return Err(StateError::Invalid(format!(
                "state value for {key:?} is {} bytes, limit is {STATE_VALUE_BYTE_LIMIT}",
                bytes.len()
            )));
        }
        total = total.saturating_add(bytes.len());
    }
    if total > STATE_TOTAL_BYTE_LIMIT {
        return Err(StateError::Invalid(format!(
            "state values total {total} bytes, limit is {STATE_TOTAL_BYTE_LIMIT}"
        )));
    }
    Ok(())
}
