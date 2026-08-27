//! Beta posterior + distribution-free conformal band for release scoring.
//!
//! Release decisions use the conformal LOWER bound, never the point estimate.
//! A 100% raw pass rate with small N cannot certify: uniform prior Beta(1,1)
//! plus the (1-α)/2 quantile keep the lower bound strictly below 1.0 for
//! finite trials. `truncate_score` is applied only at the output boundary.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;

use crate::parity_taxonomy::truncate_score;

pub const SCORECARD_SCHEMA: &str = "gauntlet.parity_scorecard.v1";
pub const DEFAULT_CONFIDENCE: f64 = 0.95;
pub const UNIFORM_ALPHA_PRIOR: f64 = 1.0;
pub const UNIFORM_BETA_PRIOR: f64 = 1.0;
/// Need ≥2 held-out residuals before the conformal band is calibrated.
pub const MIN_CALIBRATION_RESIDUALS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BetaParams {
    pub alpha: f64,
    pub beta: f64,
}

impl BetaParams {
    pub const UNIFORM_PRIOR: Self = Self {
        alpha: UNIFORM_ALPHA_PRIOR,
        beta: UNIFORM_BETA_PRIOR,
    };

    pub fn posterior(self, successes: f64, failures: f64) -> Self {
        let successes = successes.max(0.0);
        let failures = failures.max(0.0);
        Self {
            alpha: self.alpha + successes,
            beta: self.beta + failures,
        }
    }

    pub fn from_passes_trials(passes: f64, trials: f64) -> Self {
        let passes = passes.max(0.0);
        let trials = trials.max(passes);
        Self::UNIFORM_PRIOR.posterior(passes, trials - passes)
    }

    pub fn mean(self) -> f64 {
        let ab = self.alpha + self.beta;
        if !self.is_valid() || ab == 0.0 {
            return 0.5;
        }
        self.alpha / ab
    }

    pub fn variance(self) -> f64 {
        let ab = self.alpha + self.beta;
        if !self.is_valid() || ab == 0.0 {
            return 0.0;
        }
        (self.alpha * self.beta) / (ab * ab * (ab + 1.0))
    }

    pub fn is_valid(self) -> bool {
        self.alpha.is_finite() && self.beta.is_finite() && self.alpha > 0.0 && self.beta > 0.0
    }

    /// Regularized incomplete-beta CDF F(x) = I_x(α, β).
    pub fn cdf(self, x: f64) -> f64 {
        if !self.is_valid() {
            return 0.0;
        }
        regularized_incomplete_beta(x, self.alpha, self.beta)
    }

