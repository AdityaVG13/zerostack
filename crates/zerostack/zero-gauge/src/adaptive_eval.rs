//! Adaptive validation task selection for efficient Zero optimization.

use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq)]
pub struct TaskHistory {
    pub id: String,
    pub successes: u64,
    pub trials: u64,
}

impl TaskHistory {
    pub fn success_rate(&self) -> f64 {
        if self.trials == 0 {
            0.0
        } else {
            self.successes.min(self.trials) as f64 / self.trials as f64
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveEvalConfig {
    pub never_solved_floor: f64,
    pub uncertainty_bonus: f64,
    pub monte_carlo_rounds: u32,
}

impl Default for AdaptiveEvalConfig {
    fn default() -> Self {
        Self {
            never_solved_floor: 0.125,
            uncertainty_bonus: 0.025,
            monte_carlo_rounds: 4_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskSelection {
    pub indexes: Vec<usize>,
    pub inclusion_probabilities: Vec<f64>,
}

pub fn task_weight(history: &TaskHistory, config: AdaptiveEvalConfig) -> f64 {
    let p = history.success_rate();
    let floor = if history.successes == 0 {
        config.never_solved_floor
    } else {
        0.0
    };
    let uncertainty = config.uncertainty_bonus / (history.trials.max(1) as f64).sqrt();
    (p * (1.0 - p)).max(floor) + uncertainty
}

pub fn select_tasks(
    histories: &[TaskHistory],
    count: usize,
    seed: u64,
    config: AdaptiveEvalConfig,
) -> Result<TaskSelection, String> {
    if histories.is_empty() || count == 0 || count > histories.len() {
        return Err("adaptive evaluation count must be within the task pool".into());
    }
    if config.monte_carlo_rounds == 0
        || !config.never_solved_floor.is_finite()
        || !config.uncertainty_bonus.is_finite()
        || config.never_solved_floor < 0.0
        || config.uncertainty_bonus < 0.0
    {
        return Err("adaptive evaluation configuration is invalid".into());
    }
    let mut ids = BTreeSet::new();
    for history in histories {
        if history.id.is_empty() || history.successes > history.trials || !ids.insert(&history.id) {
            return Err(
                "adaptive evaluation histories must have unique ids and valid outcomes".into(),
            );
        }
    }
    let weights: Vec<f64> = histories
        .iter()
        .map(|history| task_weight(history, config))
        .collect();
    let indexes = weighted_sample(&weights, count, seed);
    let mut inclusions = vec![0_u32; histories.len()];
    let mut rng = SplitMix64::new(seed ^ 0x9e37_79b9_7f4a_7c15);
    for _ in 0..config.monte_carlo_rounds {
        for index in weighted_sample(&weights, count, rng.next_u64()) {
            inclusions[index] = inclusions[index].saturating_add(1);
        }
    }
    let rounds = f64::from(config.monte_carlo_rounds);
    let inclusion_probabilities = inclusions
        .into_iter()
        .map(|included| (f64::from(included) / rounds).max(1.0 / rounds))
        .collect();
    Ok(TaskSelection {
        indexes,
        inclusion_probabilities,
    })
}

pub fn hajek_estimate(samples: &[(f64, f64)]) -> Result<f64, String> {
    let mut weighted = 0.0;
    let mut mass = 0.0;
    for &(outcome, inclusion) in samples {
        validate_sample(outcome, inclusion)?;
        weighted += outcome / inclusion;
        mass += 1.0 / inclusion;
    }
    if mass == 0.0 {
        return Err("Hajek estimate requires samples".into());
    }
    Ok(weighted / mass)
}

pub fn anchored_difference_estimate(
    anchors: &[f64],
    samples: &[(usize, f64, f64)],
) -> Result<f64, String> {
    if anchors.is_empty() || anchors.iter().any(|value| !(0.0..=1.0).contains(value)) {
        return Err("anchored estimate requires valid task anchors".into());
    }
    let mut estimate = anchors.iter().sum::<f64>() / anchors.len() as f64;
    for &(index, outcome, inclusion) in samples {
        validate_sample(outcome, inclusion)?;
        let anchor = *anchors
            .get(index)
            .ok_or("sample index is outside anchor pool")?;
        estimate += (outcome - anchor) / inclusion / anchors.len() as f64;
    }
    Ok(estimate.clamp(0.0, 1.0))
}

fn validate_sample(outcome: f64, inclusion: f64) -> Result<(), String> {
    if !(0.0..=1.0).contains(&outcome) || !(0.0..=1.0).contains(&inclusion) || inclusion == 0.0 {
        return Err("sample outcomes and inclusion probabilities must be within bounds".into());
    }
    Ok(())
}

fn weighted_sample(weights: &[f64], count: usize, seed: u64) -> Vec<usize> {
    let mut rng = SplitMix64::new(seed);
    let mut keys: Vec<(f64, usize)> = weights
        .iter()
        .enumerate()
        .map(|(index, weight)| {
            let u = rng.next_f64().max(f64::MIN_POSITIVE);
            (u.ln() / weight.max(f64::MIN_POSITIVE), index)
        })
        .collect();
    keys.sort_by(|(left_key, left_index), (right_key, right_index)| {
        right_key
            .total_cmp(left_key)
            .then_with(|| left_index.cmp(right_index))
    });
    let mut selected: Vec<usize> = keys
        .into_iter()
        .take(count)
        .map(|(_, index)| index)
        .collect();
    selected.sort_unstable();
    selected
}

struct SplitMix64(u64);
impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / ((1_u64 << 53) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontier_tasks_receive_more_sampling_mass() {
        let histories = vec![
            TaskHistory {
                id: "easy".into(),
                successes: 10,
                trials: 10,
            },
            TaskHistory {
                id: "frontier".into(),
                successes: 5,
                trials: 10,
            },
            TaskHistory {
                id: "hard".into(),
                successes: 0,
                trials: 10,
            },
        ];
        let selection = select_tasks(&histories, 1, 7, AdaptiveEvalConfig::default()).unwrap();
        assert!(selection.inclusion_probabilities[1] > selection.inclusion_probabilities[0]);
        assert!(selection.inclusion_probabilities[1] > selection.inclusion_probabilities[2]);
    }

    #[test]
    fn estimators_preserve_constant_outcomes() {
        assert!((hajek_estimate(&[(0.5, 0.2), (0.5, 0.8)]).unwrap() - 0.5).abs() < 1e-9);
        let estimate = anchored_difference_estimate(&[0.5, 0.5], &[(0, 0.5, 0.5)]).unwrap();
        assert!((estimate - 0.5).abs() < 1e-9);
    }
}
