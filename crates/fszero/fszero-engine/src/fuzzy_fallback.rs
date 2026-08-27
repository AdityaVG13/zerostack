//! Auto-fuzzy fallback on zero matches + weak-match detector (fszero-svr8).
//!
//! Used when literal `fs.search` finds zero exact hits: rank path/symbol
//! candidates by edit distance on basename/stem, then either surface strong
//! approximate hits or suppress uniformly weak noise.

use std::path::Path;

/// Levenshtein distance (simple DP); for short query/candidate tokens only.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Best edit distance of `query` against full path, basename, and stem.
pub fn best_path_distance(query: &str, candidate: &str) -> usize {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return usize::MAX;
    }
    let cl = candidate.to_lowercase();
    let mut best = edit_distance(&q, &cl);
    let path = Path::new(&cl);
    if let Some(base) = path.file_name().and_then(|s| s.to_str()) {
        best = best.min(edit_distance(&q, base));
        if let Some(stem) = Path::new(base).file_stem().and_then(|s| s.to_str()) {
            best = best.min(edit_distance(&q, stem));
        }
    }
    // Path segments (e.g. `search` in `src/core/search.rs`).
    for seg in cl.split(['/', '\\']) {
        if seg.is_empty() {
            continue;
        }
        best = best.min(edit_distance(&q, seg));
        if let Some(stem) = Path::new(seg).file_stem().and_then(|s| s.to_str()) {
            best = best.min(edit_distance(&q, stem));
        }
    }
    best
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyHit {
    pub candidate: String,
    pub distance: usize,
    /// Exact matches are never labeled fuzzy.
    pub exact: bool,
}

/// When exact hits empty, return weak fuzzy candidates (distance ≤ max_dist).
pub fn fuzzy_fallback(query: &str, candidates: &[String], max_dist: usize) -> Vec<FuzzyHit> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let exact: Vec<_> = candidates
        .iter()
        .filter(|c| {
            let cl = c.to_lowercase();
            cl.contains(&q) || c.eq_ignore_ascii_case(query)
        })
        .map(|c| FuzzyHit {
            candidate: c.clone(),
            distance: 0,
            exact: true,
        })
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    let mut hits: Vec<FuzzyHit> = candidates
        .iter()
        .filter_map(|c| {
            let d = best_path_distance(&q, c);
            if d <= max_dist {
                Some(FuzzyHit {
                    candidate: c.clone(),
                    distance: d,
                    exact: false,
                })
            } else {
                None
            }
        })
        .collect();
    hits.sort_by(|a, b| {
        a.distance
            .cmp(&b.distance)
            .then_with(|| a.candidate.cmp(&b.candidate))
    });
    hits
}

/// Legacy API: any non-exact positive distance is "fuzzy".
/// Prefer [`is_weak_match_for`] for quality gating.
pub fn is_weak_match(hit: &FuzzyHit) -> bool {
    !hit.exact && hit.distance > 0
}

/// Quality gate: single-typo fuzzy hits are strong; large relative distance is weak.
pub fn is_weak_match_for(hit: &FuzzyHit, query: &str) -> bool {
    if hit.exact || hit.distance == 0 {
        return false;
    }
    let qlen = query.trim().chars().count().max(1);
    match hit.distance {
        1 => false,
        2 => qlen < 6,
        d => d.saturating_mul(3) > qlen,
    }
}

/// True when every fuzzy hit is low-confidence (scatter noise).
pub fn is_uniformly_weak(hits: &[FuzzyHit], query: &str) -> bool {
    !hits.is_empty() && hits.iter().all(|h| is_weak_match_for(h, query))
}

/// Keep only strong approximate hits (for agent-facing payloads).
pub fn strong_fuzzy_hits(hits: Vec<FuzzyHit>, query: &str, limit: usize) -> Vec<FuzzyHit> {
    hits.into_iter()
        .filter(|h| !is_weak_match_for(h, query))
        .take(limit)
        .collect()
}
