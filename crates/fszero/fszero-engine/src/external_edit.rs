//! External-edit detection via mtime/len signatures (fszero-6m5h), plus the
//! V6-F4 (ZS-STORE-005) commit-time rescan-gate receipt record: every
//! external effect on a committed path is receipted durably and bound into
//! the world's commit record -- never a silent absorb.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSig {
    pub mtime_secs: u64,
    pub len: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ExternalEditDetector {
    baseline: BTreeMap<String, FileSig>,
}

impl ExternalEditDetector {
    pub fn snapshot_file(path: &Path) -> Option<FileSig> {
        let meta = fs::metadata(path).ok()?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Some(FileSig {
            mtime_secs: mtime,
            len: meta.len(),
        })
    }

    pub fn remember(&mut self, rel: impl Into<String>, sig: FileSig) {
        self.baseline.insert(rel.into(), sig);
    }

    /// Paths whose current sig differs from baseline (external edit).
    pub fn detect(&self, root: &Path, rels: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for rel in rels {
            let Some(base) = self.baseline.get(rel) else {
                continue;
            };
            let cur = match Self::snapshot_file(&root.join(rel)) {
                Some(s) => s,
                None => {
                    out.push(rel.clone());
                    continue;
                }
            };
            if &cur != base {
                out.push(rel.clone());
            }
        }
        out
    }
}

/// Disposition of one external-effect detection (V6-F4 / ZS-STORE-005).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalEffectDisposition {
    /// Undeclared mutation on a committed path at commit time: the commit is
    /// refused, nothing is written, and the receipt records why.
    Refused,
    /// External base absorbed deliberately via `resolve:mine` / `resolve:merged`
    /// or `rebase`: the agent declared the current disk base as the world's
    /// base, so the commit may proceed -- but the absorb is receipted.
    DeclaredAbsorb,
}

/// Durable receipt for one external edit on a committed path. Bound into the
/// world's commit record (`world_{wid}/external_effects`) -- never a silent
/// absorb (ZS-STORE-005).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalEffectReceipt {
    /// Repo-relative path of the externally edited file.
    pub path: String,
    /// Digest of the session's last known state for the path (sha256 hex);
    /// empty when the session had no tracked state ("old digest if known").
    pub old_digest: String,
    /// Digest of the on-disk content at detection (sha256 hex); empty when
    /// there is no content on disk (external delete / declared-empty base).
    pub new_digest: String,
    /// Detection-time ordinal: the mutation-journal seq the next journal row
    /// will take (monotonic, durable-anchored, cross-checkable).
    pub detected_seq: i64,
    pub disposition: ExternalEffectDisposition,
}

impl ExternalEffectReceipt {
    pub fn new(
        path: impl Into<String>,
        old_digest: impl Into<String>,
        new_digest: impl Into<String>,
        detected_seq: i64,
        disposition: ExternalEffectDisposition,
    ) -> Self {
        Self {
            path: path.into(),
            old_digest: old_digest.into(),
            new_digest: new_digest.into(),
            detected_seq,
            disposition,
        }
    }
}
