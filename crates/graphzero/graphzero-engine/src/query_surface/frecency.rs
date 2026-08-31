//! Apply store frecency to search/blast display order.
//! Scores are heuristic. Blast `confidence` stays path-min; this only reorders.

use std::collections::HashMap;

use graphzero_store::Snapshot;
use graphzero_store::store::frecency::{
    FrecencyLedger, ai_mode, as_of_from_snapshot_nanos, blob_hash_from_ref, combined_entry, load,
    path_from_evidence_ref, score,
};

use super::types::SearchHit;

pub struct RankCtx {
    ledger: FrecencyLedger,
    as_of: u64,
    ai: bool,
    hash_to_path: HashMap<String, String>,
    path_mtime: HashMap<String, u64>,
}

impl RankCtx {
    pub fn load(snapshot: &Snapshot) -> Self {
        let mut hash_to_path = HashMap::new();
        let mut path_mtime = HashMap::new();
        for (hash, rec) in snapshot.path_records() {
            hash_to_path.insert(hash.to_hex(), rec.path.clone());
            let mtime = (rec.mtime_nanos / 1_000_000_000) as u64;
            if mtime > 0 {
                path_mtime.insert(rec.path.clone(), mtime);
            }
        }
        Self {
            ledger: load(&snapshot.store_root),
            as_of: as_of_from_snapshot_nanos(snapshot.entry.timestamp_nanos),
            ai: ai_mode(),
            hash_to_path,
            path_mtime,
        }
    }

    pub fn score(&self, label: &str, evidence_ref: &str) -> f64 {
        let blob = blob_hash_from_ref(evidence_ref);
        let path = path_from_evidence_ref(evidence_ref)
            .or_else(|| {
                blob.as_ref()
                    .and_then(|hash| self.hash_to_path.get(hash).cloned())
            })
            .or_else(|| {
                if label.contains('/') || label.contains('.') {
                    Some(label.split('#').next().unwrap_or(label).to_string())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        let mut entry = combined_entry(&self.ledger, &path, blob.as_deref());
        if entry.last_commit_unix == 0 {
            if let Some(mtime) = self.path_mtime.get(&path).copied() {
                entry.last_commit_unix = mtime;
            }
        }
        score(&entry, self.as_of, self.ai)
    }
}

pub fn rank_search_hits(snapshot: &Snapshot, hits: &mut [SearchHit]) {
    if hits.len() < 2 {
        return;
    }
    let ctx = RankCtx::load(snapshot);
    hits.sort_by(|a, b| {
        ctx.score(&a.label, &a.evidence_ref)
            .total_cmp(&ctx.score(&b.label, &b.evidence_ref))
            .then_with(|| a.content_sha256.cmp(&b.content_sha256))
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.evidence_ref.cmp(&b.evidence_ref))
    });
}
