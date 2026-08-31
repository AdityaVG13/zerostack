#![allow(unsafe_code)]
//! Canonical BLAKE3 CAS for ZeroKernel. The only unsafe operation is creation of a read-only memory
//! map.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::ops::Deref;
use std::path::{Path, PathBuf};

use memmap2::{Mmap, MmapOptions};
use serde::{Deserialize, Serialize};
use zero_abi::{ExpandOptions, ZeroHandle};

use crate::fs_replace::{SyncPolicy, atomic_write_file_with_sync};
use crate::gc_lock::{LOCK_DEADLINE, StoreLock};

pub const ZERO_CAS_LAYOUT: &str = "blobs/blake3/<hh>/<digest>";
pub const ZERO_CAS_OBJECT_BYTE_LIMIT: u64 = 256 * 1024 * 1024;
pub const ZERO_CAS_INDEX_BYTE_LIMIT: u64 = 4 * 1024 * 1024;
const OBJECTS_DIR: &str = "blobs";
const ALGORITHM_DIR: &str = "blake3";
const INDEX_DIR: &str = "indexes";

#[derive(Debug, thiserror::Error)]
pub enum ZeroCasError {
    #[error("invalid ZeroHandle: {0}")]
    InvalidHandle(String),
    #[error("object exceeds policy: {0}")]
    Policy(String),
    #[error("object not found")]
    NotFound,
    #[error("CAS corruption: expected {expected}, observed {actual}")]
    Corrupt { expected: String, actual: String },
    #[error("invalid selection: {0}")]
    InvalidSelection(String),
    #[error("CAS I/O: {0}")]
    Io(String),
}

fn io(context: &str, error: impl std::fmt::Display) -> ZeroCasError {
    ZeroCasError::Io(format!("{context}: {error}"))
}

