//! P5.4 semantic index packs: GZSV sidecars bundled with dependency shard packs.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use graphzero_semantic::{
    DeterministicEmbedder, SemanticIndex, SemanticRecord, SemanticShardReader, SemanticShardWriter,
};
use graphzero_store::ContentHash;
use graphzero_store::store::indexer;
use graphzero_store::store::semantic::semantic_sidecar_path;
use sha2::{Digest, Sha256};

use crate::manifest::{PackManifest, PackSemanticSidecarEntry};

/// Build deterministic GZSV sidecars for each gzsh shard in `pack_root/shards`.
pub fn build_semantic_sidecars_for_pack(pack_root: &Path) -> Result<Vec<PackSemanticSidecarEntry>> {
    let shards_dir = pack_root.join("shards");
    if !shards_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&shards_dir)? {
        let shard_path = entry?.path();
        if shard_path.extension().is_none_or(|e| e != "gzsh") {
            continue;
        }
        let records = semantic_records_for_shard_blob_dir(pack_root, &shard_path)?;
        let sidecar_path = semantic_sidecar_path(&shard_path);
        SemanticShardWriter::write(&sidecar_path, &records)?;
        let digest = sha256_file(&sidecar_path)?;
        let record_count = records.len() as u32;
        let file_name = sidecar_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("sidecar.semantic")
            .to_string();
        out.push(PackSemanticSidecarEntry {
            gzsh_file_name: shard_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("shard.gzsh")
                .to_string(),
            file_name,
            content_sha256: digest,
            record_count,
        });
    }
    out.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(out)
}

fn semantic_records_for_shard_blob_dir(
    pack_root: &Path,
    _shard_path: &Path,
) -> Result<Vec<SemanticRecord>> {
    let blobs_dir = pack_root.join("blobs");
    let embedder = DeterministicEmbedder;
    let mut index = SemanticIndex::new();
    if blobs_dir.is_dir() {
        for entry in fs::read_dir(&blobs_dir)? {
            let path = entry?.path();
            if !path.is_file() {
                continue;
            }
            let content = fs::read(&path)?;
            let hash = ContentHash::of(&content);
            let defs = indexer::extract_defs(&hash, &content);
            let spans: Vec<_> = defs
                .iter()
                .map(|d| graphzero_semantic::spans::EmbedSpan {
                    start: d.start,
                    end: d.end,
                    label: d.name.clone(),
                })
                .collect();
            if spans.is_empty() {
                continue;
            }
            index.upsert_blob(hash, &content, &spans, &embedder);
        }
    }
    Ok(index.records().to_vec())
}

/// Attach semantic sidecar metadata to an unsigned manifest (call before sign).
pub fn attach_semantic_sidecars(manifest: &mut PackManifest, pack_root: &Path) -> Result<()> {
    manifest.semantic_sidecars = build_semantic_sidecars_for_pack(pack_root)?;
    Ok(())
}

/// Copy installed semantic sidecars beside gzsh shards in the store pack dir.
pub fn install_semantic_sidecars(
    pack_root: &Path,
    dest_shards_dir: &Path,
    manifest: &PackManifest,
) -> Result<()> {
    for entry in &manifest.semantic_sidecars {
        let src = pack_root.join("shards").join(&entry.file_name);
        if !src.is_file() {
            anyhow::bail!("missing semantic sidecar {}", entry.file_name);
        }
        let dst = dest_shards_dir.join(&entry.file_name);
        fs::copy(&src, &dst).context("copy semantic sidecar")?;
    }
    Ok(())
}

/// Golden vector hash for an installed sidecar (FR-004).
pub fn golden_hash_for_installed_sidecar(
    dest_shards_dir: &Path,
    file_name: &str,
) -> Result<String> {
    let path = dest_shards_dir.join(file_name);
    let reader = SemanticShardReader::open(&path)?;
    reader.golden_vector_hash()
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}
