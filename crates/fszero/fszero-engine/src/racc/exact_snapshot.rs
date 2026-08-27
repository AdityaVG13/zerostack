//! Immutable exact snapshots of project file bytes (fszero-ip9y, V6-F2).
//!
//! Snapshot identity covers: canonical relative path, full-content digest,
//! length, declared POSIX mode bits, symlink target (never followed), the
//! declared toolchain/lockfile contract (when bound), and the declared
//! [`NonsemanticExclusion`] set. Exclusions are declared, never silent.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Default POSIX permission bits for a regular file when no mode is declared
/// (in-memory bytes, legacy records).
pub const DEFAULT_FILE_MODE: u32 = 0o644;

/// Metadata classes deliberately excluded from snapshot identity.
///
/// The exclusion is DECLARED (never silent): readers can see exactly which
/// filesystem metadata the snapshot does NOT cover. Exclusions are bound into
/// the root digest, so two snapshots that disagree on the declaration have
/// different identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NonsemanticExclusion {
    /// File modification time does not affect snapshot identity.
    Mtime,
    /// Ownership (uid/gid) does not affect snapshot identity.
    Ownership,
    /// Extended attributes (xattrs) do not affect snapshot identity.
    Xattrs,
    /// Device/inode identity (non-symlink) does not affect snapshot identity.
    Inode,
}

impl NonsemanticExclusion {
    /// The standard declared exclusion set for byte-exact project snapshots:
    /// content, mode and symlink target are covered; everything else is not.
    pub const fn standard() -> [Self; 4] {
        [Self::Mtime, Self::Ownership, Self::Xattrs, Self::Inode]
    }

    /// Stable wire tag; never derive from `Debug` formatting.
    pub(crate) fn tag(&self) -> &'static [u8] {
        match self {
            Self::Mtime => b"excl:mtime\0",
            Self::Ownership => b"excl:ownership\0",
            Self::Xattrs => b"excl:xattrs\0",
            Self::Inode => b"excl:inode\0",
        }
    }
}

fn default_nonsemantic_exclusions() -> Vec<NonsemanticExclusion> {
    NonsemanticExclusion::standard().to_vec()
}

/// One exact file record: canonical relative path + full-content digest +
/// length + declared semantic metadata (mode bits, symlink target).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: String,
    pub digest_hex: String,
    pub len: u64,
    /// POSIX permission bits (executable bit at minimum). Old snapshots
    /// deserialize with the non-executable regular-file default.
    #[serde(default = "default_file_mode")]
    pub mode: u32,
    /// Symlink target path when this record is a symlink. The digest covers
    /// the target string only; the referent is NEVER followed and its content
    /// is never digested as a regular file.
    #[serde(default)]
    pub symlink_target: Option<String>,
}

fn default_file_mode() -> u32 {
    DEFAULT_FILE_MODE
}

/// Declared toolchain/lockfile contract bound into snapshot coverage.
///
/// The identity derivation mirrors the memo cache key in `op_memo.rs`:
/// the tool identity is the `name@version` string (there, content-addressed
/// via a blob ref; here, hashed directly into the snapshot). Lockfile inputs
/// are declared as content refs and sorted before hashing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainContract {
    pub tool_name: String,
    pub tool_version: String,
    /// Sorted, deduped content refs of declared lockfile inputs.
    #[serde(default)]
    pub lockfile_refs: Vec<String>,
}

/// Digest of a declared toolchain contract: domain tag + `name@version`
/// (the op_memo derivation) + sorted, deduped lockfile content refs.
pub fn toolchain_contract_digest(contract: &ToolchainContract) -> String {
    let mut h = Sha256::new();
    h.update(b"FSZERO-TOOLCHAIN-CONTRACT-V1\0");
    h.update(contract.tool_name.as_bytes());
    h.update(&[0]);
    h.update(contract.tool_version.as_bytes());
    h.update(&[0]);
    let mut lockfile_refs = contract.lockfile_refs.clone();
    lockfile_refs.sort();
    lockfile_refs.dedup();
    for r in &lockfile_refs {
        h.update(r.as_bytes());
        h.update(&[0]);
    }
    hex_encode(h.finalize().as_slice())
}

