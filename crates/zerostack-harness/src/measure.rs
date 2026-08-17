//! Shared timing helpers for honest microbenches.
//!
//! Six timing constants are hard (not flags). `measure_with_teardown`
//! captures `start.elapsed()` BEFORE `teardown()` runs.

use std::time::{Duration, Instant};

use serde::Serialize;

pub const WARMUP_ITERS: usize = 2;
pub const MIN_ITERS: usize = 3;
pub const MAX_ITERS: usize = 10;
pub const TARGET_DURATION: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Serialize)]
pub struct Measurement {
    pub label: String,
    pub iterations: usize,
    pub median_ms: f64,
    pub mean_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub stddev_ms: f64,
    /// Coefficient of variation in percent. `None` if mean is 0.
    pub cv_pct: Option<f64>,
}

impl Measurement {
    pub fn from_samples(label: &str, mut times: Vec<Duration>) -> Self {
        assert!(
            !times.is_empty(),
            "measure() requires at least one timed sample"
        );
        times.sort_unstable();
        let n = times.len();
        let millis: Vec<f64> = times.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
        let mean = millis.iter().sum::<f64>() / n as f64;
        let variance = if n > 1 {
            millis
                .iter()
                .map(|x| {
                    let d = x - mean;
                    d * d
                })
                .sum::<f64>()
                / (n as f64 - 1.0)
        } else {
            0.0
        };
        let stddev = variance.sqrt();
        let cv_pct = if mean > 0.0 {
            Some((stddev / mean) * 100.0)
        } else {
            None
        };
        Self {
            label: label.to_owned(),
            iterations: n,
            median_ms: percentile(&millis, 0.50),
            mean_ms: mean,
            p95_ms: percentile(&millis, 0.95),
            p99_ms: percentile(&millis, 0.99),
            stddev_ms: stddev,
            cv_pct,
        }
    }

    pub fn is_noise(&self) -> bool {
        match self.cv_pct {
            Some(cv) => cv > 5.0,
            None => true,
        }
    }
}

fn percentile(sorted_ms: &[f64], p: f64) -> f64 {
    if sorted_ms.len() == 1 {
        return sorted_ms[0];
    }
    let rank = p * (sorted_ms.len() as f64 - 1.0);
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        sorted_ms[lo]
    } else {
        let w = rank - lo as f64;
        sorted_ms[lo] * (1.0 - w) + sorted_ms[hi] * w
    }
}

pub fn measure<F>(label: &str, f: F) -> Measurement
where
    F: Fn(),
{
    for _ in 0..WARMUP_ITERS {
        f();
    }
    let start_total = Instant::now();
    let mut times = Vec::new();
    for iter in 0..MAX_ITERS {
        let start = Instant::now();
        f();
        let elapsed = start.elapsed();
        times.push(elapsed);
        if iter >= MIN_ITERS && start_total.elapsed() >= TARGET_DURATION {
            break;
        }
    }
    Measurement::from_samples(label, times)
}

/// Setup/teardown stay outside the timed window.
///
/// Warmup also calls teardown so measured iters see the same start state.
/// `start.elapsed()` is captured BEFORE `teardown()` runs.
pub fn measure_with_teardown<F, T>(label: &str, f: F, teardown: T) -> Measurement
where
    F: Fn(),
    T: Fn(),
{
    for _ in 0..WARMUP_ITERS {
        f();
        teardown();
    }
    let start_total = Instant::now();
    let mut times = Vec::new();
    for iter in 0..MAX_ITERS {
        let start = Instant::now();
        f();
        let elapsed = start.elapsed();
        times.push(elapsed);
        teardown();
        if iter >= MIN_ITERS && start_total.elapsed() >= TARGET_DURATION {
            break;
        }
    }
    Measurement::from_samples(label, times)
}

#[cfg(test)]
mod tests {
    use super::{Measurement, measure_with_teardown};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    #[test]
    fn teardown_is_outside_timed_window() {
        let timed = AtomicU64::new(0);
        let torn = AtomicU64::new(0);
        let measurement = measure_with_teardown(
            "teardown_discipline",
            || {
                timed.fetch_add(1, Ordering::Relaxed);
            },
            || {
                std::thread::sleep(Duration::from_millis(20));
                torn.fetch_add(1, Ordering::Relaxed);
            },
        );
        assert!(timed.load(Ordering::Relaxed) >= 5);
        assert!(torn.load(Ordering::Relaxed) >= 5);
        assert!(
            measurement.median_ms < 15.0,
            "teardown leaked into the timer: median_ms={}",
            measurement.median_ms
        );
    }

    #[test]
    fn from_samples_reports_cv_pct() {
        let times = vec![
            Duration::from_millis(10),
            Duration::from_millis(10),
            Duration::from_millis(10),
        ];
        let m = Measurement::from_samples("flat", times);
        assert_eq!(m.median_ms, 10.0);
        assert!(m.cv_pct.is_some());
        assert!(m.cv_pct.unwrap() < 1.0);
        assert!(!m.is_noise());
    }
}
