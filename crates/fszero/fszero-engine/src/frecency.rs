//! Frecency ranking for file results (fszero-rwda).

use std::cmp::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy)]
pub struct FrecencySignals {
    pub visits: u32,
    pub last_visit_secs: u64,
    /// Optional git warm: recent touch boost 0.0..1.0
    pub git_warm: f64,
}

/// AI-mode decay: score = visits / (1 + age_hours)^decay * (1 + git_warm).
pub fn frecency_score(sig: FrecencySignals, now_secs: u64, decay: f64) -> f64 {
    let age_secs = now_secs.saturating_sub(sig.last_visit_secs) as f64;
    let age_hours = age_secs / 3600.0;
    let visits = f64::from(sig.visits.max(1));
    let base = visits / (1.0 + age_hours).powf(decay.max(0.1));
    base * (1.0 + sig.git_warm.clamp(0.0, 1.0))
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs()
}

pub fn rank_paths(mut items: Vec<(String, FrecencySignals)>, decay: f64) -> Vec<String> {
    let now = now_secs();
    items.sort_by(|a, b| {
        let sa = frecency_score(a.1, now, decay);
        let sb = frecency_score(b.1, now, decay);
        sb.partial_cmp(&sa).unwrap_or(Ordering::Equal)
    });
    items.into_iter().map(|(p, _)| p).collect()
}
