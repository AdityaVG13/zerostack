//! Frecency over the existing recovery `order` log (tokenzero-gzc0).
//!
//! No new store files. Re-puts append to `order` (frequency + recency);
//! reads still do not. AI-mode decay is a 3-day half-life with
//! burst-compressed counts, matching fff. When only the insertion log is
//! available, age is the number of later order events (one event ≈ one tick
//! of the same decay curve, scaled so a 3-day half-life stays the formula
//! under test).

/// fff AI-mode half-life: three days in seconds.
pub const HALF_LIFE_SECS: u64 = 3 * 24 * 60 * 60;

/// Burst-compress a raw access/put count so a burst is not a flood.
pub fn burst_compress(count: u32) -> f64 {
    if count == 0 {
        return 0.0;
    }
    (f64::from(count)).log2()
}

/// `0.5.pow(age / half_life)` — 1.0 when fresh, 0.5 at three days.
pub fn decay(age_secs: u64) -> f64 {
    0.5_f64.powf(age_secs as f64 / HALF_LIFE_SECS as f64)
}

/// Decayed, burst-compressed frecency. Higher is hotter.
pub fn score(count: u32, age_secs: u64) -> f64 {
    burst_compress(count) * decay(age_secs)
}

/// Score a ref from the existing insertion/re-put log.
///
/// `count` is how many times the ref appears. Age is how many later events
/// sit after its last appearance, mapped through the same 3-day decay with
/// one later event treated as one second so the formula stays continuous
/// and FIFO is preserved when every ref appears once (oldest = coldest).
pub fn score_from_order(order: &[String], ref_id: &str) -> f64 {
    let mut count = 0u32;
    let mut last = 0usize;
    let mut seen = false;
    for (idx, id) in order.iter().enumerate() {
        if id == ref_id {
            count = count.saturating_add(1);
            last = idx;
            seen = true;
        }
    }
    if !seen {
        return 0.0;
    }
    let age_secs = order.len().saturating_sub(last.saturating_add(1)) as u64;
    score(count, age_secs)
}

/// Coldest live ref in `order`. Equal scores keep FIFO (first in `order`).
pub fn coldest<'a>(order: &'a [String], is_live: impl Fn(&str) -> bool) -> Option<&'a str> {
    let mut best: Option<(&str, f64, usize)> = None;
    for (idx, id) in order.iter().enumerate() {
        if !is_live(id) {
            continue;
        }
        let value = score_from_order(order, id);
        let replace = match best {
            None => true,
            Some((_, best_score, best_idx)) => {
                value < best_score || (value == best_score && idx < best_idx)
            }
        };
        if replace {
            best = Some((id.as_str(), value, idx));
        }
    }
    best.map(|(id, _, _)| id)
}
