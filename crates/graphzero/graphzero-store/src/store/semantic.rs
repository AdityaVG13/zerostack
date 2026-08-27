//! P5.1 semantic sidecar persistence (mmap read path).

use std::path::{Path, PathBuf};

/// Default filename for semantic vectors beside a GZSH shard.
pub fn semantic_sidecar_path(shard_path: &Path) -> PathBuf {
    let name = shard_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("shard.bin");
    shard_path.with_file_name(format!("{name}.semantic"))
}

/// True when a semantic sidecar exists for the shard.
pub fn semantic_sidecar_exists(shard_path: &Path) -> bool {
    semantic_sidecar_path(shard_path).is_file()
}

/// Load semantic tier coverage percent from sidecar presence (walking skeleton).
pub fn semantic_tier_percent_for_shards(shard_paths: &[PathBuf]) -> f64 {
    if shard_paths.is_empty() {
        return 0.0;
    }
    let with = shard_paths
        .iter()
        .filter(|p| semantic_sidecar_exists(p))
        .count();
    (with as f64 / shard_paths.len() as f64) * 100.0
}

/// Sidecar path when the semantic file exists.
pub fn semantic_sidecar_path_if_exists(shard_path: &Path) -> Option<PathBuf> {
    let sidecar = semantic_sidecar_path(shard_path);
    sidecar.is_file().then_some(sidecar)
}
