//! Worktree overlay: diverged blobs live as a delta-log layer in
//! `.graphzero/worktrees/<id>/wal` over the shared snapshot. Reads combine
//! shared + delta; coverage reflects the worktree.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::ContentHash;

use super::blob_store::BlobStore;
use super::delta_log::{DeltaEntry, DeltaLog, entry_type, read_all_segments};
use super::indexer::{DEFAULT_EDGE_CONFIDENCE, extract_defs, extract_edges};
use super::lock::WriterLock;
use super::path_safety::validate_safe_id;
use super::query::{Capsule, PendingFacts, Snapshot, encode_edge_with_meta, encode_symbol};

fn worktree_wal(store_root: &Path, worktree_id: &str) -> Result<PathBuf> {
    validate_safe_id(worktree_id, "worktree_id")?;
    Ok(store_root.join("worktrees").join(worktree_id).join("wal"))
}

fn blob_hex(hash: &[u8; 32]) -> String {
    crate::fast_hex_32(hash)
}

fn append_worktree_file_entries(
    log: &mut DeltaLog,
    store_root: &Path,
    rel: &str,
    hash: ContentHash,
    content: &[u8],
) -> Result<()> {
    log.append(DeltaEntry {
        entry_type: entry_type::BLOB,
        blob_hash: hash.0,
        payload: rel.as_bytes().to_vec(),
    })?;
    let defs = extract_defs(&hash, content);
    let known: std::collections::BTreeMap<String, ()> =
        defs.iter().map(|d| (d.name.clone(), ())).collect();
    for d in &defs {
        log.append(DeltaEntry {
            entry_type: entry_type::SYMBOL,
            blob_hash: hash.0,
            payload: encode_symbol(&d.name, d.kind, 0, d.start, d.end)?,
        })?;
    }
    let digest = blob_hex(&hash.0);
    // Opt-in outline + semantic provenance on def/span transforms.
    for d in &defs {
        let _ = super::provenance::attach_def_span_provenance(
            store_root,
            &digest,
            d.start,
            d.end,
            d.block_start,
            d.block_end,
            &d.name,
            Some(content),
        )?;
    }
    for e in extract_edges(&hash, content, &known, &defs) {
        log.append(DeltaEntry {
            entry_type: entry_type::EDGE,
            blob_hash: hash.0,
            payload: encode_edge_with_meta(
                &e.src,
                &e.dst,
                e.kind,
                DEFAULT_EDGE_CONFIDENCE,
                e.start,
                e.end,
                None,
            )?,
        })?;
        // Opt-in per-row provenance; no-op when disabled.
        let _ = super::provenance::attach_overlay_edge_provenance(
            store_root,
            &digest,
            e.start,
            e.end,
            &e.src,
            &e.dst,
            e.kind,
            Some(content),
        )?;
    }
    log.append(DeltaEntry {
        entry_type: entry_type::COVERAGE,
        blob_hash: hash.0,
        payload: vec![0b001],
    })?;
    Ok(())
}

fn strip_dirty_base_defs(capsule: &mut Capsule, dirty_paths: &BTreeSet<&str>) {
    for m in &mut capsule.matches {
        m.defs.retain(|d| {
            d.path
                .as_ref()
                .map(|p| !dirty_paths.contains(p.as_str()))
                .unwrap_or(true)
        });
    }
}

fn overlay_def_matches_symbol(name: &str, symbol: &str) -> bool {
    name == symbol || name.starts_with(symbol)
}

fn push_overlay_def(capsule: &mut Capsule, name: &str, def: super::query::CapsuleDef) {
    if let Some(m) = capsule.matches.iter_mut().find(|m| m.name == name) {
        m.defs.push(def);
    } else {
        capsule.matches.push(super::query::CapsuleMatch {
            name: name.to_string(),
            defs: vec![def],
            edges: Vec::new(),
        });
    }
}

fn merge_overlay_defs(
    capsule: &mut Capsule,
    overlay: &PendingFacts,
    symbol: &str,
    repo: Option<&Path>,
    check_freshness: bool,
    freshness: &mut super::query::FreshnessDiagnostics,
) {
    for (name, blob, start, end) in &overlay.defs {
        if !overlay_def_matches_symbol(name, symbol) {
            continue;
        }
        let hex = blob_hex(blob);
        let rel = overlay.paths.get(blob).cloned();
        let mut stale = false;
        if check_freshness && let (Some(r), Some(repo)) = (rel.as_deref(), repo) {
            stale = overlay_def_stale(repo, r, blob);
            freshness.hash_checks += 1;
            if stale {
                freshness.events.push(format!("stale_detected_overlay:{r}"));
            }
        }
        let def = super::query::CapsuleDef {
            evidence_ref: super::refs::blob_span_ref(&hex, *start, *end),
            path: rel,
            stale,
        };
        push_overlay_def(capsule, name, def);
    }
}