/// Immutable snapshot: sorted path → file records. Root digest is identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExactSnapshot {
    records: BTreeMap<String, FileRecord>,
    #[serde(default)]
    toolchain_contract_digest: Option<String>,
    /// Declared, never silent: metadata classes NOT covered by this identity.
    #[serde(default = "default_nonsemantic_exclusions")]
    nonsemantic_exclusions: Vec<NonsemanticExclusion>,
    root_digest_hex: String,
}

/// Deserialize recomputes the root digest from the records and the declared
/// coverage fields; a serialized root digest is never trusted. Old snapshots
/// with `{path,digest_hex,len}`-only records deserialize via serde defaults.
impl<'de> Deserialize<'de> for ExactSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            records: BTreeMap<String, FileRecord>,
            #[serde(default)]
            toolchain_contract_digest: Option<String>,
            #[serde(default = "default_nonsemantic_exclusions")]
            nonsemantic_exclusions: Vec<NonsemanticExclusion>,
            #[serde(default)]
            #[allow(dead_code)]
            root_digest_hex: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        let root_digest_hex = snapshot_root_digest(
            &wire.records,
            wire.toolchain_contract_digest.as_deref(),
            &wire.nonsemantic_exclusions,
        );
        Ok(Self {
            records: wire.records,
            toolchain_contract_digest: wire.toolchain_contract_digest,
            nonsemantic_exclusions: wire.nonsemantic_exclusions,
            root_digest_hex,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    InvalidPath(String),
    DuplicatePath(String),
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPath(p) => write!(f, "invalid snapshot path: {p}"),
            Self::DuplicatePath(p) => write!(f, "duplicate snapshot path: {p}"),
        }
    }
}
impl std::error::Error for SnapshotError {}

/// One declared snapshot input: path + content bytes (or symlink target
/// string bytes) + mode bits + symlink marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotEntry {
    pub path: String,
    /// Content bytes for regular files. Ignored for symlink entries: the
    /// symlink's semantic content is its target string, which is digested
    /// instead (the referent is never followed).
    pub bytes: Vec<u8>,
    /// POSIX permission bits for regular files.
    pub mode: u32,
    /// Target path when this entry is a symlink; `None` for regular files.
    pub symlink_target: Option<String>,
}

/// Normalize a relative project path: reject empty, absolute, `..`, NUL.
pub fn normalize_path(path: &str) -> Result<String, SnapshotError> {
    if path.is_empty() || path.contains('\0') {
        return Err(SnapshotError::InvalidPath(path.to_string()));
    }
    let p = Path::new(path);
    if p.is_absolute() {
        return Err(SnapshotError::InvalidPath(path.to_string()));
    }
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::Normal(s) => out.push(s),
            std::path::Component::CurDir => {}
            _ => return Err(SnapshotError::InvalidPath(path.to_string())),
        }
    }
    if out.as_os_str().is_empty() {
        return Err(SnapshotError::InvalidPath(path.to_string()));
    }
    Ok(out.to_string_lossy().replace('\\', "/"))
}

fn content_digest_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex_encode(h.finalize().as_slice())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

/// Snapshot root digest: domain tag + sorted (path, len, digest, mode,
/// symlink-target) records + declared toolchain contract + declared
/// nonsemantic exclusions. V2: mode/symlink/toolchain/exclusions covered.
pub fn snapshot_root_digest(
    records: &BTreeMap<String, FileRecord>,
    toolchain_contract_digest: Option<&str>,
    nonsemantic_exclusions: &[NonsemanticExclusion],
) -> String {
    let mut h = Sha256::new();
    h.update(b"FSZERO-SNAPSHOT-V2\0");
    h.update(&(records.len() as u64).to_le_bytes());
    for (path, rec) in records {
        h.update(path.as_bytes());
        h.update(&[0]);
        h.update(&rec.len.to_le_bytes());
        h.update(rec.digest_hex.as_bytes());
        h.update(&[0]);
        h.update(&rec.mode.to_le_bytes());
        match &rec.symlink_target {
            Some(target) => {
                h.update(b"link\0");
                h.update(target.as_bytes());
            }
            None => h.update(b"file\0"),
        }
        h.update(&[0]);
    }
    match toolchain_contract_digest {
        Some(tc) => {
            h.update(b"toolchain\0");
            h.update(tc.as_bytes());
        }
        None => h.update(b"toolchain=none\0"),
    }
    h.update(&[0]);
    let mut exclusions: Vec<NonsemanticExclusion> = nonsemantic_exclusions.to_vec();
    exclusions.sort();
    exclusions.dedup();
    for e in &exclusions {
        h.update(e.tag());
    }
    hex_encode(h.finalize().as_slice())
}

