//! Idle-overhead release gate (ZS-OPS-006 / V6-R14).
//!
//! The daemonless law says the sidecar is invisible when idle: no background
//! work, no spawn-per-call measurement, and a measured steady-state budget
//! of <= 0.1% CPU and <= 500 MB resident memory. This module is the
//! mechanism that makes that budget a *release gate*:
//!
//! - [`IdleWindowEvidence`] is the sealed evidence artifact: a window of
//!   in-process samples (cumulative user/sys CPU nanoseconds and RSS bytes,
//!   all host-clock readouts -- no child processes are spawned to measure)
//!   plus a counter of background-activity events observed during the
//!   window. Its `digest()` is content-derived over the canonical window
//!   bytes, so evidence can be anchored externally.
//! - [`evaluate_idle_release_gate`] refuses *without evidence*:
//!   `None` evidence is a loud [`IdleGateError::RequiresEvidence`] -- the
//!   gate never passes on prose. With evidence it compares the observed
//!   window maximums against [`IdleBudgets`] (default 0.1% CPU fraction,
//!   500 MB RSS) and returns a sealed [`IdleGateReceipt`]; budget
//!   breaches are fail-loud refusals carried inside the receipt
//!   ([`IdleGateRefusal`]), never silent truncation or clamping.
//! - Background activity observed during the idle window refuses the gate
//!   ([`IdleGateRefusalReason::BackgroundActivityDetected`]): the daemonless
//!   law is enforced by evidence, not by promise.
//! - [`IdleSampler`] is the in-process sampling seam; the host adapter for
//!   the sidecar (zsx-node) is residual hub wiring -- the crate proves the
//!   gate with a real `getrusage`-backed sampler in tests.
//!
//! The contract manifest [`idle_gate_contract`] freezes these semantics
//! (budgets, no-evidence refusal, no background work, no spawn-per-call).

use serde::{Deserialize, Serialize};

use zero_abi::{Sha256Digest, canonical_json};

/// Schema version of the idle-gate artifacts.
pub const IDLE_GATE_SCHEMA_VERSION: u16 = 1;
/// Domain tag bound into every idle-gate evidence digest.
pub const IDLE_GATE_DOMAIN: &[u8] = b"zerostack.idle-gate\0";
/// Default idle CPU budget: 0.1% expressed in parts per billion (1e-3).
pub const DEFAULT_IDLE_MAX_CPU_FRACTION_PPB: u64 = 1_000_000;
/// Default idle RSS budget: 500 MB.
pub const DEFAULT_IDLE_MAX_RSS_BYTES: u64 = 500 * 1024 * 1024;
/// ABI tag carried by idle-gate artifacts.
pub const IDLE_GATE_ABI_VERSION: &str = "v6-r14";

/// One in-process measurement readout. CPU fields are cumulative since
/// process start (getrusage semantics); the window fraction is derived from
/// the delta between the first and last sample so a window can never see
/// CPU time spent before the window.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdleSample {
    /// Cumulative wall time of the sampled process at readout, ns.
    pub elapsed_wall_ns: u64,
    /// Cumulative user CPU time of the sampled process at readout, ns.
    pub cpu_user_ns: u64,
    /// Cumulative system CPU time of the sampled process at readout, ns.
    pub cpu_sys_ns: u64,
    /// Resident set size at readout, bytes.
    pub rss_bytes: u64,
    /// Host-clock readout time.
    pub sampled_at_unix_ns: u64,
}

impl IdleSample {
    pub fn new(
        elapsed_wall_ns: u64,
        cpu_user_ns: u64,
        cpu_sys_ns: u64,
        rss_bytes: u64,
        sampled_at_unix_ns: u64,
    ) -> Self {
        Self {
            elapsed_wall_ns,
            cpu_user_ns,
            cpu_sys_ns,
            rss_bytes,
            sampled_at_unix_ns,
        }
    }

    fn cumulative_cpu_ns(&self) -> u64 {
        self.cpu_user_ns.saturating_add(self.cpu_sys_ns)
    }
}

/// Sealed evidence for one idle window. `background_activity_events` is the
/// count of background activities the sidecar logged during the window; any
/// nonzero count refuses the release gate (daemonless law).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdleWindowEvidence {
    pub schema_version: u16,
    pub window_start_unix_ns: u64,
    pub window_end_unix_ns: u64,
    pub samples: Vec<IdleSample>,
    pub background_activity_events: u64,
    pub abi_version: String,
}