    pub fn quantile(self, p: f64) -> f64 {
        if !self.is_valid() {
            return 0.0;
        }
        if !p.is_finite() || p <= 0.0 {
            return 0.0;
        }
        if p >= 1.0 {
            return 1.0;
        }
        // Closed forms used as exact checks; general path is bisection on CDF.
        if (self.beta - 1.0).abs() < 1e-15 {
            return p.powf(1.0 / self.alpha).clamp(0.0, 1.0);
        }
        if (self.alpha - 1.0).abs() < 1e-15 {
            return (1.0 - (1.0 - p).powf(1.0 / self.beta)).clamp(0.0, 1.0);
        }
        let mut lo = 0.0;
        let mut hi = 1.0;
        for _ in 0..80 {
            let mid = 0.5 * (lo + hi);
            if self.cdf(mid) < p {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        (0.5 * (lo + hi)).clamp(0.0, 1.0)
    }

    pub fn credible_interval(self, confidence: f64) -> ConformalInterval {
        let confidence = sanitize_confidence(confidence);
        let tail = (1.0 - confidence) / 2.0;
        ConformalInterval {
            point: self.mean(),
            lower: self.quantile(tail),
            upper: self.quantile(1.0 - tail),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ConformalInterval {
    pub point: f64,
    pub lower: f64,
    pub upper: f64,
}

impl ConformalInterval {
    pub fn truncated(self) -> Self {
        Self {
            point: truncate_score(self.point),
            lower: truncate_score(self.lower.clamp(0.0, 1.0)),
            upper: truncate_score(self.upper.clamp(0.0, 1.0)),
        }
    }
}

/// Input: passes/trials (Partial already converted to 0.5). Output is truncated.
pub fn score_passes_trials(passes: f64, trials: f64, confidence: f64) -> ConformalInterval {
    BetaParams::from_passes_trials(passes, trials)
        .credible_interval(confidence)
        .truncated()
}

/// Vovk–Gammerman–Shafer residual quantile: index ⌈(n+1)·confidence⌉ − 1.
pub fn residual_quantile(residuals: &[f64], confidence: f64) -> Option<f64> {
    if residuals.len() < MIN_CALIBRATION_RESIDUALS {
        return None;
    }
    let confidence = sanitize_confidence(confidence);
    let mut ordered = residuals.to_vec();
    ordered.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let n = ordered.len();
    let rank = ((n as f64 + 1.0) * confidence).ceil() as usize;
    let idx = rank.saturating_sub(1).min(n - 1);
    Some(ordered[idx])
}

/// Wrap a Bayesian interval in a distribution-free band when residuals exist.
/// Combined lower = min(Bayesian lower, point − q). Fewer than 2 residuals
/// keeps the bootstrap Bayesian interval (Phase 9 first baseline).
pub fn apply_conformal_residuals(
    interval: ConformalInterval,
    residuals: &[f64],
    confidence: f64,
) -> (ConformalInterval, ConformalStatus, Option<f64>) {
    match residual_quantile(residuals, confidence) {
        None => (
            interval.truncated(),
            ConformalStatus::BootstrapBayesian,
            None,
        ),
        Some(q) => {
            let conf_lo = (interval.point - q).max(0.0);
            let conf_hi = (interval.point + q).min(1.0);
            let combined = ConformalInterval {
                point: interval.point,
                lower: interval.lower.min(conf_lo),
                upper: interval.upper.max(conf_hi),
            }
            .truncated();
            (
                combined,
                ConformalStatus::Calibrated,
                Some(truncate_score(q)),
            )
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConformalStatus {
    BootstrapBayesian,
    Calibrated,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CategoryEvidence {
    pub category: String,
    pub weight: f64,
    pub successes: f64,
    pub failures: f64,
}

impl CategoryEvidence {
    pub fn trials(&self) -> f64 {
        (self.successes + self.failures).max(0.0)
    }

    pub fn raw_rate(&self) -> f64 {
        let n = self.trials();
        if n <= 0.0 {
            return 0.0;
        }
        (self.successes / n).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CategoryScore {
    pub category: String,
    pub weight: f64,
    pub successes: f64,
    pub failures: f64,
    pub trials: f64,
    pub alpha: f64,
    pub beta: f64,
    pub raw_rate: f64,
    pub point: f64,
    pub lower: f64,
    pub upper: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParityScorecard {
    pub schema_version: String,
    pub origin: String,
    pub confidence: f64,
    pub alpha_prior: f64,
    pub beta_prior: f64,
    pub conformal_status: ConformalStatus,
    pub calibration_count: usize,
    pub residual_quantile: Option<f64>,
    pub categories: BTreeMap<String, CategoryScore>,
    pub observation_count: f64,
    pub global_raw: f64,
    pub global_point: f64,
    pub global_lower: f64,
    pub global_upper: f64,
    /// Load-bearing: release must name this field, not `global_point`.
    pub release_uses: &'static str,
    pub point_estimate_as_bound: &'static str,
}

impl Default for ParityScorecard {
    fn default() -> Self {
        Self {
            schema_version: SCORECARD_SCHEMA.to_string(),
            origin: String::new(),
            confidence: DEFAULT_CONFIDENCE,
            alpha_prior: UNIFORM_ALPHA_PRIOR,
            beta_prior: UNIFORM_BETA_PRIOR,
            conformal_status: ConformalStatus::BootstrapBayesian,
            calibration_count: 0,
            residual_quantile: None,
            categories: BTreeMap::new(),
            observation_count: 0.0,
            global_raw: 0.0,
            global_point: 0.0,
            global_lower: 0.0,
            global_upper: 1.0,
            release_uses: "conformal_lower",
            point_estimate_as_bound: "fail_closed",
        }
    }
}

impl ParityScorecard {
    pub fn with_origin(mut self, origin: impl Into<String>) -> Self {
        self.origin = origin.into();
        self
    }

    /// Fail-closed unless `reported_bound` is the conformal lower bound.
    /// Passing the point estimate or the raw rate is a Block, even at 1.0.
    pub fn release_decision(&self, reported_bound: f64, threshold: f64) -> ReleaseVerdict {
        if !self.global_lower.is_finite()
            || !self.global_point.is_finite()
            || self.global_lower > self.global_point + 1e-12
        {
            return ReleaseVerdict::Block(ReleaseBlock::InvariantBroken);
        }
        if self.observation_count <= 0.0 {
            return ReleaseVerdict::Block(ReleaseBlock::NoEvidence);
        }
        let reported = truncate_score(reported_bound);
        if reported != self.global_lower {
            return ReleaseVerdict::Block(ReleaseBlock::PointEstimateUsedAsBound);
        }
        if self.global_lower < truncate_score(threshold) {
            return ReleaseVerdict::Block(ReleaseBlock::LowerBoundBelowThreshold);
        }
        ReleaseVerdict::Allow
    }

    pub fn conformal_certifiable(&self, threshold: f64) -> bool {
        matches!(
            self.release_decision(self.global_lower, threshold),
            ReleaseVerdict::Allow
        )
    }

    /// Using the point estimate as if it were the bound is always false.
    pub fn release_on_point_estimate(&self, threshold: f64) -> bool {
        release_pass_on_point_estimate(self.global_point, threshold)
    }
}

/// Fail-closed: a pass predicate that treats the point estimate as the
/// release bound never succeeds, including a 100% raw rate with small N.
pub fn release_pass_on_point_estimate(_point: f64, _threshold: f64) -> bool {
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseVerdict {
    Allow,
    Block(ReleaseBlock),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseBlock {
    PointEstimateUsedAsBound,
    LowerBoundBelowThreshold,
    NoEvidence,
    InvariantBroken,
}

impl fmt::Display for ReleaseVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allow => f.write_str("allow"),
            Self::Block(b) => write!(f, "block:{b}"),
        }
    }
}

impl fmt::Display for ReleaseBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PointEstimateUsedAsBound => "point_estimate_used_as_bound",
            Self::LowerBoundBelowThreshold => "lower_bound_below_threshold",
            Self::NoEvidence => "no_evidence",
            Self::InvariantBroken => "invariant_broken",
        })
    }
}

pub fn score_categories(
    evidence: &[CategoryEvidence],
    prior: BetaParams,
    confidence: f64,
    residuals: &[f64],
) -> ParityScorecard {
    let confidence = sanitize_confidence(confidence);
    let mut card = ParityScorecard {
        confidence,
        alpha_prior: prior.alpha,
        beta_prior: prior.beta,
        ..ParityScorecard::default()
    };
    if evidence.is_empty() {
        return card;
    }

    let mut weight_sum = 0.0;
    let mut global_raw = 0.0;
    let mut global_point = 0.0;
    let mut global_bayes_lower = 0.0;
    let mut global_bayes_upper = 0.0;
    let mut observations = 0.0;

    for ev in evidence {
        if !ev.weight.is_finite() || ev.weight <= 0.0 {
            continue;
        }
        let posterior = prior.posterior(ev.successes, ev.failures);
        let interval = posterior.credible_interval(confidence).truncated();
        let trials = truncate_score(ev.trials());
        observations += ev.trials();
        weight_sum += ev.weight;
        global_raw += ev.weight * ev.raw_rate();
        global_point += ev.weight * interval.point;
        global_bayes_lower += ev.weight * interval.lower;
        global_bayes_upper += ev.weight * interval.upper;
        card.categories.insert(
            ev.category.clone(),
            CategoryScore {
                category: ev.category.clone(),
                weight: ev.weight,
                successes: truncate_score(ev.successes),
                failures: truncate_score(ev.failures),
                trials,
                alpha: posterior.alpha,
                beta: posterior.beta,
                raw_rate: truncate_score(ev.raw_rate()),
                point: interval.point,
                lower: interval.lower,
                upper: interval.upper,
            },
        );
    }

    if weight_sum <= 0.0 {
        return card;
    }

    let bayesian = ConformalInterval {
        point: global_point / weight_sum,
        lower: global_bayes_lower / weight_sum,
        upper: global_bayes_upper / weight_sum,
    };
    let (combined, status, q) = apply_conformal_residuals(bayesian, residuals, confidence);
    card.conformal_status = status;
    card.calibration_count = residuals.len();
    card.residual_quantile = q;
    card.observation_count = truncate_score(observations);
    card.global_raw = truncate_score(global_raw / weight_sum);
    card.global_point = combined.point;
    card.global_lower = combined.lower;
    card.global_upper = combined.upper;
    card
}

fn sanitize_confidence(confidence: f64) -> f64 {
    if confidence.is_finite() && confidence > 0.0 && confidence < 1.0 {
        confidence
    } else {
        DEFAULT_CONFIDENCE
    }
}

/// Lanczos approximation for ln Γ(z), z > 0. Reflection for z ∈ (0, 0.5).
fn ln_gamma(z: f64) -> f64 {
    const G: f64 = 7.0;
    const P: [f64; 9] = [
        0.999_999_999_999_809_93,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_13,
        -176.615_029_162_140_59,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_571_6e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if z <= 0.0 || !z.is_finite() {
        return f64::NAN;
    }
    if z < 0.5 {
        return std::f64::consts::PI.ln()
            - (std::f64::consts::PI * z).sin().ln()
            - ln_gamma(1.0 - z);
    }
    let zm1 = z - 1.0;
    let mut x = P[0];
    for (i, coeff) in P.iter().enumerate().skip(1) {
        x += coeff / (zm1 + i as f64);
    }
    let t = zm1 + G + 0.5;
    0.5 * (2.0 * std::f64::consts::PI).ln() + (zm1 + 0.5) * t.ln() - t + x.ln()
}

fn ln_beta(a: f64, b: f64) -> f64 {
    ln_gamma(a) + ln_gamma(b) - ln_gamma(a + b)
}

fn betacf(a: f64, b: f64, x: f64) -> f64 {
    const MAX_ITER: usize = 200;
    const EPS: f64 = 3.0e-16;
    const FPMIN: f64 = 1.0e-300;
    let qab = a + b;
    let qap = a + 1.0;
    let qam = a - 1.0;
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < FPMIN {
        d = FPMIN;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..=MAX_ITER {
        let m_f = m as f64;
        let m2 = (2 * m) as f64;
        let aa = m_f * (b - m_f) * x / ((qam + m2) * (a + m2));
        d = 1.0 + aa * d;
        if d.abs() < FPMIN {
            d = FPMIN;
        }
        c = 1.0 + aa / c;
        if c.abs() < FPMIN {
            c = FPMIN;
        }
        d = 1.0 / d;
        h *= d * c;
        let aa = -(a + m_f) * (qab + m_f) * x / ((a + m2) * (qap + m2));
        d = 1.0 + aa * d;
        if d.abs() < FPMIN {
            d = FPMIN;
        }
        c = 1.0 + aa / c;
        if c.abs() < FPMIN {
            c = FPMIN;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < EPS {
            return h;
        }
    }
    h
}

fn regularized_incomplete_beta(x: f64, a: f64, b: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    if a <= 0.0 || b <= 0.0 || !a.is_finite() || !b.is_finite() {
        return 0.0;
    }
    let lbeta = ln_beta(a, b);
    let front = a * x.ln() + b * (1.0 - x).ln() - lbeta;
    if x < (a + 1.0) / (a + b + 2.0) {
        (front - a.ln() + betacf(a, b, x).ln())
            .exp()
            .clamp(0.0, 1.0)
    } else {
        (1.0 - (front - b.ln() + betacf(b, a, 1.0 - x).ln()).exp()).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
#[path = "../../../../tests/tokenzero/unit/tokenzero-test-support/conformal_tests.rs"]
mod tests;
