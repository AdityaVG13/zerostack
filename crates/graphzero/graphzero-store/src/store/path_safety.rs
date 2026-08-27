//! Path-component validation for store filesystem lookups.
//!
//! Rejects traversal sequences and separator characters before any user-controlled
//! string is joined into `.graphzero/` paths.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

pub const MAX_SAFE_ID_LEN: usize = 256;
pub const MAX_MANIFEST_SNAPSHOT_COUNT: usize = 10_000;
pub const MAX_MANIFEST_VEC_COUNT: usize = 100_000;
pub const MAX_DELTA_SEGMENT_ENTRIES: u32 = 1_000_000;

/// Safe opaque id for `gz://query/`, `gz://snap/`, `gz://node/`, `gz://edge/` tails.
pub fn validate_safe_id(id: &str, context: &str) -> Result<()> {
    if id.is_empty() {
        bail!("{context}: empty id");
    }
    if id.len() > MAX_SAFE_ID_LEN {
        bail!("{context}: id too long");
    }
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        bail!("{context}: id contains path traversal");
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'@')
    {
        bail!("{context}: id contains invalid characters");
    }
    Ok(())
}

pub(crate) fn absolute_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    })
}

/// Convert a filesystem file name to UTF-8, rejecting invalid names instead of
/// replacing bytes with U+FFFD and looking up or deleting the wrong store file.
pub fn file_name_to_str<'a>(name: &'a std::ffi::OsStr, context: &str) -> Result<&'a str> {
    let Some(text) = name.to_str() else {
        bail!("{context}: non-UTF-8 file name rejected");
    };
    Ok(text)
}

/// Blob hash prefix or full digest used under `.graphzero/blobs/`.
pub fn validate_blob_hash_component(hash_hex: &str, context: &str) -> Result<()> {
    if hash_hex.is_empty() || hash_hex.len() > 64 {
        bail!("{context}: invalid blob hash length");
    }
    if !hash_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("{context}: blob hash must be hex");
    }
    Ok(())
}

/// Pack manifest path components (`pack_id`, `version`, shard `file_name`).
pub fn validate_pack_path_component(value: &str, field: &str) -> Result<()> {
    if value.is_empty() || value.len() > MAX_SAFE_ID_LEN {
        bail!("pack {field}: invalid length");
    }
    if value.contains('/') || value.contains('\\') || value.contains("..") {
        bail!("pack {field}: path traversal rejected");
    }
    if !value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
    {
        bail!("pack {field}: invalid characters");
    }
    Ok(())
}

/// Read a file under `store_root/queries/` after canonical path containment checks.
pub fn read_queries_file(store_root: &std::path::Path, file_name: &str) -> Result<Vec<u8>> {
    validate_safe_id(
        file_name.strip_suffix(".json").unwrap_or(file_name),
        "query file",
    )?;
    let queries_dir = store_root.join("queries");
    std::fs::create_dir_all(&queries_dir)?;
    let path = queries_dir.join(file_name);
    let canonical_dir = queries_dir.canonicalize()?;
    let canonical_path = match path.canonicalize() {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(e.into()),
        Err(e) => anyhow::bail!("query path canonicalize: {e}"),
    };
    if !canonical_path.starts_with(&canonical_dir) {
        anyhow::bail!("query path escapes store queries directory");
    }
    Ok(std::fs::read(canonical_path)?)
}

#[cfg(test)]
#[path = "../../../../../tests/graphzero/unit/graphzero-store/path_safety_tests.rs"]
mod tests;
