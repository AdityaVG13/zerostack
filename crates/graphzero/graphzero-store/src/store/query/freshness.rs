//! Blob freshness verification against on-disk repo files (INV-001).

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use crate::ContentHash;

use super::snapshot::Snapshot;
use super::types::{CapsuleDef, CapsuleMatch, FreshnessDiagnostics, PathRecord};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StalenessVerdict {
    Fresh,
    StatOnlyChange,
    Stale,
    Missing,
    Unreadable,
}

pub fn file_mtime_nanos(meta: &fs::Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

pub fn blob_staleness_verdict(
    repo: &Path,
    rel: &str,
    indexed_hash_hex: &str,
    rec: &PathRecord,
) -> StalenessVerdict {
    let path = repo.join(rel);
    let Ok(meta) = fs::metadata(&path) else {
        return StalenessVerdict::Missing;
    };
    let mtime = file_mtime_nanos(&meta);
    // Stat fast path: when mtime AND size are byte-identical to the indexed
    // record, the file is the one that was hashed at index time. Reading and
    // re-hashing every indexed file on every freshness probe turned each warm
    // query into a full-repo scan (orient p50 regression); stat metadata is
    // the same evidence `git status` uses. Only a stat mismatch falls through
    // to the authoritative read+hash check.
    if mtime == rec.mtime_nanos && meta.len() == rec.size {
        return StalenessVerdict::Fresh;
    }
    match fs::read(&path) {
        Ok(content) => {
            if ContentHash::of(&content).to_hex() != indexed_hash_hex {
                StalenessVerdict::Stale
            } else if mtime == rec.mtime_nanos && meta.len() == rec.size {
                StalenessVerdict::Fresh
            } else {
                StalenessVerdict::StatOnlyChange
            }
        }
        Err(_) => StalenessVerdict::Unreadable,
    }
}

pub fn render_def_staleness(
    snapshot: &Snapshot,
    hash_hex: &str,
    check_freshness: bool,
) -> (Option<String>, bool) {
    let record = ContentHash::from_hex(hash_hex).and_then(|h| snapshot.paths().get(&h));
    let mut stale = false;
    if check_freshness && let (Some(rec), Some(repo)) = (record, snapshot.repo_root.as_ref()) {
        match blob_staleness_verdict(repo, &rec.path, hash_hex, rec) {
            StalenessVerdict::Fresh | StalenessVerdict::StatOnlyChange => {}
            StalenessVerdict::Stale | StalenessVerdict::Missing | StalenessVerdict::Unreadable => {
                stale = true;
            }
        }
    }
    (record.map(|r| r.path.clone()), stale)
}

pub fn record_staleness_event(
    diag: &mut FreshnessDiagnostics,
    rel: &str,
    verdict: StalenessVerdict,
) {
    match verdict {
        StalenessVerdict::Fresh => {}
        StalenessVerdict::StatOnlyChange => {
            diag.events.push(format!("stat_changed_hash_same:{rel}"));
        }
        StalenessVerdict::Stale => {
            diag.events.push(format!("stale_detected:{rel}"));
        }
        StalenessVerdict::Missing => {
            diag.events.push(format!("missing_file:{rel}"));
        }
        StalenessVerdict::Unreadable => {
            diag.events.push(format!("unreadable:{rel}"));
        }
    }
}

pub fn indexed_path_stale_vs_disk(
    repo: &Path,
    rel: &str,
    hash_hex: &str,
    rec: &PathRecord,
    diag: &mut FreshnessDiagnostics,
) -> bool {
    let verdict = blob_staleness_verdict(repo, rel, hash_hex, rec);
    record_staleness_event(diag, rel, verdict);
    matches!(
        verdict,
        StalenessVerdict::Stale | StalenessVerdict::Missing | StalenessVerdict::Unreadable
    )
}

pub fn path_record_for_rel<'a>(
    snapshot: &'a Snapshot,
    rel: &str,
) -> Option<(String, &'a PathRecord)> {
    snapshot
        .paths()
        .iter()
        .find_map(|(hash, rec)| (rec.path == rel).then_some((hash.to_hex(), rec)))
}