fn digest_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn verified_digest(handle: &ZeroHandle, bytes: &[u8]) -> Result<(), ZeroCasError> {
    let actual = digest_hex(bytes);
    if actual != handle.digest() {
        return Err(ZeroCasError::Corrupt {
            expected: handle.digest().to_owned(),
            actual,
        });
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolSelection {
    pub byte_start: u64,
    pub byte_end: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionIndex {
    /// Byte offset of each one-based source line. The first entry is always 0.
    pub line_starts: Vec<u64>,
    #[serde(default)]
    pub symbols: BTreeMap<String, SymbolSelection>,
}

impl SelectionIndex {
    pub fn from_utf8(text: &str) -> Self {
        let mut line_starts = vec![0];
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' && index + 1 < text.len() {
                line_starts.push((index + 1) as u64);
            }
        }
        Self {
            line_starts,
            symbols: BTreeMap::new(),
        }
    }

    pub fn with_symbol(
        mut self,
        symbol: impl Into<String>,
        byte_start: u64,
        byte_end: u64,
    ) -> Result<Self, ZeroCasError> {
        if byte_start > byte_end {
            return Err(ZeroCasError::InvalidSelection(
                "symbol byte_start exceeds byte_end".into(),
            ));
        }
        self.symbols.insert(
            symbol.into(),
            SymbolSelection {
                byte_start,
                byte_end,
            },
        );
        Ok(self)
    }

    fn line_range(
        &self,
        start: u32,
        end: u32,
        total_bytes: usize,
    ) -> Result<std::ops::Range<usize>, ZeroCasError> {
        if start == 0 || end < start {
            return Err(ZeroCasError::InvalidSelection(
                "line range must be one-based and inclusive".into(),
            ));
        }
        let start_index = usize::try_from(start - 1).map_err(|_| {
            ZeroCasError::InvalidSelection("line start does not fit this platform".into())
        })?;
        if start_index >= self.line_starts.len() {
            return Err(ZeroCasError::InvalidSelection(
                "line start is beyond the object".into(),
            ));
        }
        let end_index = usize::try_from(end).map_err(|_| {
            ZeroCasError::InvalidSelection("line end does not fit this platform".into())
        })?;
        let byte_start = usize::try_from(self.line_starts[start_index]).map_err(|_| {
            ZeroCasError::InvalidSelection("line offset does not fit this platform".into())
        })?;
        let byte_end = self
            .line_starts
            .get(end_index)
            .copied()
            .map(usize::try_from)
            .transpose()
            .map_err(|_| {
                ZeroCasError::InvalidSelection("line offset does not fit this platform".into())
            })?
            .unwrap_or(total_bytes);
        Ok(byte_start..byte_end.min(total_bytes))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroObjectMetadata {
    pub handle: ZeroHandle,
    pub byte_len: u64,
    pub media_type: String,
    pub producer: String,
    pub contract_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<SelectionIndex>,
}

/// A verified read-only mapping of one immutable ZeroKernel CAS object.
pub struct MappedBlob {
    handle: ZeroHandle,
    map: Mmap,
}

impl MappedBlob {
    pub fn handle(&self) -> &ZeroHandle {
        &self.handle
    }

    pub fn bytes(&self) -> &[u8] {
        &self.map
    }
}

impl Deref for MappedBlob {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.bytes()
    }
}

impl std::fmt::Debug for MappedBlob {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MappedBlob")
            .field("handle", &self.handle)
            .field("byte_len", &self.map.len())
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpandedBlob {
    pub bytes: Vec<u8>,
    pub byte_start: u64,
    pub byte_end: u64,
    pub byte_length: u64,
}

#[derive(Clone, Debug)]
pub struct ZeroCas {
    root: PathBuf,
}

impl ZeroCas {
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn object_path(&self, handle: &ZeroHandle) -> PathBuf {
        self.root
            .join(OBJECTS_DIR)
            .join(ALGORITHM_DIR)
            .join(&handle.digest()[..2])
            .join(handle.digest())
    }

    pub fn metadata_path(&self, handle: &ZeroHandle) -> PathBuf {
        self.root
            .join(INDEX_DIR)
            .join(ALGORITHM_DIR)
            .join(&handle.digest()[..2])
            .join(format!("{}.json", handle.digest()))
    }

    pub fn contains(&self, handle: &ZeroHandle) -> bool {
        fs::symlink_metadata(self.object_path(handle))
            .map(|metadata| metadata.file_type().is_file())
            .unwrap_or(false)
    }

    pub fn put(&self, bytes: &[u8]) -> Result<ZeroHandle, ZeroCasError> {
        self.put_limited(bytes, ZERO_CAS_OBJECT_BYTE_LIMIT)
    }

    pub fn put_limited(&self, bytes: &[u8], limit: u64) -> Result<ZeroHandle, ZeroCasError> {
        let guard = StoreLock::publish(&self.root, LOCK_DEADLINE)
            .map_err(|error| io("acquire publish lock", error))?;
        self.put_in_lock(bytes, limit, &guard)
    }

    /// Publish while the caller holds this store's shared or exclusive
    /// coordination lock. This is used when an object and the pointer that
    /// names it must become one transaction boundary.
    pub fn put_in_lock(
        &self,
        bytes: &[u8],
        limit: u64,
        guard: &StoreLock,
    ) -> Result<ZeroHandle, ZeroCasError> {
        if !guard.is_for_store_root(&self.root) {
            return Err(ZeroCasError::Policy(
                "store lock belongs to a different root".into(),
            ));
        }
        self.put_unlocked(bytes, limit)
    }

    fn put_unlocked(&self, bytes: &[u8], limit: u64) -> Result<ZeroHandle, ZeroCasError> {
        if bytes.len() as u64 > limit.min(ZERO_CAS_OBJECT_BYTE_LIMIT) {
            return Err(ZeroCasError::Policy(format!(
                "{} bytes exceeds {limit}",
                bytes.len()
            )));
        }
        let handle = ZeroHandle::from_digest(&digest_hex(bytes))
            .map_err(|error| ZeroCasError::InvalidHandle(error.to_string()))?;
        let path = self.object_path(&handle);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() => {
                let existing = self.get_limited(&handle, limit)?;
                verified_digest(&handle, &existing)?;
                return Ok(handle);
            }
            Ok(_) => {
                return Err(ZeroCasError::Io(
                    "canonical object path is not a regular file".into(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io("stat canonical object", error)),
        }
        atomic_write_file_with_sync(&path, bytes, SyncPolicy::Required)
            .map_err(|error| io("publish BLAKE3 object", error))?;
        let published = self.get_limited(&handle, limit)?;
        verified_digest(&handle, &published)?;
        Ok(handle)
    }

    pub fn get(&self, handle: &ZeroHandle) -> Result<Vec<u8>, ZeroCasError> {
        self.get_limited(handle, ZERO_CAS_OBJECT_BYTE_LIMIT)
    }

    pub fn get_limited(&self, handle: &ZeroHandle, limit: u64) -> Result<Vec<u8>, ZeroCasError> {
        let path = self.object_path(handle);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() => metadata,
            Ok(_) => {
                return Err(ZeroCasError::Io(
                    "canonical object path is not a regular file".into(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ZeroCasError::NotFound);
            }
            Err(error) => return Err(io("stat BLAKE3 object", error)),
        };
        if metadata.len() > limit.min(ZERO_CAS_OBJECT_BYTE_LIMIT) {
            return Err(ZeroCasError::Policy(format!(
                "object is {} bytes, limit is {limit}",
                metadata.len()
            )));
        }
        let bytes = fs::read(&path).map_err(|error| io("read BLAKE3 object", error))?;
        verified_digest(handle, &bytes)?;
        Ok(bytes)
    }

    pub fn map(&self, handle: &ZeroHandle) -> Result<MappedBlob, ZeroCasError> {
        let path = self.object_path(handle);
        let file = File::open(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ZeroCasError::NotFound
            } else {
                io("open BLAKE3 object", error)
            }
        })?;
        let metadata = file
            .metadata()
            .map_err(|error| io("stat BLAKE3 object", error))?;
        if !metadata.file_type().is_file() {
            return Err(ZeroCasError::Io("mapped object is not regular".into()));
        }
        if metadata.len() > ZERO_CAS_OBJECT_BYTE_LIMIT {
            return Err(ZeroCasError::Policy(format!(
                "mapped object is {} bytes",
                metadata.len()
            )));
        }
        // SAFETY: documented module invariant above. The file is regular,
        // read-only, size-bounded, immutable by CAS contract, and re-hashed.
        let map = unsafe { MmapOptions::new().map(&file) }
            .map_err(|error| io("map BLAKE3 object", error))?;
        verified_digest(handle, &map)?;
        Ok(MappedBlob {
            handle: handle.clone(),
            map,
        })
    }

    pub fn publish_metadata(&self, metadata: &ZeroObjectMetadata) -> Result<(), ZeroCasError> {
        if !self.contains(&metadata.handle) {
            return Err(ZeroCasError::NotFound);
        }
        let bytes = serde_json::to_vec(metadata)
            .map_err(|error| ZeroCasError::InvalidSelection(error.to_string()))?;
        if bytes.len() as u64 > ZERO_CAS_INDEX_BYTE_LIMIT {
            return Err(ZeroCasError::Policy(format!(
                "selection metadata is {} bytes",
                bytes.len()
            )));
        }
        atomic_write_file_with_sync(
            &self.metadata_path(&metadata.handle),
            &bytes,
            SyncPolicy::Required,
        )
        .map_err(|error| io("publish selection metadata", error))
    }

    pub fn metadata(
        &self,
        handle: &ZeroHandle,
    ) -> Result<Option<ZeroObjectMetadata>, ZeroCasError> {
        let path = self.metadata_path(handle);
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io("read selection metadata", error)),
        };
        if bytes.len() as u64 > ZERO_CAS_INDEX_BYTE_LIMIT {
            return Err(ZeroCasError::Policy(
                "selection metadata exceeds policy".into(),
            ));
        }
        let metadata: ZeroObjectMetadata = serde_json::from_slice(&bytes)
            .map_err(|error| ZeroCasError::InvalidSelection(error.to_string()))?;
        if &metadata.handle != handle {
            return Err(ZeroCasError::InvalidSelection(
                "selection metadata handle mismatch".into(),
            ));
        }
        Ok(Some(metadata))
    }

    pub fn expand(
        &self,
        handle: &ZeroHandle,
        options: &ExpandOptions,
    ) -> Result<Vec<u8>, ZeroCasError> {
        self.expand_with_range(handle, options)
            .map(|expanded| expanded.bytes)
    }

    pub fn expand_with_range(
        &self,
        handle: &ZeroHandle,
        options: &ExpandOptions,
    ) -> Result<ExpandedBlob, ZeroCasError> {
        let mapped = self.map(handle)?;
        let total = mapped.len();
        let range = if let Some(symbol) = options.symbol.as_deref() {
            let metadata = self.metadata(handle)?.ok_or_else(|| {
                ZeroCasError::InvalidSelection("symbol expansion requires metadata".into())
            })?;
            let selection = metadata
                .selection
                .and_then(|index| index.symbols.get(symbol).cloned())
                .ok_or_else(|| {
                    ZeroCasError::InvalidSelection(format!("unknown symbol {symbol}"))
                })?;
            let start = usize::try_from(selection.byte_start).map_err(|_| {
                ZeroCasError::InvalidSelection("symbol start does not fit platform".into())
            })?;
            let end = usize::try_from(selection.byte_end).map_err(|_| {
                ZeroCasError::InvalidSelection("symbol end does not fit platform".into())
            })?;
            start.min(total)..end.min(total)
        } else if let (Some(start), Some(end)) = (options.line_start, options.line_end) {
            if let Some(index) = self
                .metadata(handle)?
                .and_then(|metadata| metadata.selection)
            {
                index.line_range(start, end, total)?
            } else {
                let text = std::str::from_utf8(&mapped).map_err(|_| {
                    ZeroCasError::InvalidSelection("line expansion requires UTF-8 bytes".into())
                })?;
                SelectionIndex::from_utf8(text).line_range(start, end, total)?
            }
        } else {
            let start = usize::try_from(options.offset.unwrap_or(0)).map_err(|_| {
                ZeroCasError::InvalidSelection("offset does not fit platform".into())
            })?;
            let requested =
                usize::try_from(options.limit.unwrap_or(total as u64)).map_err(|_| {
                    ZeroCasError::InvalidSelection("limit does not fit platform".into())
                })?;
            let start = start.min(total);
            start..start.saturating_add(requested).min(total)
        };
        if range.start > range.end {
            return Err(ZeroCasError::InvalidSelection(
                "selection start exceeds end".into(),
            ));
        }
        Ok(ExpandedBlob {
            bytes: mapped[range.clone()].to_vec(),
            byte_start: range.start as u64,
            byte_end: range.end as u64,
            byte_length: total as u64,
        })
    }
}
