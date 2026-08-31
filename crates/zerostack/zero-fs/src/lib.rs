#![forbid(unsafe_code)]

//! Typed FSZero implementation consumed directly by ZeroKernel. This
//! module owns bytes and filesystem effects only. It exposes no command
//! registry, CodeMode host, MCP surface, planner, or string operation dispatch.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use fszero_store::{CapsuleObjectStore, CapsuleStoreError, CasError, CasStore};
use serde::Serialize;
use zero_abi::{
    CapsulePublication, EngineError, EngineErrorKind, EngineInvocation, FileEffectKind,
    FileEffectReceipt, FileEffectRequest, FileEngine, FileLease, FileMetadata, FileReadRequest,
    FileSnapshot, LookupOptions, WorkCapsule, ZeroHandle,
};
use zero_store::{LOCK_DEADLINE, SelectionIndex, StoreLock, ZeroCas, ZeroObjectMetadata};

const INLINE_FILE_BYTE_LIMIT: usize = 8 * 1024;
const LOOKUP_ENTRY_LIMIT: usize = 10_000;
const LOOKUP_DEPTH_LIMIT: usize = 64;
const OUTLINE_LINE_LIMIT: usize = 128;
const TRANSACTION_LEASE_NAMESPACE: &str = "fszero-transaction-lease";
const READ_CACHE_ENTRY_LIMIT: usize = 64;
const READ_CACHE_FILE_BYTE_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Debug)]
pub struct ZeroFileLease {
    _lock: StoreLock,
}

impl FileLease for ZeroFileLease {}

pub struct ZeroFileEngine {
    root: PathBuf,
    lease_root: PathBuf,
    cas: ZeroCas,
    capsules: CapsuleObjectStore,
    contract_digest: String,
    read_cache: Mutex<HashMap<PathBuf, (Vec<u8>, fs::Metadata)>>,
}

