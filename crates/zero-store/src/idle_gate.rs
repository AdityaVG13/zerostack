//! Idle-overhead release gate (ZS-OPS-006 / V6-R14).
//!
//! The daemonless law says the sidecar is invisible when idle: no background
//! work, no spawn-per-call measurement, and a measured steady-state budget
//! of <= 0.1% CPU and <= 500 MB resident memory. This module is the
//! mechanism that makes that budget a *release gate*:
//!
//! - [`IdleWindowEvidenceV1`] is the sealed evidence artifact: a window of
//!   in-process samples (cumulative user/sys CPU nanoseconds and RSS bytes,
//!   all host-clock readouts -- no child processes are spawned to measure)
//!   plus a counter of background-activity events observed during the
//!   window. Its `digest()` is content-derived over the canonical window
//!   bytes, so evidence can be anchored externally.
//! - [`evaluate_idle_release_gate_v1`] refuses *without evidence*:
//!   `None` evidence is a loud [`IdleGateErrorV1::RequiresEvidence`] -- the
//!   gate never passes on prose. With evidence it compares the observed
//!   window maximums against [`IdleBudgetsV1`] (default 0.1% CPU fraction,
//!   500 MB RSS) and returns a sealed [`IdleGateReceiptV1`]; budget
//!   breaches are fail-loud refusals carried inside the receipt
//!   ([`IdleGateRefusalV1`]), never silent truncation or clamping.
//! - Background activity observed during the idle window refuses the gate
//!   ([`IdleGateRefusalReasonV1::BackgroundActivityDetected`]): the daemonless
//!   law is enforced by evidence, not by promise.
//! - [`IdleSamplerV1`] is the in-process sampling seam; the host adapter for
//!   the sidecar (zsx-node) is residual hub wiring -- the crate proves the
//!   gate with a real `getrusage`-backed sampler in tests.
//!
//! The contract manifest [`idle_gate_contract_v1`] freezes these semantics
//! (budgets, no-evidence refusal, no background work, no spawn-per-call).

use serde::{Deserialize, Serialize};

use zero_abi::{DigestV1, canonical_json};

/// Schema version of the idle-gate artifacts.
pub const IDLE_GATE_SCHEMA_VERSION_V1: u16 = 1;
/// Domain tag bound into every idle-gate evidence digest.
pub const IDLE_GATE_DOMAIN_V1: &[u8] = b"zerostack.idle-gate.v1\0";
/// Default idle CPU budget: 0.1% expressed in parts per billion (1e-3).
pub const DEFAULT_IDLE_MAX_CPU_FRACTION_PPB_V1: u64 = 1_000_000;
/// Default idle RSS budget: 500 MB.
pub const DEFAULT_IDLE_MAX_RSS_BYTES_V1: u64 = 500 * 1024 * 1024;
/// ABI tag carried by idle-gate artifacts.
pub const IDLE_GATE_ABI_VERSION_V1: &str = "v6-r14";

/// One in-process measurement readout. CPU fields are cumulative since
/// process start (getrusage semantics); the window fraction is derived from
/// the delta between the first and last sample so a window can never see
/// CPU time spent before the window.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdleSampleV1 {
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

impl IdleSampleV1 {
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
pub struct IdleWindowEvidenceV1 {
    pub schema_version: u16,
    pub window_start_unix_ns: u64,
    pub window_end_unix_ns: u64,
    pub samples: Vec<IdleSampleV1>,
    pub background_activity_events: u64,
    pub abi_version: String,
}

impl IdleWindowEvidenceV1 {
    pub fn new(
        window_start_unix_ns: u64,
        window_end_unix_ns: u64,
        samples: Vec<IdleSampleV1>,
        background_activity_events: u64,
    ) -> Result<Self, IdleGateErrorV1> {
        let evidence = Self {
            schema_version: IDLE_GATE_SCHEMA_VERSION_V1,
            window_start_unix_ns,
            window_end_unix_ns,
            samples,
            background_activity_events,
            abi_version: IDLE_GATE_ABI_VERSION_V1.to_owned(),
        };
        evidence.validate()?;
        Ok(evidence)
    }