impl ExactSnapshot {
    /// From in-memory path→bytes with default regular-file semantics
    /// (mode [`DEFAULT_FILE_MODE`], no symlinks, no toolchain contract).
    pub fn from_files(
        files: impl IntoIterator<Item = (String, Vec<u8>)>,
    ) -> Result<Self, SnapshotError> {
        Self::from_entries(files.into_iter().map(|(path, bytes)| SnapshotEntry {
            path,
            bytes,
            mode: DEFAULT_FILE_MODE,
            symlink_target: None,
        }))
    }

    /// From declared entries: content bytes (or symlink target string bytes),
    /// mode bits, and the symlink marker.
    pub fn from_entries(
        entries: impl IntoIterator<Item = SnapshotEntry>,
    ) -> Result<Self, SnapshotError> {
        let mut records = BTreeMap::new();
        for entry in entries {
            let path = normalize_path(&entry.path)?;
            if records.contains_key(&path) {
                return Err(SnapshotError::DuplicatePath(path));
            }
            // Symlink identity covers the target string only; the referent is
            // never followed into digesting its content as a regular file.
            // Regular-file identity covers the full content bytes.
            let (digest_hex, len) = match &entry.symlink_target {
                Some(target) => (content_digest_hex(target.as_bytes()), target.len() as u64),
                None => (content_digest_hex(&entry.bytes), entry.bytes.len() as u64),
            };
            records.insert(
                path.clone(),
                FileRecord {
                    path,
                    digest_hex,
                    len,
                    mode: entry.mode,
                    symlink_target: entry.symlink_target,
                },
            );
        }
        let nonsemantic_exclusions = default_nonsemantic_exclusions();
        let root_digest_hex = snapshot_root_digest(&records, None, &nonsemantic_exclusions);
        Ok(Self {
            records,
            toolchain_contract_digest: None,
            nonsemantic_exclusions,
            root_digest_hex,
        })
    }

    /// Declare a different nonsemantic exclusion set. Binding a declaration
    /// into identity is the point: coverage disagreements change the digest.
    pub fn with_exclusions(
        mut self,
        exclusions: Vec<NonsemanticExclusion>,
    ) -> Result<Self, SnapshotError> {
        let mut exclusions = exclusions;
        exclusions.sort();
        exclusions.dedup();
        self.nonsemantic_exclusions = exclusions;
        self.root_digest_hex = snapshot_root_digest(
            &self.records,
            self.toolchain_contract_digest.as_deref(),
            &self.nonsemantic_exclusions,
        );
        Ok(self)
    }

    /// Bind a declared toolchain/lockfile contract into snapshot coverage.
    /// The binding is part of identity: a different contract flips the root
    /// digest even for identical file bytes.
    pub fn with_toolchain_contract(mut self, contract: &ToolchainContract) -> Self {
        self.toolchain_contract_digest = Some(toolchain_contract_digest(contract));
        self.root_digest_hex = snapshot_root_digest(
            &self.records,
            self.toolchain_contract_digest.as_deref(),
            &self.nonsemantic_exclusions,
        );
        self
    }

    pub fn root_digest_hex(&self) -> &str {
        &self.root_digest_hex
    }

    pub fn get(&self, path: &str) -> Option<&FileRecord> {
        let ok = normalize_path(path).ok()?;
        self.records.get(&ok)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn records(&self) -> &BTreeMap<String, FileRecord> {
        &self.records
    }

    /// Declared toolchain contract digest bound into this snapshot, if any.
    pub fn toolchain_contract_digest(&self) -> Option<&str> {
        self.toolchain_contract_digest.as_deref()
    }

    /// Declared nonsemantic exclusions (never silent).
    pub fn nonsemantic_exclusions(&self) -> &[NonsemanticExclusion] {
        &self.nonsemantic_exclusions
    }
}
