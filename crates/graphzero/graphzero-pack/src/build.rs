//! Reproducible pack build from dependency source blobs (tier-A walking skeleton).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use graphzero_store::ContentHash;
use graphzero_store::store::coverage::CoverageBitmap;
use graphzero_store::store::csr::CsrBuilder;
use graphzero_store::store::format::{SpanEntry, symbol_kind};
use graphzero_store::store::indexer::{self, IndexData};
use graphzero_store::store::shard::{ShardBuilder, ShardReader};
use graphzero_store::store::symbol_table::SymbolTableBuilder;
use sha2::{Digest, Sha256};

use crate::manifest::{
    MANIFEST_SCHEMA_VERSION, PackManifest, PackProvenance, PackShardEntry, PackSignKey,
};
use crate::semantic_index::attach_semantic_sidecars;
use crate::sign::sign_manifest;

/// Deterministic digest for reproducible_build tests (signature cleared).
pub fn reproducible_manifest_digest(manifest: &PackManifest) -> Result<String> {
    manifest.digest_sha256()
}

/// Build a minimal fixture pack (two synthetic dep files) for tests and benches.
pub fn build_fixture_pack(out_dir: &Path, key: &PackSignKey) -> Result<PathBuf> {
    let sources = vec![
        (
            "dep_alpha.rs".to_string(),
            b"fn alpha(x: u64) -> u64 {\n    beta(x)\n}\nfn beta(v: u64) -> u64 { v }\n".to_vec(),
        ),
        (
            "dep_beta.rs".to_string(),
            b"fn gamma() { alpha(1); }\n".to_vec(),
        ),
    ];
    build_pack_from_sources(out_dir, "fixture-deps", "0.1.0", &sources, key)
}

pub fn build_pack_from_sources(
    out_dir: &Path,
    pack_id: &str,
    version: &str,
    sources: &[(String, Vec<u8>)],
    key: &PackSignKey,
) -> Result<PathBuf> {
    fs::create_dir_all(out_dir)?;
    fs::create_dir_all(out_dir.join("shards"))?;
    let (data, lockfile_sha256) = index_dep_sources(out_dir, sources)?;
    write_signed_pack_manifest(out_dir, pack_id, version, &data, lockfile_sha256, key)
}

fn index_dep_sources(out_dir: &Path, sources: &[(String, Vec<u8>)]) -> Result<(IndexData, String)> {
    let mut data = IndexData::default();
    let mut lock_hasher = Sha256::new();
    let blobs_dir = out_dir.join("blobs");
    fs::create_dir_all(&blobs_dir)?;
    for (path, content) in sources {
        lock_hasher.update(path.as_bytes());
        lock_hasher.update(content);
        let hash = ContentHash::of(content);
        fs::write(blobs_dir.join(hash.to_hex()), content)?;
        data.blobs.insert(
            hash,
            indexer::BlobMeta {
                path: path.clone(),
                mtime_nanos: 0,
                size: content.len() as u64,
                tier_bits: 0b001,
                content_len: content.len(),
            },
        );
        data.blob_order.push(hash);
        data.defs.extend(indexer::extract_defs(&hash, content));
    }
    let known: BTreeMap<String, ()> = data.defs.iter().map(|d| (d.name.clone(), ())).collect();
    for (_path, content) in sources {
        let hash = ContentHash::of(content);
        let local: Vec<_> = data
            .defs
            .iter()
            .filter(|d| d.blob == hash)
            .cloned()
            .collect();
        data.edges
            .extend(indexer::extract_edges(&hash, content, &known, &local));
    }
    Ok((data, hex::encode(lock_hasher.finalize())))
}