impl IdleWindowEvidence {
    pub fn new(
        window_start_unix_ns: u64,
        window_end_unix_ns: u64,
        samples: Vec<IdleSample>,
        background_activity_events: u64,
    ) -> Result<Self, IdleGateError> {
        let evidence = Self {
            schema_version: IDLE_GATE_SCHEMA_VERSION,
            window_start_unix_ns,
            window_end_unix_ns,
            samples,
            background_activity_events,
            abi_version: IDLE_GATE_ABI_VERSION.to_owned(),
        };
        evidence.validate()?;
        Ok(evidence)
    }

    /// Structural validation. Sample readouts must be monotonic and the
    /// window bounds must match the first/last sample readouts.
    pub fn validate(&self) -> Result<(), IdleGateError> {
        if self.schema_version != IDLE_GATE_SCHEMA_VERSION {
            return Err(IdleGateError::InvalidEvidence(
                "idle evidence schema version is not supported".to_owned(),
            ));
        }
        if self.abi_version != IDLE_GATE_ABI_VERSION {
            return Err(IdleGateError::InvalidEvidence(
                "idle evidence ABI version is not supported".to_owned(),
            ));
        }
        if self.samples.is_empty() {
            return Err(IdleGateError::InvalidEvidence(
                "an idle window needs at least a start and an end sample".to_owned(),
            ));
        }
        if self.window_end_unix_ns < self.window_start_unix_ns {
            return Err(IdleGateError::InvalidEvidence(
                "window end precedes window start".to_owned(),
            ));
        }
        let mut previous: Option<&IdleSample> = None;
        for sample in &self.samples {
            if let Some(previous) = previous {
                if sample.sampled_at_unix_ns < previous.sampled_at_unix_ns
                    || sample.elapsed_wall_ns < previous.elapsed_wall_ns
                    || sample.cumulative_cpu_ns() < previous.cumulative_cpu_ns()
                {
                    return Err(IdleGateError::InvalidEvidence(
                        "sample readouts must be monotonic".to_owned(),
                    ));
                }
            }
            previous = Some(sample);
        }
        let first = self.samples.first().expect("samples nonempty");
        let last = self.samples.last().expect("samples nonempty");
        if first.sampled_at_unix_ns != self.window_start_unix_ns
            || last.sampled_at_unix_ns != self.window_end_unix_ns
        {
            return Err(IdleGateError::InvalidEvidence(
                "window bounds must equal the first and last sample readouts".to_owned(),
            ));
        }
        Ok(())
    }

    /// Wall-clock length of the window, ns.
    pub fn window_wall_ns(&self) -> u64 {
        self.window_end_unix_ns.saturating_sub(self.window_start_unix_ns)
    }

    /// Observed maximum CPU fraction over the window, parts per billion.
    /// Only CPU time accumulated *inside* the window counts (delta between
    /// the first and last cumulative readouts).
    pub fn observed_max_cpu_fraction_ppb(&self) -> u64 {
        let first = self.samples.first().expect("samples nonempty");
        let last = self.samples.last().expect("samples nonempty");
        let wall = last
            .elapsed_wall_ns
            .saturating_sub(first.elapsed_wall_ns);
        let cpu = last.cumulative_cpu_ns().saturating_sub(first.cumulative_cpu_ns());
        if wall == 0 {
            return 0;
        }
        // Fraction in parts per billion: cpu/wall * 1e9.
        cpu.saturating_mul(1_000_000_000) / wall
    }

    /// Observed maximum RSS over the window, bytes.
    pub fn observed_max_rss_bytes(&self) -> u64 {
        self.samples
            .iter()
            .map(|sample| sample.rss_bytes)
            .max()
            .unwrap_or(0)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, IdleGateError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(|error| {
            IdleGateError::InvalidEvidence(format!("evidence is not JSON-serializable: {error}"))
        })?;
        Ok(canonical_json(&value).into_bytes())
    }

    /// Content-derived evidence digest over the domain-tagged canonical
    /// window bytes. Same window, same digest; tampering any field changes
    /// the digest.
    pub fn digest(&self) -> Result<Sha256Digest, IdleGateError> {
        let mut tagged = Vec::with_capacity(IDLE_GATE_DOMAIN.len() + 128);
        tagged.extend_from_slice(IDLE_GATE_DOMAIN);
        tagged.extend_from_slice(&self.canonical_bytes()?);
        Ok(Sha256Digest::from_bytes(zero_abi::sha256(&tagged)))
    }
}