pub fn collect_stale_from_indexed_defs(
    snapshot: &Snapshot,
    repo: &Path,
    matches: &[CapsuleMatch],
    checked_paths: &mut BTreeSet<String>,
    stale_paths: &mut BTreeSet<String>,
    diag: &mut FreshnessDiagnostics,
) {
    for m in matches {
        for d in &m.defs {
            let Some(rel) = &d.path else {
                continue;
            };
            if !checked_paths.insert(rel.clone()) {
                continue;
            }
            diag.hash_checks += 1;
            if d.stale {
                stale_paths.insert(rel.clone());
                diag.events.push(format!("stale_detected:{rel}"));
                continue;
            }
            let Some((hash_hex, rec)) = path_record_for_rel(snapshot, rel) else {
                continue;
            };
            if indexed_path_stale_vs_disk(repo, rel, &hash_hex, rec, diag) {
                stale_paths.insert(rel.clone());
            }
        }
    }
}

pub fn collect_stale_when_symbol_missing(
    snapshot: &Snapshot,
    repo: &Path,
    symbol: &str,
    matches: &[CapsuleMatch],
    checked_paths: &mut BTreeSet<String>,
    stale_paths: &mut BTreeSet<String>,
    diag: &mut FreshnessDiagnostics,
) {
    if !matches.iter().all(|m| m.name != symbol) {
        return;
    }
    if let Some(rel) = Snapshot::first_unindexed_source_path(repo, snapshot.paths()) {
        diag.events.push(format!("unindexed_source:{rel}"));
        stale_paths.insert(rel);
    }
    for (hash, rec) in snapshot.paths() {
        if !checked_paths.insert(rec.path.clone()) {
            continue;
        }
        diag.hash_checks += 1;
        let hash_hex = hash.to_hex();
        if indexed_path_stale_vs_disk(repo, &rec.path, &hash_hex, rec, diag) {
            stale_paths.insert(rec.path.clone());
        }
    }
}

pub fn snapshot_staleness_diagnostic(
    repo: &Path,
    indexed: &HashMap<ContentHash, PathRecord>,
) -> Option<String> {
    if let Some(path) = Snapshot::first_unindexed_source_path(repo, indexed) {
        return Some(format!("unindexed_source:{path}"));
    }
    // Sequential stat scan (graphzero perf note): rayon `find_first` was tried
    // and measured slower — rayon pool/task overhead exceeds the ~0.9ms the
    // 930 per-file stats cost at this corpus size. Revisit if typical repos
    // grow 10x.
    for (hash, rec) in indexed {
        let hash_hex = hash.to_hex();
        let verdict = blob_staleness_verdict(repo, &rec.path, &hash_hex, rec);
        match verdict {
            StalenessVerdict::Stale => return Some(format!("hash_mismatch:{}", rec.path)),
            StalenessVerdict::Missing => return Some(format!("missing_file:{}", rec.path)),
            StalenessVerdict::Unreadable => return Some(format!("unreadable:{}", rec.path)),
            StalenessVerdict::StatOnlyChange => {}
            StalenessVerdict::Fresh => {}
        }
    }
    None
}

pub fn merge_repaired_def_batch(
    symbol: &str,
    rel: &str,
    defs: Vec<(String, String, u32, u32)>,
    matches: &mut Vec<CapsuleMatch>,
) {
    for (name, hash_hex, start, end) in defs {
        if name != symbol && !name.starts_with(symbol) {
            continue;
        }
        let evidence_ref = super::super::refs::blob_span_ref(&hash_hex, start, end);
        if let Some(m) = matches.iter_mut().find(|m| m.name == name) {
            if !m.defs.iter().any(|d| d.evidence_ref == evidence_ref) {
                m.defs.push(CapsuleDef {
                    evidence_ref,
                    path: Some(rel.to_string()),
                    stale: false,
                });
            }
        } else {
            matches.push(CapsuleMatch {
                name: name.clone(),
                defs: vec![CapsuleDef {
                    evidence_ref,
                    path: Some(rel.to_string()),
                    stale: false,
                }],
                edges: Vec::new(),
            });
        }
    }
}
