//! Exact fixed-point multiplier and prepared-coverage calculations. A
//! verified Hundredfold claim binds ledger roots, recomputes the
//! reported ratio, and rejects protected regressions or weak sliding windows.

use serde::{Deserialize, Serialize};

pub const PPM: u128 = 1_000_000;
pub const HUNDREDFOLD: u128 = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MultiplierCoordinate {
    Visible,
    Billed,
    PlanCapacity,
    CompleteWork,
    SlidingWindow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    Verified,
    Observed,
    Estimated,
    Rejected,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MultiplierReport {
    pub coordinate: MultiplierCoordinate,
    pub baseline_usage: u64,
    pub optimized_usage: u64,
    pub multiplier_ppm: u64,
    pub task_scope_root: String,
    pub quality_ok: bool,
    pub protected_regressions: u32,
    pub ledger_root: String,
    pub status: ReportStatus,
    pub window_minimum_ppm: Option<u64>,
}

impl MultiplierReport {
    pub fn validate(&self) -> Result<(), String> {
        let valid_root = |root: &str| {
            root.len() == 64
                && root
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        };
        if !valid_root(&self.task_scope_root) || !valid_root(&self.ledger_root) {
            return Err("multiplier report carries an invalid root".into());
        }
        if self.multiplier_ppm != multiplier_ppm(self.baseline_usage, self.optimized_usage)? {
            return Err("reported multiplier does not match measured usage".into());
        }
        if self.coordinate == MultiplierCoordinate::SlidingWindow
            && self.window_minimum_ppm.is_none()
        {
            return Err("sliding-window report requires a window minimum".into());
        }
        Ok(())
    }

    pub fn verified_hundredfold(&self) -> bool {
        self.validate().is_ok()
            && self.status == ReportStatus::Verified
            && self.quality_ok
            && self.protected_regressions == 0
            && u128::from(self.multiplier_ppm) >= HUNDREDFOLD * PPM
            && self
                .window_minimum_ppm
                .is_none_or(|minimum| u128::from(minimum) >= HUNDREDFOLD * PPM)
    }
}

pub fn multiplier_ppm(baseline: u64, optimized: u64) -> Result<u64, String> {
    if baseline == 0 || optimized == 0 {
        return Err("multiplier inputs must be positive".into());
    }
    let scaled = u128::from(baseline)
        .checked_mul(PPM)
        .ok_or("multiplier overflow")?
        / u128::from(optimized);
    u64::try_from(scaled).map_err(|_| "multiplier does not fit u64".into())
}

pub fn required_prepared_coverage_ppm(
    baseline: u64,
    preparation: u64,
    hit: u64,
    fallback: u64,
    target_multiplier: u64,
) -> Result<u64, String> {
    if baseline == 0 || target_multiplier == 0 || fallback <= hit {
        return Err(
            "prepared coverage requires positive baseline/target and fallback > hit".into(),
        );
    }
    let target = u128::from(baseline) * PPM / u128::from(target_multiplier);
    let numerator = (u128::from(preparation) + u128::from(fallback))
        .saturating_mul(PPM)
        .saturating_sub(target);
    let denominator = u128::from(fallback - hit);
    let coverage = numerator.div_ceil(denominator);
    if coverage > PPM {
        return Err("target is infeasible even at full prepared coverage".into());
    }
    Ok(coverage as u64)
}

pub fn coverage_chain_ppm(gates: &[u64]) -> Result<u64, String> {
    let mut product = PPM;
    for &gate in gates {
        if u128::from(gate) > PPM {
            return Err("coverage gate exceeds one million ppm".into());
        }
        product = product
            .checked_mul(u128::from(gate))
            .ok_or("coverage product overflow")?
            .div_ceil(PPM);
    }
    Ok(product as u64)
}

pub fn minimum_window_multiplier_ppm(
    baseline: &[u64],
    optimized: &[u64],
    window: usize,
) -> Result<u64, String> {
    if baseline.len() != optimized.len()
        || baseline.is_empty()
        || window == 0
        || window > baseline.len()
    {
        return Err("invalid sliding-window inputs".into());
    }
    let mut minimum = u64::MAX;
    for start in 0..=baseline.len() - window {
        let base = baseline[start..start + window]
            .iter()
            .try_fold(0_u64, |sum, value| sum.checked_add(*value))
            .ok_or("baseline window overflow")?;
        let zero = optimized[start..start + window]
            .iter()
            .try_fold(0_u64, |sum, value| sum.checked_add(*value))
            .ok_or("optimized window overflow")?;
        minimum = minimum.min(multiplier_ppm(base, zero)?);
    }
    Ok(minimum)
}