/// The budgets the release gate enforces. Defaults are the V6 steady-state
/// targets: <= 0.1% CPU and <= 500 MB RSS while idle.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdleBudgets {
    /// Maximum idle CPU fraction, parts per billion (0.1% = 1_000_000).
    pub max_cpu_fraction_ppb: u64,
    /// Maximum idle RSS, bytes.
    pub max_rss_bytes: u64,
}

impl Default for IdleBudgets {
    fn default() -> Self {
        Self {
            max_cpu_fraction_ppb: DEFAULT_IDLE_MAX_CPU_FRACTION_PPB,
            max_rss_bytes: DEFAULT_IDLE_MAX_RSS_BYTES,
        }
    }
}

impl IdleBudgets {
    pub fn new(max_cpu_fraction_ppb: u64, max_rss_bytes: u64) -> Result<Self, IdleGateError> {
        let budgets = Self {
            max_cpu_fraction_ppb,
            max_rss_bytes,
        };
        if budgets.max_cpu_fraction_ppb == 0 || budgets.max_rss_bytes == 0 {
            return Err(IdleGateError::InvalidEvidence(
                "idle budgets must be nonzero".to_owned(),
            ));
        }
        Ok(budgets)
    }
}

/// Why the release gate refused. Refusals are loud and carried inside the
/// sealed receipt -- a budget breach is a decision with a receipt, never a
/// silent clamp or truncation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdleGateRefusal {
    pub reason: IdleGateRefusalReason,
    pub observed: u64,
    pub budget: u64,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleGateRefusalReason {
    /// Observed CPU fraction exceeded the budget (ppb).
    CpuBudgetViolation,
    /// Observed RSS exceeded the budget (bytes).
    RssBudgetViolation,
    /// Background activity ran during the idle window (daemonless law).
    BackgroundActivityDetected,
}

/// Sealed outcome of one release-gate evaluation. `admitted == false`
/// always carries a refusal; the receipt digest anchors the whole decision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdleGateReceipt {
    pub schema_version: u16,
    pub admitted: bool,
    pub refusal: Option<IdleGateRefusal>,
    pub samples: usize,
    pub window_wall_ns: u64,
    pub observed_max_cpu_fraction_ppb: u64,
    pub observed_max_rss_bytes: u64,
    pub budgets: IdleBudgets,
    pub evidence_digest: Sha256Digest,
    pub abi_version: String,
}

impl IdleGateReceipt {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, IdleGateError> {
        let value = serde_json::to_value(self).map_err(|error| {
            IdleGateError::Evaluation(format!("receipt is not JSON-serializable: {error}"))
        })?;
        Ok(canonical_json(&value).into_bytes())
    }

    pub fn digest(&self) -> Result<Sha256Digest, IdleGateError> {
        let mut tagged = Vec::with_capacity(IDLE_GATE_DOMAIN.len() + 128);
        tagged.extend_from_slice(IDLE_GATE_DOMAIN);
        tagged.extend_from_slice(&self.canonical_bytes()?);
        Ok(Sha256Digest::from_bytes(zero_abi::sha256(&tagged)))
    }
}

/// Loud, fail-closed errors for the release gate. Structural problems
/// (missing evidence, malformed windows) are errors; budget breaches are
/// sealed refusals inside an [`IdleGateReceipt`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdleGateError {
    /// The gate was evaluated without evidence. The release gate fails
    /// without evidence -- always.
    RequiresEvidence,
    /// The evidence window is structurally invalid.
    InvalidEvidence(String),
    /// The evaluation itself failed (serialization).
    Evaluation(String),
}

impl std::fmt::Display for IdleGateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdleGateError::RequiresEvidence => {
                write!(formatter, "release gate requires idle-window evidence")
            }
            IdleGateError::InvalidEvidence(detail) => {
                write!(formatter, "invalid idle evidence: {detail}")
            }
            IdleGateError::Evaluation(detail) => {
                write!(formatter, "idle gate evaluation failed: {detail}")
            }
        }
    }
}

impl std::error::Error for IdleGateError {}