impl ZeroFileEngine {
    pub fn open(
        root: impl Into<PathBuf>,
        store_root: impl Into<PathBuf>,
        contract_digest: impl Into<String>,
    ) -> Result<Self, EngineError> {
        let root = fs::canonicalize(root.into()).map_err(|error| {
            EngineError::new(
                EngineErrorKind::InvalidInput,
                format!("canonicalize FSZero root: {error}"),
                false,
            )
        })?;
        let store_root = store_root.into();
        Ok(Self {
            root,
            lease_root: store_root.join(TRANSACTION_LEASE_NAMESPACE),
            cas: ZeroCas::open(store_root.clone()),
            capsules: CapsuleObjectStore::new(CasStore::for_store_root(&store_root)),
            contract_digest: contract_digest.into(),
            read_cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn cancelled(invocation: &EngineInvocation) -> Result<(), EngineError> {
        if invocation.cancellation.is_cancelled() {
            return Err(EngineError::new(
                EngineErrorKind::Cancelled,
                "FSZero operation cancelled",
                false,
            ));
        }
        Ok(())
    }

    fn existing_path(&self, relative: &Path) -> Result<PathBuf, EngineError> {
        if relative.is_absolute() {
            if relative
                .components()
                .any(|c| matches!(c, Component::ParentDir))
            {
                return Err(EngineError::new(
                    EngineErrorKind::OutsideWorkspace,
                    "path must be workspace-relative or an absolute external path",
                    false,
                ));
            }
            let canonical = fs::canonicalize(relative).map_err(|error| {
                let kind = if error.kind() == std::io::ErrorKind::NotFound {
                    EngineErrorKind::NotFound
                } else {
                    EngineErrorKind::Io
                };
                EngineError::new(
                    kind,
                    format!("resolve {}: {error}", relative.display()),
                    false,
                )
            })?;
            return Ok(canonical);
        }
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| matches!(component, Component::RootDir | Component::Prefix(_)))
        {
            return Err(EngineError::new(
                EngineErrorKind::OutsideWorkspace,
                "path must be workspace-relative, parent-relative, or absolute",
                false,
            ));
        }
        // An explicit `..` is the relative spelling of an external path. This
        // does not widen read authority: canonical absolute external paths
        // are already accepted above. Keep implicit symlink escapes rejected.
        let explicitly_external = relative
            .components()
            .any(|component| matches!(component, Component::ParentDir));
        let path = fs::canonicalize(self.root.join(relative)).map_err(|error| {
            let kind = if error.kind() == std::io::ErrorKind::NotFound {
                EngineErrorKind::NotFound
            } else {
                EngineErrorKind::Io
            };
            EngineError::new(
                kind,
                format!("resolve {}: {error}", relative.display()),
                false,
            )
        })?;
        if !path.starts_with(&self.root) && !explicitly_external {
            return Err(EngineError::new(
                EngineErrorKind::OutsideWorkspace,
                format!(
                    "{} escapes the workspace through a symlink",
                    relative.display()
                ),
                false,
            ));
        }
        Ok(path)
    }

    fn write_path(&self, relative: &Path) -> Result<PathBuf, EngineError> {
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
        {
            return Err(EngineError::new(
                EngineErrorKind::OutsideWorkspace,
                "write path must stay within the workspace",
                false,
            ));
        }
        let target =
            fszero_store::path::validate_rollback_path(&self.root, relative).map_err(|_| {
                EngineError::new(
                    EngineErrorKind::OutsideWorkspace,
                    format!("{} escapes the workspace", relative.display()),
                    false,
                )
            })?;
        let parent = target.parent().ok_or_else(|| {
            EngineError::new(
                EngineErrorKind::InvalidInput,
                "write path has no parent",
                false,
            )
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            EngineError::new(
                EngineErrorKind::Io,
                format!("create parent dir: {error}"),
                false,
            )
        })?;
        self.guard_mutation_target(&target)?;
        Ok(target)
    }

    fn write_path_no_create(&self, relative: &Path) -> Result<PathBuf, EngineError> {
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
        {
            return Err(EngineError::new(
                EngineErrorKind::OutsideWorkspace,
                "write path must stay within the workspace",
                false,
            ));
        }
        let target =
            fszero_store::path::validate_rollback_path(&self.root, relative).map_err(|_| {
                EngineError::new(
                    EngineErrorKind::OutsideWorkspace,
                    format!("{} escapes the workspace", relative.display()),
                    false,
                )
            })?;
        let parent = target.parent().ok_or_else(|| {
            EngineError::new(
                EngineErrorKind::InvalidInput,
                "write path has no parent",
                false,
            )
        })?;
        if !parent.exists() {
            return Err(EngineError::new(
                EngineErrorKind::NotFound,
                "mutation target parent does not exist",
                false,
            ));
        }
        self.guard_mutation_target(&target)?;
        Ok(target)
    }

    fn guard_mutation_target(&self, target: &Path) -> Result<(), EngineError> {
        fszero_store::path::guard_write_target_parent(&self.root, target).map_err(|_| {
            EngineError::new(
                EngineErrorKind::OutsideWorkspace,
                format!("{} escapes the workspace", target.display()),
                false,
            )
        })
    }

    fn logical_path_for_target(&self, request_path: &Path, target: &Path) -> PathBuf {
        if request_path.is_absolute() && target.starts_with(&self.root) {
            target
                .strip_prefix(&self.root)
                .unwrap_or(target)
                .to_path_buf()
        } else if request_path.is_absolute() {
            target.to_path_buf()
        } else {
            request_path.to_path_buf()
        }
    }

    fn logical_path_for_read(&self, request_path: &Path, canonical: &Path) -> PathBuf {
        if request_path.is_absolute() && canonical.starts_with(&self.root) {
            canonical
                .strip_prefix(&self.root)
                .unwrap_or(canonical)
                .to_path_buf()
        } else if request_path.is_absolute() {
            canonical.to_path_buf()
        } else {
            request_path.to_path_buf()
        }
    }

    fn stable_read(path: &Path) -> Result<(Vec<u8>, fs::Metadata), EngineError> {
        // Metadata only: File::open on a FIFO blocks the worker.
        if let Err(detail) = fszero_store::path::refuse_non_regular_file(path) {
            return Err(EngineError::new(
                EngineErrorKind::InvalidInput,
                detail,
                false,
            ));
        }
        let mut file = fs::File::open(path).map_err(|error| {
            EngineError::new(EngineErrorKind::Io, format!("open file: {error}"), false)
        })?;
        let before = file.metadata().map_err(|error| {
            EngineError::new(
                EngineErrorKind::Io,
                format!("stat open file: {error}"),
                false,
            )
        })?;
        if !before.file_type().is_file() {
            return Err(EngineError::new(
                EngineErrorKind::InvalidInput,
                "path is not a regular file",
                false,
            ));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|error| {
            EngineError::new(EngineErrorKind::Io, format!("read file: {error}"), false)
        })?;
        let after_read = file.metadata().map_err(|error| {
            EngineError::new(
                EngineErrorKind::Io,
                format!("restat open file: {error}"),
                false,
            )
        })?;
        let after_path = fs::symlink_metadata(path).map_err(|error| {
            EngineError::new(
                EngineErrorKind::Io,
                format!("restat file path: {error}"),
                false,
            )
        })?;
        if !same_file_version(&before, &after_read) || !same_file_version(&after_read, &after_path)
        {
            return Err(EngineError::new(
                EngineErrorKind::Conflict,
                "file changed or was replaced during read",
                true,
            ));
        }
        Ok((bytes, after_read))
    }

    fn cached_read(&self, path: &Path) -> Result<(Vec<u8>, fs::Metadata), EngineError> {
        if let Ok(current) = fs::metadata(path)
            && let Ok(cache) = self.read_cache.lock()
            && let Some((bytes, cached_metadata)) = cache.get(path)
            && same_file_version(cached_metadata, &current)
        {
            return Ok((bytes.clone(), current));
        }

        let (bytes, metadata) = Self::stable_read(path)?;
        if bytes.len() <= READ_CACHE_FILE_BYTE_LIMIT
            && let Ok(mut cache) = self.read_cache.lock()
        {
            if cache.len() >= READ_CACHE_ENTRY_LIMIT && !cache.contains_key(path) {
                cache.clear();
            }
            cache.insert(path.to_path_buf(), (bytes.clone(), metadata.clone()));
        }
        Ok((bytes, metadata))
    }

    fn publish_snapshot(
        &self,
        relative: PathBuf,
        bytes: Vec<u8>,
        metadata: fs::Metadata,
        symlink_target: Option<PathBuf>,
        symlink_target_is_dir: bool,
    ) -> Result<FileSnapshot, EngineError> {
        let handle = self.cas.put(&bytes).map_err(cas_error)?;
        let text = std::str::from_utf8(&bytes).ok();
        let selection = text.map(SelectionIndex::from_utf8);
        self.cas
            .publish_metadata(&ZeroObjectMetadata {
                handle: handle.clone(),
                byte_len: bytes.len() as u64,
                media_type: if text.is_some() {
                    "text/plain; charset=utf-8".into()
                } else {
                    "application/octet-stream".into()
                },
                producer: "FSZero".into(),
                contract_digest: self.contract_digest.clone(),
                selection,
            })
            .map_err(cas_error)?;
        let inline_utf8 = text
            .filter(|_| bytes.len() <= INLINE_FILE_BYTE_LIMIT)
            .map(str::to_owned);
        let outline = text
            .filter(|_| bytes.len() > INLINE_FILE_BYTE_LIMIT)
            .map(structural_outline);
        Ok(FileSnapshot {
            path: relative,
            content: handle,
            byte_len: bytes.len() as u64,
            modified_unix_ns: modified_ns(&metadata),
            mode: file_mode(&metadata),
            symlink_target,
            symlink_target_is_dir,
            inline_utf8,
            outline,
        })
    }

    fn snapshot_existing(
        &self,
        relative: &Path,
    ) -> Result<(PathBuf, Vec<u8>, fs::Metadata, ZeroHandle), EngineError> {
        let path = self.existing_path(relative)?;
        let (bytes, metadata) = Self::stable_read(&path)?;
        let handle = self.cas.put(&bytes).map_err(cas_error)?;
        Ok((path, bytes, metadata, handle))
    }

    fn commit_file_bytes(
        &self,
        target: &Path,
        logical: &Path,
        bytes: &[u8],
    ) -> Result<ZeroHandle, EngineError> {
        self.guard_mutation_target(target)?;
        fszero_store::path::atomic_write_with_outcome(target, bytes).map_err(|error| {
            EngineError::new(
                EngineErrorKind::Io,
                format!("atomic file effect at {}: {error}", logical.display()),
                false,
            )
        })?;
        let materialized = fs::read(target).map_err(|error| {
            EngineError::new(
                EngineErrorKind::Io,
                format!("verify file effect: {error}"),
                false,
            )
        })?;
        if materialized != bytes {
            return Err(EngineError::new(
                EngineErrorKind::Corrupt,
                "file postimage differs from requested content",
                false,
            ));
        }
        self.cas.put(bytes).map_err(cas_error)
    }

    fn restore_bytes(&self, request: &FileEffectRequest) -> Result<Vec<u8>, EngineError> {
        if let Some(content) = request.content.as_deref() {
            return Ok(content.to_vec());
        }
        let handle = request.expected_preimage.as_ref().ok_or_else(|| {
            EngineError::new(
                EngineErrorKind::InvalidInput,
                "restore requires expected_preimage when content is absent",
                false,
            )
        })?;
        self.cas.get(handle).map_err(|error| match error {
            zero_store::ZeroCasError::NotFound => EngineError::new(
                EngineErrorKind::NotFound,
                "restore preimage is not present in CAS",
                false,
            ),
            other => cas_error(other),
        })
    }
}

fn unique_replace_bytes(source: &[u8], old: &[u8], new: &[u8]) -> Result<Vec<u8>, EngineError> {
    if old.is_empty() {
        return Err(EngineError::new(
            EngineErrorKind::InvalidInput,
            "patch find must be non-empty",
            false,
        ));
    }
    let mut pos = 0;
    let mut first = None;
    while let Some(rel) = source[pos..]
        .windows(old.len())
        .position(|window| window == old)
    {
        if first.is_some() {
            return Err(EngineError::new(
                EngineErrorKind::InvalidInput,
                "patch is not a unique match",
                false,
            ));
        }
        first = Some(pos + rel);
        pos += rel + old.len();
        if pos >= source.len() {
            break;
        }
    }
    let Some(start) = first else {
        return Err(EngineError::new(
            EngineErrorKind::InvalidInput,
            "patch is not a unique match",
            false,
        ));
    };
    let mut out = Vec::with_capacity(source.len() - old.len() + new.len());
    out.extend_from_slice(&source[..start]);
    out.extend_from_slice(new);
    out.extend_from_slice(&source[start + old.len()..]);
    Ok(out)
}

fn apply_unique_patch(source: &[u8], patch: &str) -> Result<Vec<u8>, EngineError> {
    let unsupported =
        || EngineError::new(EngineErrorKind::InvalidInput, "patch not supported", false);
    let value: serde_json::Value = serde_json::from_str(patch).map_err(|_| unsupported())?;
    let obj = value.as_object().ok_or_else(unsupported)?;

    let (find, replacement) = match obj.get("kind") {
        None => {
            if obj.len() != 2 || !obj.contains_key("find") || !obj.contains_key("replacement") {
                return Err(unsupported());
            }
            let find = obj
                .get("find")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(unsupported)?;
            let replacement = obj
                .get("replacement")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(unsupported)?;
            (find, replacement)
        }
        Some(serde_json::Value::String(kind)) if kind == "replace_exact" => {
            if obj.len() != 4
                || !obj.contains_key("old")
                || !obj.contains_key("replacement")
                || obj.get("expectedCount").and_then(serde_json::Value::as_u64) != Some(1)
            {
                return Err(unsupported());
            }
            let find = obj
                .get("old")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(unsupported)?;
            let replacement = obj
                .get("replacement")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(unsupported)?;
            (find, replacement)
        }
        Some(_) => return Err(unsupported()),
    };

    unique_replace_bytes(source, find.as_bytes(), replacement.as_bytes())
}

fn write_or_edit_bytes(
    request: &FileEffectRequest,
    current: Option<&[u8]>,
) -> Result<Vec<u8>, EngineError> {
    if let Some(patch) = request.patch.as_deref() {
        let patch_value: serde_json::Value = serde_json::from_str(patch).map_err(|_| {
            EngineError::new(EngineErrorKind::InvalidInput, "patch not supported", false)
        })?;
        let patch_kind = patch_value
            .as_object()
            .and_then(|object| object.get("kind"))
            .and_then(serde_json::Value::as_str);
        if matches!(
            patch_kind,
            Some("replace_lines" | "insert_before" | "insert_after" | "replace_file")
        ) {
            return request
                .content
                .as_deref()
                .map(<[u8]>::to_vec)
                .ok_or_else(|| {
                    EngineError::new(
                        EngineErrorKind::InvalidInput,
                        "typed edit patch requires the host-computed postimage",
                        false,
                    )
                });
        }
        let current = current.ok_or_else(|| {
            EngineError::new(
                EngineErrorKind::InvalidInput,
                "patch requires existing file bytes",
                false,
            )
        })?;
        let patched = apply_unique_patch(current, patch)?;
        if let Some(content) = request.content.as_deref()
            && content != patched.as_slice()
        {
            return Err(EngineError::new(
                EngineErrorKind::InvalidInput,
                "patch postimage does not match requested content",
                false,
            ));
        }
        return Ok(patched);
    }
    request
        .content
        .as_deref()
        .map(<[u8]>::to_vec)
        .ok_or_else(|| {
            EngineError::new(
                EngineErrorKind::InvalidInput,
                "write/edit requires content",
                false,
            )
        })
}

impl FileEngine for ZeroFileEngine {
    fn lease(&self, invocation: &EngineInvocation) -> Result<Box<dyn FileLease>, EngineError> {
        Self::cancelled(invocation)?;
        // Keep transaction serialization separate from the CAS coordinator:
        // effects publish receipts while this exclusive lease is held.
        let lock = StoreLock::sweep(&self.lease_root, LOCK_DEADLINE).map_err(|error| {
            EngineError::new(
                EngineErrorKind::Conflict,
                format!("acquire FSZero transaction lease: {error}"),
                true,
            )
        })?;
        Ok(Box::new(ZeroFileLease { _lock: lock }))
    }

    fn read(
        &self,
        invocation: &EngineInvocation,
        request: FileReadRequest,
    ) -> Result<FileSnapshot, EngineError> {
        Self::cancelled(invocation)?;
        let entry_path = if request.path.is_absolute() {
            request.path.clone()
        } else {
            self.root.join(&request.path)
        };
        let canonical = self.existing_path(&request.path)?;
        let (mut bytes, metadata) = self.cached_read(&canonical)?;
        let symlink_target = match fs::symlink_metadata(&entry_path) {
            Ok(entry) if entry.file_type().is_symlink() => {
                Some(fs::read_link(&entry_path).map_err(|error| {
                    EngineError::new(
                        EngineErrorKind::Io,
                        format!("read symlink {}: {error}", request.path.display()),
                        false,
                    )
                })?)
            }
            Ok(_) => None,
            Err(error) => {
                return Err(EngineError::new(
                    EngineErrorKind::Io,
                    format!("inspect {}: {error}", request.path.display()),
                    false,
                ));
            }
        };
        let symlink_target_is_dir = symlink_target.is_some() && metadata.is_dir();
        if let Some(range) = request.options.range.as_deref() {
            bytes = line_range(&bytes, range)?;
        }
        if let Some(limit) = request.options.max_bytes {
            let limit = usize::try_from(limit).unwrap_or(usize::MAX);
            bytes.truncate(limit);
        }
        Self::cancelled(invocation)?;
        let logical = self.logical_path_for_read(&request.path, &canonical);
        self.publish_snapshot(
            logical,
            bytes,
            metadata,
            symlink_target,
            symlink_target_is_dir,
        )
    }

    fn lookup(
        &self,
        invocation: &EngineInvocation,
        root: PathBuf,
        options: LookupOptions,
    ) -> Result<Vec<PathBuf>, EngineError> {
        // Do not descend into VCS or store caches. Their directory entries still count
        // toward the parent's visit budget.
        const IGNORED_LOOKUP_DIRS: &[&str] = &[
            ".git",
            ".zerostack",
            ".fszero",
            ".graphzero",
            ".tokenzero",
            ".asgrep",
            ".beads",
            "node_modules",
            "target",
        ];
        Self::cancelled(invocation)?;
        let base = if root.as_os_str().is_empty() || root == Path::new(".") {
            self.root.clone()
        } else {
            self.existing_path(&root)?
        };
        if !base.is_dir() {
            return Err(EngineError::new(
                EngineErrorKind::InvalidInput,
                "lookup root is not a directory",
                false,
            ));
        }
        // Use the public maximum internally because this return type has no
        // truncation flag; an apparently complete prefix must not hide entries.
        let limit = options.limit.unwrap_or(1_000).clamp(1, 100_000) as usize;
        let filter = options.filter.as_deref().unwrap_or("");
        let mut pending = vec![(base, 0_usize)];
        let mut visited = 0_usize;
        let mut output = Vec::new();
        let mut incomplete = false;
        while let Some((directory, depth)) = pending.pop() {
            Self::cancelled(invocation)?;
            if depth > LOOKUP_DEPTH_LIMIT || visited >= LOOKUP_ENTRY_LIMIT {
                incomplete = true;
                break;
            }
            let entries = fs::read_dir(&directory).map_err(|error| {
                EngineError::new(
                    EngineErrorKind::Io,
                    format!("read lookup directory: {error}"),
                    false,
                )
            })?;
            let mut children = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|error| {
                    EngineError::new(
                        EngineErrorKind::Io,
                        format!("read lookup entry: {error}"),
                        false,
                    )
                })?;
                visited = visited.saturating_add(1);
                if visited > LOOKUP_ENTRY_LIMIT {
                    incomplete = true;
                    break;
                }
                let file_type = entry.file_type().map_err(|error| {
                    EngineError::new(
                        EngineErrorKind::Io,
                        format!("stat lookup entry: {error}"),
                        false,
                    )
                })?;
                if file_type.is_symlink() {
                    continue;
                }
                let path = entry.path();
                if file_type.is_dir()
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| IGNORED_LOOKUP_DIRS.contains(&name))
                {
                    // Tool-internal directories (.git,.asgrep, target,...)
                    // are store internals: neither descended nor listed.
                    continue;
                }
                let relative = path.strip_prefix(&self.root).unwrap_or(&path).to_path_buf();
                if relative.to_string_lossy().contains(filter) {
                    if output.len() >= limit {
                        incomplete = true;
                        break;
                    }
                    output.push(relative);
                }
                if options.recursive && file_type.is_dir() && depth < LOOKUP_DEPTH_LIMIT {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if IGNORED_LOOKUP_DIRS.contains(&name) {
                            continue;
                        }
                    }
                    children.push(path);
                }
            }
            if incomplete {
                break;
            }
            children.sort();
            for child in children.into_iter().rev() {
                pending.push((child, depth + 1));
            }
        }
        if incomplete {
            return Err(EngineError::new(
                EngineErrorKind::Budget,
                format!(
                    "lookup budget exceeded: visited={visited} entry_cap={LOOKUP_ENTRY_LIMIT} matched={}/{} depth_cap={LOOKUP_DEPTH_LIMIT}",
                    output.len(),
                    limit
                ),
                false,
            ));
        }
        output.sort();
        Ok(output)
    }

    fn apply(
        &self,
        invocation: &EngineInvocation,
        request: FileEffectRequest,
    ) -> Result<FileEffectReceipt, EngineError> {
        Self::cancelled(invocation)?;
        let target = if request.kind == FileEffectKind::Remove {
            self.write_path_no_create(&request.path)?
        } else {
            self.write_path(&request.path)?
        };
        let logical = self.logical_path_for_target(&request.path, &target);
        // Refuse FIFO/socket/device/dir before snapshot_existing: that path
        // opens the target, and File::open on a FIFO blocks.
        if let Err(detail) = fszero_store::path::refuse_non_regular_file(&target) {
            return Err(EngineError::new(
                EngineErrorKind::InvalidInput,
                detail,
                false,
            ));
        }
        let entry_metadata = match fs::symlink_metadata(&target) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(EngineError::new(
                    EngineErrorKind::Io,
                    format!("inspect mutation target: {error}"),
                    false,
                ));
            }
        };
        let symlink_target = match entry_metadata.as_ref() {
            Some(metadata) if metadata.file_type().is_symlink() => {
                Some(fs::read_link(&target).map_err(|error| {
                    EngineError::new(
                        EngineErrorKind::Io,
                        format!("read mutation symlink: {error}"),
                        false,
                    )
                })?)
            }
            _ => None,
        };
        let dangling_symlink = symlink_target.is_some() && !target.exists();
        if dangling_symlink && request.kind == FileEffectKind::Edit {
            return Err(EngineError::new(
                EngineErrorKind::NotFound,
                "cannot edit through a dangling symlink",
                false,
            ));
        }
        let before = if dangling_symlink {
            let bytes = serde_json::to_vec(symlink_target.as_ref().expect("checked above"))
                .map_err(|error| {
                    EngineError::new(EngineErrorKind::Internal, error.to_string(), false)
                })?;
            let handle = self.cas.put(&bytes).map_err(cas_error)?;
            Some((
                target.clone(),
                Vec::new(),
                entry_metadata.as_ref().expect("checked above").clone(),
                handle,
            ))
        } else if entry_metadata.is_some() {
            Some(self.snapshot_existing(&request.path)?)
        } else {
            None
        };
        if request.expect_absent && before.is_some() {
            return Err(EngineError::new(
                EngineErrorKind::Conflict,
                "target exists but absence was required",
                false,
            ));
        }
        // Restore replays CAS bytes from expected_preimage (or explicit
        // content). It is not a current-file compare-and-swap like Write/Edit.
        if request.kind != FileEffectKind::Restore {
            if let Some(expected) = request.expected_preimage.as_ref() {
                if before.as_ref().map(|snapshot| &snapshot.3) != Some(expected) {
                    return Err(EngineError::new(
                        EngineErrorKind::Conflict,
                        "stale preimage: does not match expected handle",
                        false,
                    ));
                }
            }
        }
        if request.patch.is_some() && request.kind != FileEffectKind::Edit {
            return Err(EngineError::new(
                EngineErrorKind::InvalidInput,
                "patch not supported",
                false,
            ));
        }
        let before_handle = before.as_ref().map(|snapshot| snapshot.3.clone());
        let before_metadata = before.as_ref().map(|snapshot| {
            let entry = entry_metadata.as_ref().unwrap_or(&snapshot.2);
            FileMetadata {
                mode: file_mode(entry),
                modified_unix_ns: modified_ns(entry),
                symlink_target: symlink_target.clone(),
                symlink_target_is_dir: symlink_target.is_some()
                    && symlink_targets_directory(entry, Some(&snapshot.2)),
            }
        });
        let after_handle = match request.kind {
            FileEffectKind::Write | FileEffectKind::Edit => {
                let bytes = write_or_edit_bytes(
                    &request,
                    before.as_ref().map(|snapshot| snapshot.1.as_slice()),
                )?;
                Some(self.commit_file_bytes(&target, &logical, &bytes)?)
            }
            FileEffectKind::Restore => {
                let bytes = self.restore_bytes(&request)?;
                Self::cancelled(invocation)?;
                Some(self.commit_file_bytes(&target, &logical, &bytes)?)
            }
            FileEffectKind::Remove => {
                if before.is_none() {
                    return Err(EngineError::new(
                        EngineErrorKind::NotFound,
                        "remove target does not exist",
                        false,
                    ));
                }
                self.guard_mutation_target(&target)?;
                fs::remove_file(&target).map_err(|error| {
                    EngineError::new(EngineErrorKind::Io, format!("remove file: {error}"), false)
                })?;
                None
            }
        };
        // A same-length rewrite can retain metadata granularity on fast
        // filesystems. Explicit invalidation guarantees read-your-writes
        // instead of trusting size/mtime equality.
        if let Ok(mut cache) = self.read_cache.lock() {
            cache.remove(&target);
        }
        // Durable bytes are already published (commit_file_bytes / remove).
        // A late cancel must not rewrite that outcome as Cancelled.
        #[derive(Serialize)]
        struct Journal<'a> {
            kind: &'a FileEffectKind,
            path: &'a Path,
            before: &'a Option<ZeroHandle>,
            after: &'a Option<ZeroHandle>,
            trace_id: &'a str,
        }
        let journal_bytes = serde_json::to_vec(&Journal {
            kind: &request.kind,
            path: &logical,
            before: &before_handle,
            after: &after_handle,
            trace_id: &invocation.context.trace_id,
        })
        .map_err(|error| EngineError::new(EngineErrorKind::Internal, error.to_string(), false))?;
        let journal = self.cas.put(&journal_bytes).map_err(cas_error)?;
        Ok(FileEffectReceipt {
            kind: request.kind,
            path: logical,
            before: before_handle,
            after: after_handle,
            before_metadata,
            journal,
        })
    }

    fn restore(
        &self,
        _invocation: &EngineInvocation,
        receipt: &FileEffectReceipt,
    ) -> Result<(), EngineError> {
        // Rollback is mandatory cleanup. A cancelled guest cannot cancel restoration.
        match receipt.before.as_ref() {
            Some(handle) => {
                // An overwritten or removed pre-existing file is not restored
                // when its parent disappears. Fail closed so ZeroKernel retains
                // RecoveryRequired instead of certifying a missing preimage.
                let raw_target = if receipt.path.is_absolute() {
                    receipt.path.clone()
                } else {
                    self.root.join(&receipt.path)
                };
                if let Some(parent) = raw_target.parent()
                    && !parent.exists()
                {
                    return Err(EngineError::new(
                        EngineErrorKind::Io,
                        format!(
                            "rollback parent for {} no longer exists",
                            receipt.path.display()
                        ),
                        false,
                    ));
                }
                let target = self.write_path(&receipt.path)?;
                if let Some(metadata) = &receipt.before_metadata
                    && let Some(link_target) = metadata.symlink_target.as_ref()
                {
                    restore_symlink_entry(&target, link_target, metadata.symlink_target_is_dir)?;
                } else {
                    let bytes = self.cas.get(handle).map_err(cas_error)?;
                    fszero_store::path::atomic_write_with_outcome(&target, &bytes).map_err(
                        |error| {
                            EngineError::new(
                                EngineErrorKind::Io,
                                format!("restore {}: {error}", receipt.path.display()),
                                false,
                            )
                        },
                    )?;
                    if let Some(metadata) = &receipt.before_metadata {
                        restore_mode(&target, metadata.mode)?;
                        restore_modified_time(&target, metadata.modified_unix_ns)?;
                    }
                }
            }
            None => {
                // Deleting a file that was created. Missing file or missing parent
                // means already restored; this prevents a durable journal replay
                // from wedging the session forever.
                let raw_target = if receipt.path.is_absolute() {
                    receipt.path.clone()
                } else {
                    self.root.join(&receipt.path)
                };
                if let Some(parent) = raw_target.parent() {
                    if !parent.exists() {
                        return Ok(());
                    }
                }
                let target = match self.write_path_no_create(&receipt.path) {
                    Ok(path) => path,
                    Err(error) if error.kind == EngineErrorKind::NotFound => return Ok(()),
                    Err(error) => return Err(error),
                };
                self.guard_mutation_target(&target)?;
                match fs::remove_file(&target) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(EngineError::new(
                            EngineErrorKind::Io,
                            format!("remove created file during restore: {error}"),
                            false,
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn reconcile(&self, invocation: &EngineInvocation) -> Result<Vec<ZeroHandle>, EngineError> {
        Self::cancelled(invocation)?;
        // Individual FSZero effects are atomic and their immutable receipt is
        // published only after postimage verification. Multi-effect recovery
        // is coordinated by ZeroKernel's durable transaction journal.
        Ok(Vec::new())
    }

    fn put_capsule(
        &self,
        invocation: &EngineInvocation,
        capsule: &WorkCapsule,
    ) -> Result<CapsulePublication, EngineError> {
        Self::cancelled(invocation)?;
        capsule.validate().map_err(|detail| {
            EngineError::new(
                EngineErrorKind::InvalidInput,
                format!("invalid work capsule: {detail}"),
                false,
            )
        })?;
        let receipt = self.capsules.put(capsule).map_err(capsule_store_error)?;
        let object = ZeroHandle::from_digest(&receipt.object_hash).map_err(|error| {
            EngineError::new(
                EngineErrorKind::Internal,
                format!("capsule object digest is not a valid handle: {error}"),
                false,
            )
        })?;
        Ok(CapsulePublication {
            capsule_root: receipt.capsule_root,
            object,
            created: receipt.created,
        })
    }

    fn get_capsule(
        &self,
        invocation: &EngineInvocation,
        publication: &CapsulePublication,
    ) -> Result<WorkCapsule, EngineError> {
        Self::cancelled(invocation)?;
        publication.validate().map_err(|detail| {
            EngineError::new(
                EngineErrorKind::InvalidInput,
                format!("invalid capsule publication: {detail}"),
                false,
            )
        })?;
        let capsule = self
            .capsules
            .get_expected(publication.object.digest(), &publication.capsule_root)
            .map_err(capsule_store_error)?;
        Ok(capsule)
    }
}

fn line_range(bytes: &[u8], range: &str) -> Result<Vec<u8>, EngineError> {
    let (start, end) = range.split_once(':').ok_or_else(|| {
        EngineError::new(
            EngineErrorKind::InvalidInput,
            "read range must be START:END with inclusive positive line numbers",
            false,
        )
    })?;
    let start = start.parse::<usize>().ok().filter(|line| *line > 0);
    let end = end.parse::<usize>().ok().filter(|line| *line > 0);
    let (Some(start), Some(end)) = (start, end) else {
        return Err(EngineError::new(
            EngineErrorKind::InvalidInput,
            "read range must be START:END with inclusive positive line numbers",
            false,
        ));
    };
    if end < start {
        return Err(EngineError::new(
            EngineErrorKind::InvalidInput,
            "read range end must be greater than or equal to start",
            false,
        ));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| {
        EngineError::new(
            EngineErrorKind::InvalidInput,
            "line ranges require a UTF-8 file",
            false,
        )
    })?;
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    if start > lines.len() {
        return Err(EngineError::new(
            EngineErrorKind::InvalidInput,
            format!(
                "read range start {start} exceeds file line count {}",
                lines.len()
            ),
            false,
        ));
    }
    Ok(lines[start - 1..end.min(lines.len())].concat().into_bytes())
}

fn cas_error(error: zero_store::ZeroCasError) -> EngineError {
    EngineError::new(EngineErrorKind::Corrupt, error.to_string(), false)
}

fn capsule_store_error(error: CapsuleStoreError) -> EngineError {
    let (kind, detail) = match &error {
        CapsuleStoreError::Cas(cas) => match cas {
            CasError::Malformed(hash) => (
                EngineErrorKind::InvalidInput,
                format!("capsule object hash malformed: {hash}"),
            ),
            CasError::Missing(hash) => (
                EngineErrorKind::NotFound,
                format!("capsule object not present in CAS: {hash}"),
            ),
            CasError::Io { context, .. } => (
                EngineErrorKind::Io,
                format!("capsule storage io failure: {context}"),
            ),
            CasError::Corrupt { hash, detail } => (
                EngineErrorKind::Corrupt,
                format!("capsule object corrupt sha256/{hash}: {detail}"),
            ),
            other => (
                EngineErrorKind::Io,
                format!("capsule storage failure: {other}"),
            ),
        },
        CapsuleStoreError::Envelope(detail) => (
            EngineErrorKind::Corrupt,
            format!("capsule envelope invalid: {detail}"),
        ),
        CapsuleStoreError::Manifest(detail) => (
            EngineErrorKind::Corrupt,
            format!("capsule manifest invalid: {detail}"),
        ),
        CapsuleStoreError::RootMismatch { expected, actual } => (
            EngineErrorKind::Corrupt,
            format!("capsule envelope root mismatch: declared {expected}, manifest {actual}"),
        ),
        CapsuleStoreError::ExactRootMismatch { expected, actual } => (
            EngineErrorKind::Corrupt,
            format!("capsule root mismatch: expected {expected}, stored {actual}"),
        ),
    };
    EngineError::new(kind, detail, false)
}
#[cfg(windows)]
fn symlink_targets_directory(entry: &fs::Metadata, referent: Option<&fs::Metadata>) -> bool {
    use std::os::windows::fs::FileTypeExt;
    entry.file_type().is_symlink_dir() || referent.is_some_and(fs::Metadata::is_dir)
}

#[cfg(not(windows))]
fn symlink_targets_directory(_entry: &fs::Metadata, referent: Option<&fs::Metadata>) -> bool {
    referent.is_some_and(fs::Metadata::is_dir)
}

fn remove_for_symlink_restore(path: &Path) -> Result<(), EngineError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(EngineError::new(
            EngineErrorKind::Io,
            format!("remove postimage before symlink restore: {error}"),
            false,
        )),
    }
}

#[cfg(unix)]
fn restore_symlink_entry(
    path: &Path,
    link_target: &Path,
    _target_is_dir: bool,
) -> Result<(), EngineError> {
    remove_for_symlink_restore(path)?;
    std::os::unix::fs::symlink(link_target, path).map_err(|error| {
        EngineError::new(
            EngineErrorKind::Io,
            format!("restore symlink entry: {error}"),
            false,
        )
    })
}

#[cfg(windows)]
fn restore_symlink_entry(
    path: &Path,
    link_target: &Path,
    target_is_dir: bool,
) -> Result<(), EngineError> {
    remove_for_symlink_restore(path)?;
    let result = if target_is_dir {
        std::os::windows::fs::symlink_dir(link_target, path)
    } else {
        std::os::windows::fs::symlink_file(link_target, path)
    };
    result.map_err(|error| {
        EngineError::new(
            EngineErrorKind::Io,
            format!("restore symlink entry: {error}"),
            false,
        )
    })
}

#[cfg(not(any(unix, windows)))]
fn restore_symlink_entry(
    _path: &Path,
    _link_target: &Path,
    _target_is_dir: bool,
) -> Result<(), EngineError> {
    Err(EngineError::new(
        EngineErrorKind::Unsupported,
        "symlink rollback is unsupported on this platform",
        false,
    ))
}

fn same_file_version(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    if left.len() != right.len() || modified_ns(left) != modified_ns(right) {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev()
            && left.ino() == right.ino()
            && left.ctime() == right.ctime()
            && left.ctime_nsec() == right.ctime_nsec()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        left.volume_serial_number() == right.volume_serial_number()
            && left.file_index() == right.file_index()
            && left.creation_time() == right.creation_time()
    }
    #[cfg(not(any(unix, windows)))]
    {
        true
    }
}

fn modified_ns(metadata: &fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode()
}

#[cfg(not(unix))]
fn file_mode(_metadata: &fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn restore_mode(path: &Path, mode: u32) -> Result<(), EngineError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|error| {
        EngineError::new(
            EngineErrorKind::Io,
            format!("restore file mode: {error}"),
            false,
        )
    })
}

#[cfg(not(unix))]
fn restore_mode(_path: &Path, _mode: u32) -> Result<(), EngineError> {
    Ok(())
}

fn restore_modified_time(path: &Path, modified_unix_ns: u128) -> Result<(), EngineError> {
    let nanos = u64::try_from(modified_unix_ns).map_err(|_| {
        EngineError::new(
            EngineErrorKind::InvalidInput,
            "restore modified time exceeds the supported range",
            false,
        )
    })?;
    let modified = UNIX_EPOCH
        .checked_add(std::time::Duration::from_nanos(nanos))
        .ok_or_else(|| {
            EngineError::new(
                EngineErrorKind::InvalidInput,
                "restore modified time exceeds the supported range",
                false,
            )
        })?;
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(modified))
        .map_err(|error| {
            EngineError::new(
                EngineErrorKind::Io,
                format!("restore file modified time: {error}"),
                false,
            )
        })
}

fn structural_outline(text: &str) -> String {
    let mut output = String::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if [
            "fn ",
            "pub fn ",
            "struct ",
            "pub struct ",
            "enum ",
            "pub enum ",
            "trait ",
            "impl ",
            "class ",
            "interface ",
            "function ",
            "def ",
        ]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
        {
            output.push_str(&format!("L{}: {}\n", index + 1, trimmed));
            if output.lines().count() >= OUTLINE_LINE_LIMIT {
                break;
            }
        }
    }
    if output.is_empty() {
        let total_lines = text.lines().count();
        output = format!("text file: {total_lines} lines\n");
    }
    output
}

#[allow(dead_code)]
fn _system_time_to_ns(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}
