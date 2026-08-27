//! Shared fuzzy name ranking for CodeMode discovery and plan validation.

/// Rank a candidate name against a needle (lower is better).
pub(crate) fn name_score(needle: &str, candidate: &str) -> usize {
    if candidate == needle {
        0
    } else if candidate.starts_with(needle) || needle.starts_with(candidate) {
        1
    } else {
        2 + edit_distance(needle, candidate)
    }
}

/// Levenshtein distance over Unicode scalar values.
pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let a = a.chars().collect::<Vec<_>>();
    let b = b.chars().collect::<Vec<_>>();
    let mut prev = (0..=b.len()).collect::<Vec<_>>();
    let mut cur = vec![0; b.len() + 1];
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

/// Sort `(score, name)` pairs (lower score better; name tie-break) and take top `k` names.
#[inline]
pub(crate) fn take_top_ranked<T: Ord>(mut ranked: Vec<(usize, T)>, k: usize) -> Vec<T> {
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    ranked.into_iter().take(k).map(|(_, name)| name).collect()
}
