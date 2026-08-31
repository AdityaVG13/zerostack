//! Worktree view: shared snapshot + per-id overlay.

use std::path::Path;

use anyhow::Result;

use super::absence::{AbsenceAnswer, AbsenceCertificate, AbsenceConfig, AnswerClass};
use super::overlay::{load_overlay, query_overlay};
use super::query::{Capsule, PendingFacts, Snapshot};
use crate::Tier;

/// Query surface scoped to one worktree overlay id.
pub struct WorktreeView {
    pub id: String,
    snapshot: Snapshot,
    overlay: PendingFacts,
}

impl WorktreeView {
    pub fn open(store_root: &Path, repo_root: &Path, worktree_id: &str) -> Result<Self> {
        let snapshot = Snapshot::open(store_root, Some(repo_root))?;
        let overlay = load_overlay(store_root, worktree_id)?;
        Ok(Self {
            id: worktree_id.to_string(),
            snapshot,
            overlay,
        })
    }

    pub fn from_parts(snapshot: Snapshot, worktree_id: &str, overlay: PendingFacts) -> Self {
        Self {
            id: worktree_id.to_string(),
            snapshot,
            overlay,
        }
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn overlay(&self) -> &PendingFacts {
        &self.overlay
    }

    pub fn query(&self, symbol: &str, budget: usize, check_freshness: bool) -> Result<Capsule> {
        query_overlay(
            &self.snapshot,
            &self.overlay,
            symbol,
            budget,
            check_freshness,
        )
    }

    pub fn absence(&self, symbol: &str, config: AbsenceConfig) -> Result<AbsenceAnswer> {
        absence_overlay(&self.snapshot, &self.overlay, symbol, config)
    }
}

/// Certified absence through overlay paths with lazy freshness.
pub fn absence_overlay(
    snapshot: &Snapshot,
    overlay: &PendingFacts,
    symbol: &str,
    config: AbsenceConfig,
) -> Result<AbsenceAnswer> {
    let query = symbol.trim().to_string();
    let capsule = query_overlay(snapshot, overlay, &query, 256, config.check_freshness)?;
    let stale_reason = if config.check_freshness {
        overlay_staleness_diagnostic(snapshot, overlay)
    } else {
        None
    };

    let tier_a = capsule.tier_a;
    let snapshot_id = capsule.snapshot_id;
    let gap_blob_count = snapshot.unindexed_blob_count(Tier::A);
    let tier_a_pct = tier_a * 100.0;
    let cert = AbsenceCertificate {
        tier_a_pct,
        tier_b_pct: capsule.tier_b * 100.0,
        tier_c_pct: capsule.tier_c * 100.0,
        freshness_verified: stale_reason.is_none() && tier_a >= config.tier_a_threshold,
        snapshot_id,
        generated_at_secs: unix_now_secs(),
        gap_blob_count,
    };

    if !capsule.matches.is_empty() {
        let evidence_ref = capsule
            .matches
            .first()
            .and_then(|m| m.defs.first())
            .map(|d| d.evidence_ref.clone());
        let summary = format!(
            "present: symbol {:?} found (snapshot {})",
            query, snapshot_id
        );
        return Ok(AbsenceAnswer {
            class: AnswerClass::Present,
            query,
            certificate: cert,
            evidence_ref,
            staleness_reason: None,
            summary,
        });
    }

    if let Some(reason) = &stale_reason {
        let summary = format!(
            "unknown: overlay stale — {}; tier-A {:.1}% (snapshot {})",
            reason, tier_a_pct, snapshot_id
        );
        return Ok(AbsenceAnswer {
            class: AnswerClass::Unknown,
            query,
            certificate: cert,
            evidence_ref: None,
            staleness_reason: stale_reason,
            summary,
        });
    }

    if tier_a < config.tier_a_threshold {
        let summary = format!(
            "unknown: tier-A coverage {:.1}% below threshold (snapshot {})",
            tier_a_pct, snapshot_id
        );
        return Ok(AbsenceAnswer {
            class: AnswerClass::Unknown,
            query,
            certificate: cert,
            evidence_ref: None,
            staleness_reason: Some("partial_coverage".into()),
            summary,
        });
    }

    let summary = format!(
        "absent: no symbol {:?} under tier-A {:.1}% fresh overlay coverage (snapshot {})",
        query, tier_a_pct, snapshot_id
    );
    Ok(AbsenceAnswer {
        class: AnswerClass::Absent,
        query,
        certificate: cert,
        evidence_ref: None,
        staleness_reason: None,
        summary,
    })
}

/// Lazily verify content hashes for overlay-indexed paths.
pub fn overlay_staleness_diagnostic(snapshot: &Snapshot, overlay: &PendingFacts) -> Option<String> {
    let repo = snapshot.repo_root.as_ref()?;
    for (blob, rel) in &overlay.paths {
        let path = repo.join(rel);
        let Ok(content) = std::fs::read(&path) else {
            return Some(format!("missing_file:{rel}"));
        };
        let live = crate::ContentHash::of(&content);
        if live.0 != *blob {
            return Some(format!("hash_mismatch:{rel}"));
        }
    }
    None
}

fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