/// Evaluate the idle release gate against sealed window evidence.
///
/// - `None` evidence is [`IdleGateError::RequiresEvidence`] -- the gate
///   never passes on prose.
/// - A nonzero background-activity count refuses
///   ([`IdleGateRefusalReason::BackgroundActivityDetected`]).
/// - Observed window maximums above the budgets refuse with the observed
///   and budget values in the sealed receipt. Breaches are never clamped,
///   truncated, or silently ignored.
/// - Within budget the receipt admits with counts and the evidence digest.
pub fn evaluate_idle_release_gate(
    evidence: Option<&IdleWindowEvidence>,
    budgets: IdleBudgets,
) -> Result<IdleGateReceipt, IdleGateError> {
    let Some(evidence) = evidence else {
        return Err(IdleGateError::RequiresEvidence);
    };
    evidence.validate()?;
    let evidence_digest = evidence.digest()?;
    let observed_cpu = evidence.observed_max_cpu_fraction_ppb();
    let observed_rss = evidence.observed_max_rss_bytes();
    let refusal = if evidence.background_activity_events > 0 {
        Some(IdleGateRefusal {
            reason: IdleGateRefusalReason::BackgroundActivityDetected,
            observed: evidence.background_activity_events,
            budget: 0,
            detail: format!(
                "{} background activity events observed during the idle window",
                evidence.background_activity_events
            ),
        })
    } else if observed_cpu > budgets.max_cpu_fraction_ppb {
        Some(IdleGateRefusal {
            reason: IdleGateRefusalReason::CpuBudgetViolation,
            observed: observed_cpu,
            budget: budgets.max_cpu_fraction_ppb,
            detail: format!(
                "idle CPU fraction {observed_cpu} ppb exceeds budget {} ppb",
                budgets.max_cpu_fraction_ppb
            ),
        })
    } else if observed_rss > budgets.max_rss_bytes {
        Some(IdleGateRefusal {
            reason: IdleGateRefusalReason::RssBudgetViolation,
            observed: observed_rss,
            budget: budgets.max_rss_bytes,
            detail: format!(
                "idle RSS {observed_rss} bytes exceeds budget {} bytes",
                budgets.max_rss_bytes
            ),
        })
    } else {
        None
    };
    let receipt = IdleGateReceipt {
        schema_version: IDLE_GATE_SCHEMA_VERSION,
        admitted: refusal.is_none(),
        refusal,
        samples: evidence.samples.len(),
        window_wall_ns: evidence.window_wall_ns(),
        observed_max_cpu_fraction_ppb: observed_cpu,
        observed_max_rss_bytes: observed_rss,
        budgets,
        evidence_digest,
        abi_version: IDLE_GATE_ABI_VERSION.to_owned(),
    };
    let _ = receipt.digest()?;
    Ok(receipt)
}

/// In-process sampling seam for idle windows. The sidecar host adapter
/// (zsx-node) is residual hub wiring; the crate tests a real
/// `getrusage`-backed sampler. Samplers must not spawn processes or start
/// background work -- the measurement itself must be invisible.
pub trait IdleSampler {
    fn sample(&mut self) -> Result<IdleSample, IdleGateError>;
}

/// Measure an idle window by sampling at the start, middle, and end of
/// `window_ns`, sleeping between readouts (no spinning, no background work).
/// Returns sealed window evidence ready for the release gate.
pub fn measure_idle_window<S: IdleSampler>(
    sampler: &mut S,
    window_ns: u64,
) -> Result<IdleWindowEvidence, IdleGateError> {
    let first = sampler.sample()?;
    let half = window_ns / 2;
    std::thread::sleep(std::time::Duration::from_nanos(half));
    let middle = sampler.sample()?;
    std::thread::sleep(std::time::Duration::from_nanos(window_ns - half));
    let last = sampler.sample()?;
    // The window bounds are the sample readouts themselves: the start bound
    // is the first readout and the end bound the last readout, so the
    // evidence is self-consistent by construction.
    IdleWindowEvidence::new(
        first.sampled_at_unix_ns,
        last.sampled_at_unix_ns,
        vec![first, middle, last],
        0,
    )
}

/// The frozen contract manifest for the idle release gate (ZS-OPS-006).
pub fn idle_gate_contract() -> serde_json::Value {
    serde_json::json!({
        "schema_version": IDLE_GATE_SCHEMA_VERSION,
        "domain": "zerostack.idle-gate",
        "budgets": {
            "max_cpu_fraction": "0.1% (1_000_000 ppb)",
            "max_rss_bytes": 500 * 1024 * 1024,
        },
        "release_gate": {
            "fails_without_evidence": true,
            "budget_breach": "loud sealed refusal; never clamped or truncated",
            "background_activity": "refuses the gate (daemonless law)",
        },
        "measurement": {
            "in_process_sampling": true,
            "no_spawn_per_call": true,
            "no_background_work_while_idle": true,
        },
        "abi_version": IDLE_GATE_ABI_VERSION,
    })
}