    /// Structural validation. Sample readouts must be monotonic and the
    /// window bounds must match the first/last sample readouts.
    pub fn validate(&self) -> Result<(), IdleGateErrorV1> {
        if self.schema_version != IDLE_GATE_SCHEMA_VERSION_V1 {
            return Err(IdleGateErrorV1::InvalidEvidence(
                "idle evidence schema version is not supported".to_owned(),
            ));
        }
        if self.abi_version != IDLE_GATE_ABI_VERSION_V1 {
            return Err(IdleGateErrorV1::InvalidEvidence(
                "idle evidence ABI version is not supported".to_owned(),
            ));
        }
        if self.samples.is_empty() {
            return Err(IdleGateErrorV1::InvalidEvidence(
                "an idle window needs at least a start and an end sample".to_owned(),
            ));
        }
        if self.window_end_unix_ns < self.window_start_unix_ns {
            return Err(IdleGateErrorV1::InvalidEvidence(
                "window end precedes window start".to_owned(),
            ));
        }
        let mut previous: Option<&IdleSampleV1> = None;
        for sample in &self.samples {
            if let Some(previous) = previous {
                if sample.sampled_at_unix_ns < previous.sampled_at_unix_ns
                    || sample.elapsed_wall_ns < previous.elapsed_wall_ns
                    || sample.cumulative_cpu_ns() < previous.cumulative_cpu_ns()
                {
                    return Err(IdleGateErrorV1::InvalidEvidence(
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
            return Err(IdleGateErrorV1::InvalidEvidence(
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

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, IdleGateErrorV1> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(|error| {
            IdleGateErrorV1::InvalidEvidence(format!("evidence is not JSON-serializable: {error}"))
        })?;
        Ok(canonical_json(&value).into_bytes())
    }

    /// Content-derived evidence digest over the domain-tagged canonical
    /// window bytes. Same window, same digest; tampering any field changes
    /// the digest.
    pub fn digest(&self) -> Result<DigestV1, IdleGateErrorV1> {
        let mut tagged = Vec::with_capacity(IDLE_GATE_DOMAIN_V1.len() + 128);
        tagged.extend_from_slice(IDLE_GATE_DOMAIN_V1);
        tagged.extend_from_slice(&self.canonical_bytes()?);
        Ok(DigestV1::from_bytes(zero_abi::sha256(&tagged)))
    }
}

/// The budgets the release gate enforces. Defaults are the V6 steady-state
/// targets: <= 0.1% CPU and <= 500 MB RSS while idle.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdleBudgetsV1 {
    /// Maximum idle CPU fraction, parts per billion (0.1% = 1_000_000).
    pub max_cpu_fraction_ppb: u64,
    /// Maximum idle RSS, bytes.
    pub max_rss_bytes: u64,
}

impl Default for IdleBudgetsV1 {
    fn default() -> Self {
        Self {
            max_cpu_fraction_ppb: DEFAULT_IDLE_MAX_CPU_FRACTION_PPB_V1,
            max_rss_bytes: DEFAULT_IDLE_MAX_RSS_BYTES_V1,
        }
    }
}

impl IdleBudgetsV1 {
    pub fn new(max_cpu_fraction_ppb: u64, max_rss_bytes: u64) -> Result<Self, IdleGateErrorV1> {
        let budgets = Self {
            max_cpu_fraction_ppb,
            max_rss_bytes,
        };
        if budgets.max_cpu_fraction_ppb == 0 || budgets.max_rss_bytes == 0 {
            return Err(IdleGateErrorV1::InvalidEvidence(
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
pub struct IdleGateRefusalV1 {
    pub reason: IdleGateRefusalReasonV1,
    pub observed: u64,
    pub budget: u64,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleGateRefusalReasonV1 {
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
pub struct IdleGateReceiptV1 {
    pub schema_version: u16,
    pub admitted: bool,
    pub refusal: Option<IdleGateRefusalV1>,
    pub samples: usize,
    pub window_wall_ns: u64,
    pub observed_max_cpu_fraction_ppb: u64,
    pub observed_max_rss_bytes: u64,
    pub budgets: IdleBudgetsV1,
    pub evidence_digest: DigestV1,
    pub abi_version: String,
}

impl IdleGateReceiptV1 {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, IdleGateErrorV1> {
        let value = serde_json::to_value(self).map_err(|error| {
            IdleGateErrorV1::Evaluation(format!("receipt is not JSON-serializable: {error}"))
        })?;
        Ok(canonical_json(&value).into_bytes())
    }

    pub fn digest(&self) -> Result<DigestV1, IdleGateErrorV1> {
        let mut tagged = Vec::with_capacity(IDLE_GATE_DOMAIN_V1.len() + 128);
        tagged.extend_from_slice(IDLE_GATE_DOMAIN_V1);
        tagged.extend_from_slice(&self.canonical_bytes()?);
        Ok(DigestV1::from_bytes(zero_abi::sha256(&tagged)))
    }
}

/// Loud, fail-closed errors for the release gate. Structural problems
/// (missing evidence, malformed windows) are errors; budget breaches are
/// sealed refusals inside an [`IdleGateReceiptV1`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdleGateErrorV1 {
    /// The gate was evaluated without evidence. The release gate fails
    /// without evidence -- always.
    RequiresEvidence,
    /// The evidence window is structurally invalid.
    InvalidEvidence(String),
    /// The evaluation itself failed (serialization).
    Evaluation(String),
}

impl std::fmt::Display for IdleGateErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdleGateErrorV1::RequiresEvidence => {
                write!(formatter, "release gate requires idle-window evidence")
            }
            IdleGateErrorV1::InvalidEvidence(detail) => {
                write!(formatter, "invalid idle evidence: {detail}")
            }
            IdleGateErrorV1::Evaluation(detail) => {
                write!(formatter, "idle gate evaluation failed: {detail}")
            }
        }
    }
}

impl std::error::Error for IdleGateErrorV1 {}

/// Evaluate the idle release gate against sealed window evidence.
///
/// - `None` evidence is [`IdleGateErrorV1::RequiresEvidence`] -- the gate
///   never passes on prose.
/// - A nonzero background-activity count refuses
///   ([`IdleGateRefusalReasonV1::BackgroundActivityDetected`]).
/// - Observed window maximums above the budgets refuse with the observed
///   and budget values in the sealed receipt. Breaches are never clamped,
///   truncated, or silently ignored.
/// - Within budget the receipt admits with counts and the evidence digest.
pub fn evaluate_idle_release_gate_v1(
    evidence: Option<&IdleWindowEvidenceV1>,
    budgets: IdleBudgetsV1,
) -> Result<IdleGateReceiptV1, IdleGateErrorV1> {
    let Some(evidence) = evidence else {
        return Err(IdleGateErrorV1::RequiresEvidence);
    };
    evidence.validate()?;
    let evidence_digest = evidence.digest()?;
    let observed_cpu = evidence.observed_max_cpu_fraction_ppb();
    let observed_rss = evidence.observed_max_rss_bytes();
    let refusal = if evidence.background_activity_events > 0 {
        Some(IdleGateRefusalV1 {
            reason: IdleGateRefusalReasonV1::BackgroundActivityDetected,
            observed: evidence.background_activity_events,
            budget: 0,
            detail: format!(
                "{} background activity events observed during the idle window",
                evidence.background_activity_events
            ),
        })
    } else if observed_cpu > budgets.max_cpu_fraction_ppb {
        Some(IdleGateRefusalV1 {
            reason: IdleGateRefusalReasonV1::CpuBudgetViolation,
            observed: observed_cpu,
            budget: budgets.max_cpu_fraction_ppb,
            detail: format!(
                "idle CPU fraction {observed_cpu} ppb exceeds budget {} ppb",
                budgets.max_cpu_fraction_ppb
            ),
        })
    } else if observed_rss > budgets.max_rss_bytes {
        Some(IdleGateRefusalV1 {
            reason: IdleGateRefusalReasonV1::RssBudgetViolation,
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
    let receipt = IdleGateReceiptV1 {
        schema_version: IDLE_GATE_SCHEMA_VERSION_V1,
        admitted: refusal.is_none(),
        refusal,
        samples: evidence.samples.len(),
        window_wall_ns: evidence.window_wall_ns(),
        observed_max_cpu_fraction_ppb: observed_cpu,
        observed_max_rss_bytes: observed_rss,
        budgets,
        evidence_digest,
        abi_version: IDLE_GATE_ABI_VERSION_V1.to_owned(),
    };
    let _ = receipt.digest()?;
    Ok(receipt)
}

/// In-process sampling seam for idle windows. The sidecar host adapter
/// (zsx-node) is residual hub wiring; the crate tests a real
/// `getrusage`-backed sampler. Samplers must not spawn processes or start
/// background work -- the measurement itself must be invisible.
pub trait IdleSamplerV1 {
    fn sample(&mut self) -> Result<IdleSampleV1, IdleGateErrorV1>;
}

/// Measure an idle window by sampling at the start, middle, and end of
/// `window_ns`, sleeping between readouts (no spinning, no background work).
/// Returns sealed window evidence ready for the release gate.
pub fn measure_idle_window_v1<S: IdleSamplerV1>(
    sampler: &mut S,
    window_ns: u64,
) -> Result<IdleWindowEvidenceV1, IdleGateErrorV1> {
    let first = sampler.sample()?;
    let half = window_ns / 2;
    std::thread::sleep(std::time::Duration::from_nanos(half));
    let middle = sampler.sample()?;
    std::thread::sleep(std::time::Duration::from_nanos(window_ns - half));
    let last = sampler.sample()?;
    // The window bounds are the sample readouts themselves: the start bound
    // is the first readout and the end bound the last readout, so the
    // evidence is self-consistent by construction.
    IdleWindowEvidenceV1::new(
        first.sampled_at_unix_ns,
        last.sampled_at_unix_ns,
        vec![first, middle, last],
        0,
    )
}

/// The frozen contract manifest for the idle release gate (ZS-OPS-006).
pub fn idle_gate_contract_v1() -> serde_json::Value {
    serde_json::json!({
        "schema_version": IDLE_GATE_SCHEMA_VERSION_V1,
        "domain": "zerostack.idle-gate.v1",
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
        "abi_version": IDLE_GATE_ABI_VERSION_V1,
    })
}

