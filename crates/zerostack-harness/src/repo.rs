//! Repo-root discovery for structural spec checks.

use std::path::{Path, PathBuf};

/// Hub checkout root. `ZEROSTACK_ROOT` wins; otherwise two parents above this
/// crate's manifest (`crates/zerostack-harness`).
pub fn repo_root() -> PathBuf {
    if let Ok(value) = std::env::var("ZEROSTACK_ROOT") {
        let path = PathBuf::from(value);
        if path.is_dir() {
            return path;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/zerostack-harness lives two levels below the hub root")
        .to_path_buf()
}

pub fn read_text(root: &Path, rel: &str) -> Result<String, String> {
    let path = root.join(rel);
    std::fs::read_to_string(&path).map_err(|error| format!("read {}: {error}", path.display()))
}

pub fn read_bytes(root: &Path, rel: &str) -> Result<Vec<u8>, String> {
    let path = root.join(rel);
    std::fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    digest
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            use std::fmt::Write;
            let _ = write!(out, "{byte:02x}");
            out
        })
}

pub fn file_sha256_hex(root: &Path, rel: &str) -> Result<String, String> {
    Ok(sha256_hex(&read_bytes(root, rel)?))
}
