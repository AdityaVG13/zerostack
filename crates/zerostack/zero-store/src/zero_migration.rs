//! Offline SHA-256 to BLAKE3 ZeroKernel store importer.
//!
//! This module is never used by runtime reads. Operators run it during a
//! write-frozen migration window, verify the signed manifest, then start the
//! new host with only `ZeroCas` enabled.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zero_abi::ZeroHandle;

use crate::fs_replace::{SyncPolicy, atomic_write_file_with_sync};
use crate::{ZERO_CAS_OBJECT_BYTE_LIMIT, ZeroCas, ZeroCasError};

pub const LEGACY_OBJECT_LAYOUT: &str = "blobs/sha256";
pub const MIGRATION_MANIFEST_BYTE_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ZeroMigrationError {
    #[error("migration I/O: {0}")]
    Io(String),
    #[error("legacy object is malformed: {0}")]
    Malformed(String),
    #[error("legacy object digest mismatch: expected {expected}, observed {actual}")]
    LegacyDigestMismatch { expected: String, actual: String },
    #[error("new CAS: {0}")]
    Cas(#[from] ZeroCasError),
    #[error("migration manifest signature mismatch")]
    SignatureMismatch,
}

fn io(context: &str, error: impl std::fmt::Display) -> ZeroMigrationError {
    ZeroMigrationError::Io(format!("{context}: {error}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroMigrationEntry {
    pub legacy_sha256: String,
    pub zero_handle: ZeroHandle,
    pub byte_len: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroMigrationManifest {
    pub source_root: PathBuf,
    pub destination_root: PathBuf,
    pub entries: Vec<ZeroMigrationEntry>,
    pub total_bytes: u64,
    pub signature: String,
}

impl ZeroMigrationManifest {
    fn unsigned_bytes(&self) -> Result<Vec<u8>, ZeroMigrationError> {
        #[derive(Serialize)]
        struct Unsigned<'a> {
            source_root: &'a Path,
            destination_root: &'a Path,
            entries: &'a [ZeroMigrationEntry],
            total_bytes: u64,
        }
        serde_json::to_vec(&Unsigned {
            source_root: &self.source_root,
            destination_root: &self.destination_root,
            entries: &self.entries,
            total_bytes: self.total_bytes,
        })
        .map_err(|error| ZeroMigrationError::Malformed(error.to_string()))
    }

    pub fn sign(&mut self, key: &[u8; 32]) -> Result<(), ZeroMigrationError> {
        self.signature = blake3::keyed_hash(key, &self.unsigned_bytes()?)
            .to_hex()
            .to_string();
        Ok(())
    }

    pub fn verify(&self, key: &[u8; 32]) -> Result<(), ZeroMigrationError> {
        let expected = blake3::keyed_hash(key, &self.unsigned_bytes()?)
            .to_hex()
            .to_string();
        if expected != self.signature {
            return Err(ZeroMigrationError::SignatureMismatch);
        }
        Ok(())
    }
}

fn legacy_objects(root: &Path) -> Result<Vec<PathBuf>, ZeroMigrationError> {
    let old_root = root.join(LEGACY_OBJECT_LAYOUT);
    let mut objects = Vec::new();
    let shards = match fs::read_dir(&old_root) {
        Ok(shards) => shards,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(objects),
        Err(error) => return Err(io("read legacy CAS root", error)),
    };
    for shard in shards {
        let shard = shard.map_err(|error| io("read legacy shard", error))?;
        if !shard
            .file_type()
            .map_err(|error| io("stat legacy shard", error))?
            .is_dir()
        {
            continue;
        }
        for object in
            fs::read_dir(shard.path()).map_err(|error| io("read legacy shard objects", error))?
        {
            let object = object.map_err(|error| io("read legacy object entry", error))?;
            let metadata = object
                .file_type()
                .map_err(|error| io("stat legacy object", error))?;
            if !metadata.is_file() || metadata.is_symlink() {
                continue;
            }
            let name = object.file_name().to_string_lossy().into_owned();
            if is_digest(&name) {
                objects.push(object.path());
            }
        }
    }
    objects.sort();
    Ok(objects)
}

pub fn import_legacy_store(
    source_root: &Path,
    destination_root: &Path,
    manifest_path: &Path,
    signing_key: &[u8; 32],
) -> Result<ZeroMigrationManifest, ZeroMigrationError> {
    let destination = ZeroCas::open(destination_root);
    let mut entries = Vec::new();
    let mut total_bytes = 0_u64;
    for path in legacy_objects(source_root)? {
        let expected = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| ZeroMigrationError::Malformed("legacy object has no UTF-8 name".into()))?
            .to_owned();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| io("stat legacy object before import", error))?;
        if !metadata.file_type().is_file() || metadata.len() > ZERO_CAS_OBJECT_BYTE_LIMIT {
            return Err(ZeroMigrationError::Malformed(format!(
                "legacy object {expected} is not a bounded regular file"
            )));
        }
        let bytes = fs::read(&path).map_err(|error| io("read legacy object", error))?;
        let actual = sha256_hex(&bytes);
        if expected != actual {
            return Err(ZeroMigrationError::LegacyDigestMismatch { expected, actual });
        }
        let zero_handle = destination.put(&bytes)?;
        if destination.get(&zero_handle)? != bytes {
            return Err(ZeroMigrationError::Malformed(
                "new CAS did not round-trip imported bytes".into(),
            ));
        }
        total_bytes = total_bytes.saturating_add(bytes.len() as u64);
        entries.push(ZeroMigrationEntry {
            legacy_sha256: actual,
            zero_handle,
            byte_len: bytes.len() as u64,
        });
    }
    entries.sort_by(|left, right| left.legacy_sha256.cmp(&right.legacy_sha256));
    let mut manifest = ZeroMigrationManifest {
        source_root: source_root.to_path_buf(),
        destination_root: destination_root.to_path_buf(),
        entries,
        total_bytes,
        signature: String::new(),
    };
    manifest.sign(signing_key)?;
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| ZeroMigrationError::Malformed(error.to_string()))?;
    if bytes.len() > MIGRATION_MANIFEST_BYTE_LIMIT {
        return Err(ZeroMigrationError::Malformed(
            "migration manifest exceeds policy".into(),
        ));
    }
    atomic_write_file_with_sync(manifest_path, &bytes, SyncPolicy::Required)
        .map_err(|error| io("publish migration manifest", error))?;
    Ok(manifest)
}

pub fn read_and_verify_manifest(
    path: &Path,
    signing_key: &[u8; 32],
) -> Result<ZeroMigrationManifest, ZeroMigrationError> {
    let metadata = fs::metadata(path).map_err(|error| io("stat migration manifest", error))?;
    if metadata.len() as usize > MIGRATION_MANIFEST_BYTE_LIMIT {
        return Err(ZeroMigrationError::Malformed(
            "migration manifest exceeds policy".into(),
        ));
    }
    let bytes = fs::read(path).map_err(|error| io("read migration manifest", error))?;
    let manifest: ZeroMigrationManifest = serde_json::from_slice(&bytes)
        .map_err(|error| ZeroMigrationError::Malformed(error.to_string()))?;
    manifest.verify(signing_key)?;
    Ok(manifest)
}