fn write_signed_pack_manifest(
    out_dir: &Path,
    pack_id: &str,
    version: &str,
    data: &IndexData,
    lockfile_sha256: String,
    key: &PackSignKey,
) -> Result<PathBuf> {
    let shards_dir = out_dir.join("shards");
    let snapshot_id = 1u64;
    let shard = build_global_dep_shard(data)?;
    let shard_name = format!("shard_{snapshot_id:08}_0000.gzsh");
    let shard_path = shards_dir.join(&shard_name);
    let file_hash64 = shard.write_to(&shard_path)?;
    let content_sha256 = sha256_file(&shard_path)?;
    let blob_count = data.blob_order.len() as u32;
    let tier_a = if blob_count == 0 { 0.0 } else { 100.0 };
    let mut manifest = PackManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        pack_id: pack_id.to_string(),
        version: version.to_string(),
        tier_a_coverage: tier_a,
        shards: vec![PackShardEntry {
            file_name: shard_name,
            content_sha256,
            blob_count,
            file_hash64,
        }],
        provenance: PackProvenance {
            lockfile_sha256,
            toolchain: "rustc-test".into(),
            built_at_unix_nanos: 0,
        },
        signature_hex: String::new(),
        semantic_sidecars: Vec::new(),
    };
    attach_semantic_sidecars(&mut manifest, out_dir)?;
    sign_manifest(&mut manifest, key)?;
    let manifest_path = out_dir.join("manifest.json");
    manifest.write_json(&manifest_path)?;
    Ok(manifest_path)
}

fn build_global_dep_shard(data: &IndexData) -> Result<ShardBuilder> {
    let mut stb = SymbolTableBuilder::new();
    for d in &data.defs {
        stb.insert(&d.name, d.kind, 0);
    }
    for e in &data.edges {
        stb.insert(&e.src, symbol_kind::OTHER, 0);
        stb.insert(&e.dst, symbol_kind::OTHER, 0);
    }
    let symbols = stb.build()?;
    let id_of: BTreeMap<&str, u32> = symbols
        .names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i as u32))
        .collect();
    let blob_idx_of: BTreeMap<ContentHash, u32> = data
        .blob_order
        .iter()
        .enumerate()
        .map(|(i, h)| (*h, i as u32))
        .collect();
    let mut spans: Vec<SpanEntry> = data
        .defs
        .iter()
        .map(|d| SpanEntry {
            blob_idx: blob_idx_of[&d.blob],
            start: d.start,
            end: d.end,
            symbol_id: id_of[d.name.as_str()],
            block_start: d.block_start,
            block_end: d.block_end,
        })
        .collect();
    spans.sort_by_key(|s| s.symbol_id);
    let mut csr_builder = CsrBuilder::new();
    for e in &data.edges {
        csr_builder.add_edge_with_evidence(
            id_of[e.src.as_str()],
            id_of[e.dst.as_str()],
            e.kind,
            e.confidence,
            SpanEntry {
                blob_idx: blob_idx_of[&e.blob],
                start: e.start,
                end: e.end,
                symbol_id: id_of[e.dst.as_str()],
                block_start: 0,
                block_end: 0,
            },
        );
    }
    let csr = csr_builder.build(symbols.names.len());
    let mut coverage = CoverageBitmap::new(data.blob_order.len());
    let mut coverage_blobs = Vec::with_capacity(data.blob_order.len());
    for (i, hash) in data.blob_order.iter().enumerate() {
        coverage_blobs.push(hash.0);
        if data.blobs[hash].tier_bits & 0b001 != 0 {
            coverage.set(i, graphzero_store::Tier::A, true);
        }
    }
    Ok(ShardBuilder {
        symbols,
        spans,
        csr,
        trigrams: Vec::new(),
        coverage_blobs,
        coverage,
    })
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

/// Validate on-disk shards match manifest entries (FR-001).
pub fn verify_pack_artifacts(pack_root: &Path, manifest: &PackManifest) -> Result<()> {
    let shards_dir = pack_root.join("shards");
    if manifest.shards.len() != count_gzsh(&shards_dir)? {
        anyhow::bail!("manifest shard count does not match on-disk gzsh files");
    }
    for entry in &manifest.shards {
        let path = shards_dir.join(&entry.file_name);
        if !path.is_file() {
            anyhow::bail!("missing shard file {}", entry.file_name);
        }
        let digest = sha256_file(&path)?;
        if digest != entry.content_sha256 {
            anyhow::bail!("shard {} content_sha256 mismatch", entry.file_name);
        }
        ShardReader::open(&path).context("open gzsh shard")?;
    }
    Ok(())
}

fn count_gzsh(dir: &Path) -> Result<usize> {
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut n = 0;
    for e in fs::read_dir(dir)? {
        let p = e?.path();
        if p.extension().is_some_and(|x| x == "gzsh") {
            n += 1;
        }
    }
    Ok(n)
}