fn merge_overlay_edges(capsule: &mut Capsule, overlay: &PendingFacts) {
    for (src, dst, kind, conf, blob, start, end, source) in &overlay.edges {
        let Some(m) = capsule.matches.iter_mut().find(|m| m.name == *src) else {
            continue;
        };
        let hex = blob_hex(blob);
        m.edges.push(super::query::CapsuleEdge {
            kind: *kind,
            to: dst.clone(),
            confidence: *conf as f64 / 255.0,
            evidence_ref: super::refs::blob_span_ref(&hex, *start, *end),
            source: source.clone(),
        });
    }
}

fn apply_overlay_tier_a(
    capsule: &mut Capsule,
    snapshot: &Snapshot,
    overlay: &PendingFacts,
) -> Result<()> {
    let base_cov = snapshot.coverage()?;
    let base_total = base_cov.blob_count();
    let overlay_new = overlay.blobs.len();
    let total = base_total + overlay_new;
    let overlay_a = overlay.blobs.values().filter(|b| **b & 0b001 != 0).count();
    if total > 0 {
        capsule.tier_a = (base_cov.tier_a_count() + overlay_a) as f64 / total as f64;
    }
    Ok(())
}

/// Index modified worktree files into the overlay delta layer. Blob bytes
/// go to the shared blob store so evidence refs stay expandable.
pub fn index_worktree_files(
    store_root: &Path,
    worktree_id: &str,
    repo_root: &Path,
    rel_paths: &[&str],
) -> Result<()> {
    let wal_dir = worktree_wal(store_root, worktree_id)?;
    let _lock = WriterLock::acquire(store_root)?;
    fs::create_dir_all(&wal_dir)?;
    let blob_store = BlobStore::open(store_root)?;
    let mut log = DeltaLog::open_dir(&wal_dir)?;

    // Batch blob durability: put_nosync per file + one sync_all before WAL commit. Matches
    // cold-index; avoids per-file flat+cas-local fsync when indexing many dirty worktree paths.
    for rel in rel_paths {
        let content = fs::read(repo_root.join(rel))?;
        let hash = ContentHash::of(&content);
        blob_store.put_nosync_prehashed(hash, &content)?;
        append_worktree_file_entries(&mut log, store_root, rel, hash, &content)?;
    }
    blob_store.sync_all()?;
    log.commit()
}

/// Worktree facts loaded from the overlay layer.
pub fn load_overlay(store_root: &Path, worktree_id: &str) -> Result<PendingFacts> {
    let wal_dir = worktree_wal(store_root, worktree_id)?;
    let mut facts = PendingFacts::default();
    if wal_dir.is_dir() {
        for (_, entries) in read_all_segments(&wal_dir)? {
            let f = PendingFacts::from_entries(&entries);
            facts.defs.extend(f.defs);
            facts.edges.extend(f.edges);
            facts.blobs.extend(f.blobs);
            facts.paths.extend(f.paths);
        }
    }
    Ok(facts)
}

fn overlay_def_stale(repo: &Path, rel: &str, indexed_blob: &[u8; 32]) -> bool {
    let path = repo.join(rel);
    let Ok(content) = fs::read(&path) else {
        return true;
    };
    ContentHash::of(&content).0 != *indexed_blob
}

/// Query the shared snapshot through a worktree overlay: overlay facts for
/// diverged blobs shadow the base; coverage counts the worktree blobs.
pub fn query_overlay(
    snapshot: &Snapshot,
    overlay: &PendingFacts,
    symbol: &str,
    budget: usize,
    check_freshness: bool,
) -> Result<Capsule> {
    let mut capsule = snapshot.query(symbol, budget, check_freshness)?;

    let dirty_paths: BTreeSet<&str> = overlay.paths.values().map(|p| p.as_str()).collect();
    strip_dirty_base_defs(&mut capsule, &dirty_paths);

    let repo = snapshot.repo_root.as_deref();
    let mut freshness = capsule.freshness.clone();
    if check_freshness {
        freshness.check_freshness = true;
    }
    merge_overlay_defs(
        &mut capsule,
        overlay,
        symbol,
        repo,
        check_freshness,
        &mut freshness,
    );
    merge_overlay_edges(&mut capsule, overlay);
    apply_overlay_tier_a(&mut capsule, snapshot, overlay)?;
    capsule.freshness = freshness;
    Ok(capsule)
}
