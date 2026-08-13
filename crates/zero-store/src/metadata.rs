//! Append-only provenance events stored outside immutable object bytes.
//!
//! Events use canonical JSON content digests as names. Retrieval ignores
//! non-canonical debris and reports corrupt canonical events as malformed.
//! Publication is atomic but intentionally adds no fsync durability barrier.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{CasError, SharedCas};

const MAX_EVENT_BYTES: usize = 64 * 1024;
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Wire-safe provenance for one observation of an immutable object.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ObservationMetadata {
    pub source_engine: String,
    pub session: String,
    /// Source-preserved timestamp, conventionally RFC 3339.
    pub timestamp: String,
    pub declared_kind: String,
}

impl SharedCas {
    /// Use the existing object identity path, then append the metadata event.
    /// Metadata failure never changes an already-published object.
    pub fn ingest_with_metadata(
        &self,
        bytes: &[u8],
        metadata: ObservationMetadata,
    ) -> Result<String, CasError> {
        let id = self.put(bytes)?;
        self.append_observation(&id, &metadata)?;
        Ok(id)
    }

    /// Return events in deterministic event-digest order. Missing metadata and
    /// non-canonical debris are empty/ignored; corrupt canonical files are typed.
    pub fn observation_metadata(&self, id: &str) -> Result<Vec<ObservationMetadata>, CasError> {
        validate_id(id)?;
        let dir = event_dir(self.root(), id);
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io_err("read observation metadata", error)),
        };
        let mut events = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| io_err("read metadata entry", error))?;
            if let Some(event) = read_observation_event(entry)? {
                events.push(event);
            }
        }
        events.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(events.into_iter().map(|(_, value)| value).collect())
    }

    fn append_observation(&self, id: &str, value: &ObservationMetadata) -> Result<(), CasError> {
        validate_id(id)?;
        let bytes = serialize_observation(value)?;
        let digest = hash_hex(&bytes);
        let dir = event_dir(self.root(), id);
        ensure_real_dirs(self.root(), &dir)?;
        let dest = dir.join(format!("{digest}.json"));
        if dest.exists() {
            return verify_existing(&dest, &bytes);
        }
        let temp = dir.join(format!(
            ".tmp-{digest}-{}-{}",
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| io_err("create metadata temp", e))?;
        let result = publish_observation_event(file, &temp, &dest, &bytes);
        let _ = fs::remove_file(temp);
        result
    }
}

fn read_observation_event(
    entry: fs::DirEntry,
) -> Result<Option<(String, ObservationMetadata)>, CasError> {
    let name = entry.file_name().to_string_lossy().into_owned();
    let Some(digest) = canonical_event_digest(&name) else {
        return Ok(None);
    };
    if !entry
        .file_type()
        .map_err(|e| io_err("stat metadata event", e))?
        .is_file()
    {
        return Err(CasError::Malformed(format!(
            "metadata event '{name}' is not a regular file"
        )));
    }
    let bytes = fs::read(entry.path()).map_err(|e| io_err("read metadata event", e))?;
    if bytes.len() > MAX_EVENT_BYTES || hash_hex(&bytes) != digest {
        return Err(CasError::Malformed(format!(
            "metadata event '{name}' has invalid content"
        )));
    }
    let value = serde_json::from_slice(&bytes).map_err(|e| {
        CasError::Malformed(format!("metadata event '{name}' is invalid JSON: {e}"))
    })?;
    Ok(Some((digest.to_string(), value)))
}

fn serialize_observation(value: &ObservationMetadata) -> Result<Vec<u8>, CasError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|e| CasError::Malformed(format!("serialize metadata: {e}")))?;
    if bytes.len() > MAX_EVENT_BYTES {
        return Err(CasError::PolicyDenied(format!(
            "metadata event exceeds {MAX_EVENT_BYTES} bytes"
        )));
    }
    Ok(bytes)
}

fn publish_observation_event(
    mut file: fs::File,
    temp: &Path,
    dest: &Path,
    bytes: &[u8],
) -> Result<(), CasError> {
    file.write_all(bytes)
        .map_err(|e| io_err("write metadata temp", e))?;
    match fs::hard_link(temp, dest) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => verify_existing(dest, bytes),
        Err(e) => Err(io_err("publish metadata event", e)),
    }
}

fn event_dir(root: &Path, id: &str) -> PathBuf {
    root.join("metadata/observations/sha256")
        .join(&id[..2])
        .join(id)
}

fn ensure_real_dirs(root: &Path, dir: &Path) -> Result<(), CasError> {
    fs::create_dir_all(dir).map_err(|e| io_err("create metadata directory", e))?;
    let relative = dir
        .strip_prefix(root)
        .map_err(|_| CasError::Malformed("metadata path escaped store root".into()))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        if !fs::symlink_metadata(&current)
            .map_err(|e| io_err("stat metadata directory", e))?
            .file_type()
            .is_dir()
        {
            return Err(CasError::Malformed(
                "metadata directory is not a real directory".into(),
            ));
        }
    }
    Ok(())
}

fn verify_existing(path: &Path, expected: &[u8]) -> Result<(), CasError> {
    let meta = fs::symlink_metadata(path).map_err(|e| io_err("stat metadata event", e))?;
    if !meta.file_type().is_file() {
        return Err(CasError::Malformed(
            "metadata event is not a regular file".into(),
        ));
    }
    let actual = fs::read(path).map_err(|e| io_err("read metadata event", e))?;
    if actual == expected {
        Ok(())
    } else {
        Err(CasError::Malformed(
            "metadata digest collision or corrupt event".into(),
        ))
    }
}

fn validate_id(id: &str) -> Result<(), CasError> {
    if id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(CasError::Malformed(format!(
            "identity must be full lowercase 64-hex SHA-256, got '{id}'"
        )))
    }
}

fn canonical_event_digest(name: &str) -> Option<&str> {
    let id = name.strip_suffix(".json")?;
    (id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then_some(id)
}

fn hash_hex(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(bytes);
    let digest: [u8; 32] = hash.finalize().into();
    digest.iter().fold(String::with_capacity(64), |mut out, b| {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
        out
    })
}

fn io_err(context: &str, error: impl std::fmt::Display) -> CasError {
    CasError::Io(format!("{context}: {error}"))
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-store/unit/metadata.rs"]
mod tests;
