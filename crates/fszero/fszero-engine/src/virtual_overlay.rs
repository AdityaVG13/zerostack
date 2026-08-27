//! Virtual overlay reads: journal + base without touching disk (fszero-o9hs).
//!
//! When a world is active, list/read resolve against the in-memory materialization
//! of base bytes plus overlay mutations — no write-through to the workspace.

use std::collections::BTreeMap;
use std::path::{Component, Path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtualOverlayError {
    InvalidPath(String),
    Missing(String),
    IsDirectory(String),
}

impl std::fmt::Display for VirtualOverlayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPath(p) => write!(f, "invalid path: {p}"),
            Self::Missing(p) => write!(f, "missing: {p}"),
            Self::IsDirectory(p) => write!(f, "is directory: {p}"),
        }
    }
}
impl std::error::Error for VirtualOverlayError {}

fn normalize(path: &str) -> Result<String, VirtualOverlayError> {
    if path.is_empty() || path.contains('\0') {
        return Err(VirtualOverlayError::InvalidPath(path.to_string()));
    }
    let p = Path::new(path);
    if p.is_absolute() {
        return Err(VirtualOverlayError::InvalidPath(path.to_string()));
    }
    let mut out = Vec::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => out.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return Err(VirtualOverlayError::InvalidPath(path.to_string())),
        }
    }
    if out.is_empty() {
        return Ok(".".into());
    }
    Ok(out.join("/"))
}

/// In-memory world view: base files + put/delete deltas.
#[derive(Debug, Clone, Default)]
pub struct VirtualOverlay {
    /// path -> file bytes (materialized view)
    files: BTreeMap<String, Vec<u8>>,
}

impl VirtualOverlay {
    pub fn from_base(base: BTreeMap<String, Vec<u8>>) -> Self {
        Self { files: base }
    }

    pub fn put(&mut self, path: &str, bytes: Vec<u8>) -> Result<(), VirtualOverlayError> {
        let path = normalize(path)?;
        if path == "." {
            return Err(VirtualOverlayError::InvalidPath(path));
        }
        self.files.insert(path, bytes);
        Ok(())
    }

    pub fn delete(&mut self, path: &str) -> Result<(), VirtualOverlayError> {
        let path = normalize(path)?;
        self.files.remove(&path);
        Ok(())
    }

    /// Read file bytes from the virtual view (no disk).
    pub fn read(&self, path: &str) -> Result<Vec<u8>, VirtualOverlayError> {
        let path = normalize(path)?;
        if path == "." {
            return Err(VirtualOverlayError::IsDirectory(path));
        }
        // If any child exists under path/, treat as directory.
        let prefix = format!("{path}/");
        if self.files.keys().any(|k| k.starts_with(&prefix)) && !self.files.contains_key(&path) {
            return Err(VirtualOverlayError::IsDirectory(path));
        }
        self.files
            .get(&path)
            .cloned()
            .ok_or_else(|| VirtualOverlayError::Missing(path))
    }

    /// List immediate children under path (virtual dirs inferred from file paths).
    pub fn list(&self, path: &str) -> Result<Vec<String>, VirtualOverlayError> {
        let path = normalize(path)?;
        let prefix = if path == "." {
            String::new()
        } else {
            format!("{path}/")
        };
        let mut names = std::collections::BTreeSet::new();
        for key in self.files.keys() {
            let rest = if prefix.is_empty() {
                key.as_str()
            } else if let Some(r) = key.strip_prefix(&prefix) {
                r
            } else {
                continue;
            };
            if rest.is_empty() {
                continue;
            }
            let name = rest.split('/').next().unwrap_or(rest);
            names.insert(name.to_string());
        }
        Ok(names.into_iter().collect())
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}
