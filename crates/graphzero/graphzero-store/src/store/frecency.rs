//! Heuristic frecency sidecar (graphzero-tyyi).
//!
//! Access counts live in `<store_root>/frecency.json` next to git-empirical
//! state. Decay is computed at read time (no background job). This ranking is
//! **not** a substitute for blast path-min confidence.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const FRECENCY_SCHEMA: &str = "graphzero.frecency.v1";
pub const HALF_LIFE_HUMAN_SECS: f64 = 10.0 * 86_400.0;
pub const HALF_LIFE_AI_SECS: f64 = 3.0 * 86_400.0;

/// fff AI-mode modification buckets: 30s / 5m / 15m / 1h / 4h.
const AI_MODIFY_BUCKETS: &[(f64, f64)] = &[
    (30.0, 4.0),
    (300.0, 3.0),
    (900.0, 2.5),
    (3_600.0, 2.0),
    (14_400.0, 1.5),
];

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PathFrecency {
    #[serde(default)]
    pub access_count: u64,
    #[serde(default)]
    pub last_access_unix: u64,
    #[serde(default)]
    pub last_commit_unix: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FrecencyLedger {
    pub schema: String,
    #[serde(default)]
    pub paths: BTreeMap<String, PathFrecency>,
}

impl Default for FrecencyLedger {
    fn default() -> Self {
        Self {
            schema: FRECENCY_SCHEMA.to_string(),
            paths: BTreeMap::new(),
        }
    }
}

pub fn ledger_path(store_root: &Path) -> PathBuf {
    store_root.join("frecency.json")
}

pub fn ai_mode() -> bool {
    match std::env::var("GRAPHZERO_FRECENCY_MODE") {
        Ok(mode) if mode.eq_ignore_ascii_case("human") => false,
        _ => true,
    }
}

pub fn half_life_secs(ai: bool) -> f64 {
    if ai {
        HALF_LIFE_AI_SECS
    } else {
        HALF_LIFE_HUMAN_SECS
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn as_of_from_snapshot_nanos(timestamp_nanos: i64) -> u64 {
    if timestamp_nanos <= 0 {
        return 0;
    }
    (timestamp_nanos as u128 / 1_000_000_000) as u64
}

pub fn decay(count: f64, age_secs: f64, half_life: f64) -> f64 {
    if count <= 0.0 || half_life <= 0.0 {
        return 0.0;
    }
    count * 0.5_f64.powf(age_secs.max(0.0) / half_life)
}

pub fn modify_bucket(age_secs: f64, ai: bool) -> f64 {
    let age = age_secs.max(0.0);
    if !ai {
        return if age <= 86_400.0 { 1.0 } else { 0.0 };
    }
    for &(limit, weight) in AI_MODIFY_BUCKETS {
        if age <= limit {
            return weight;
        }
    }
    0.0
}

pub fn score(entry: &PathFrecency, as_of_unix: u64, ai: bool) -> f64 {
    let half = half_life_secs(ai);
    let access = if entry.last_access_unix == 0 || entry.access_count == 0 {
        0.0
    } else {
        decay(
            entry.access_count as f64,
            as_of_unix.saturating_sub(entry.last_access_unix) as f64,
            half,
        )
    };
    let modify = if entry.last_commit_unix == 0 {
        0.0
    } else {
        let age = as_of_unix.saturating_sub(entry.last_commit_unix) as f64;
        modify_bucket(age, ai) + decay(1.0, age, half)
    };
    access + modify
}

pub fn score_path(
    ledger: &FrecencyLedger,
    path: &str,
    mtime_unix: Option<u64>,
    as_of_unix: u64,
    ai: bool,
) -> f64 {
    if path.is_empty() {
        return 0.0;
    }
    let mut entry = ledger.paths.get(path).cloned().unwrap_or_default();
    if entry.last_commit_unix == 0 {
        if let Some(mtime) = mtime_unix.filter(|m| *m > 0) {
            entry.last_commit_unix = mtime;
        }
    }
    score(&entry, as_of_unix, ai)
}

pub fn path_from_evidence_ref(reference: &str) -> Option<String> {
    let rest = reference.strip_prefix("gz://path/")?;
    let path = rest.split('#').next()?.trim();
    if path.is_empty() {
        None
    } else {
        Some(path.replace('\\', "/"))
    }
}

pub fn blob_hash_from_ref(reference: &str) -> Option<String> {
    let rest = reference
        .strip_prefix("gz://blob/")
        .or_else(|| reference.strip_prefix("gz://b/"))?;
    let hash = rest.split('#').next()?.trim();
    if hash.is_empty() || hash.chars().any(|c| !c.is_ascii_hexdigit()) {
        None
    } else {
        Some(hash.to_ascii_lowercase())
    }
}

pub fn load(store_root: &Path) -> FrecencyLedger {
    let path = ledger_path(store_root);
    let Ok(bytes) = std::fs::read(&path) else {
        return FrecencyLedger::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub fn save(store_root: &Path, ledger: &FrecencyLedger) -> Result<()> {
    let path = ledger_path(store_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create frecency dir {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(ledger).context("serialize frecency ledger")?;
    super::atomic_write_file(&path, &bytes).context("write frecency ledger")
}

pub fn merge_last_commits(store_root: &Path, path_unix: &BTreeMap<String, u64>) -> Result<()> {
    if path_unix.is_empty() {
        return Ok(());
    }
    let mut ledger = load(store_root);
    for (path, unix) in path_unix {
        if path.is_empty() || *unix == 0 {
            continue;
        }
        let entry = ledger.paths.entry(path.clone()).or_default();
        if *unix > entry.last_commit_unix {
            entry.last_commit_unix = *unix;
        }
    }
    save(store_root, &ledger)
}

pub fn touch_path(store_root: &Path, path: &str, as_of_unix: u64) -> Result<()> {
    if path.is_empty() {
        return Ok(());
    }
    let mut ledger = load(store_root);
    let entry = ledger.paths.entry(path.to_string()).or_default();
    entry.access_count = entry.access_count.saturating_add(1);
    entry.last_access_unix = entry.last_access_unix.max(as_of_unix);
    save(store_root, &ledger)
}

pub fn touch_evidence(store_root: &Path, reference: &str, as_of_unix: u64) -> Result<()> {
    if let Some(path) = path_from_evidence_ref(reference) {
        return touch_path(store_root, &path, as_of_unix);
    }
    if let Some(hash) = blob_hash_from_ref(reference) {
        return touch_path(store_root, &format!("blob:{hash}"), as_of_unix);
    }
    Ok(())
}

pub fn combined_entry(
    ledger: &FrecencyLedger,
    path: &str,
    blob_hash: Option<&str>,
) -> PathFrecency {
    let mut entry = ledger.paths.get(path).cloned().unwrap_or_default();
    if let Some(hash) = blob_hash {
        if let Some(blob) = ledger.paths.get(&format!("blob:{hash}")) {
            entry.access_count = entry.access_count.saturating_add(blob.access_count);
            entry.last_access_unix = entry.last_access_unix.max(blob.last_access_unix);
        }
    }
    entry
}
