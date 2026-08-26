//! Wave 16 savings reports: exact numerator/denominator and deterministic
//! provenance, with Unknown vs Zero distinguished and incomparable pairs
//! rejected. Measurement-only, off the authority path.
//!
//! No ungrounded speed or ratio claim is ever produced. Savings are
//! always presented as exact integer numerator/denominator (tokens, bytes,
//! calls) plus the deterministic provenance root. A zero baseline yields
//! Unknown; a measured equal native/Zero usage yields Zero.

#![forbid(unsafe_code)]

use crate::observation::{MachineFingerprint, MeasuredUsage, TaskIdentity};
use crate::pair::{PairError, PairedObservations};
use crate::provenance;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

/// Wire schema for savings reports.
pub const SAVINGS_REPORT_SCHEMA: &str = "zerostack.zero_gauge.savings_report.v1";
pub const SAVINGS_REPORT_VERSION: u16 = 1;

/// Whether a savings measurement is claimable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SavingsStatus {
    /// No claim can be made for this pair. The `reason` names the typed
    /// condition (e.g. zero denominator, negative delta) and the
    /// `numerator`/`denominator` are the exact values that were observed.
    Unknown { reason: String },
    /// Valid measurement with zero savings (native == Zero). The
    /// numerator is zero and the denominator is the exact baseline.
    Zero,
    /// Valid measurement with positive savings. The numerator is
    /// `native - zero` and the denominator is the exact baseline.
    Positive,
}

/// One savings observation per unit dimension, with exact integers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitSavings {
    /// Exact baseline (native) count in the documented unit.
    pub baseline: u64,
    /// Exact Zero-path count in the same unit.
    pub zero: u64,
    /// Exact savings `baseline - zero` (saturates only when producing Unknown).
    pub numerator: u64,
    /// Exact baseline denominator (== `baseline` when claimable, else 0 with Unknown reason).
    pub denominator: u64,
    pub status: SavingsStatusForUnit,
}

/// Per-unit status flattened for stable provenance. The top-level
/// `SavingsStatus` is the aggregate over the three units; `UnitSavings`
/// carries the per-unit status so provenance remains deterministic even when
/// one dimension is Unknown while another is Positive.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SavingsStatusForUnit {
    Unknown { reason: String },
    Zero,
    Positive,
}

/// Adapter-stable, deterministic savings report. All fields participate in
/// the canonical provenance root except `provenance_root` itself (the root
/// is the hash of the canonical rendering without the root field).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavingsReport {
    pub schema: String,
    pub version: u16,
    pub task: TaskIdentity,
    pub machine: MachineFingerprint,
    pub native: MeasuredUsage,
    pub zero: MeasuredUsage,
    pub tokens: UnitSavings,
    pub bytes: UnitSavings,
    pub calls: UnitSavings,
    /// Aggregate status: Unknown if any unit is Unknown, else Positive if any
    /// unit is Positive, else Zero. Callers must inspect per-unit status for
    /// dimension-specific claims.
    pub status: SavingsStatus,
    /// Deterministic provenance root: SHA-256 hex over the canonical JSON
    /// rendering of this report with `provenance_root` set to empty string.
    pub provenance_root: String,
}

impl SavingsReport {
    /// Build a report from a comparable pair. Returns a `SavingsReport` for
    /// every comparable pair; the `status` distinguishes Unknown from Zero.
    /// Incomparable pairs never reach this method: `PairedObservations::new`
    /// already rejected them with `PairError`.
    pub fn from_pair(pair: &PairedObservations) -> Result<Self, ReportError> {
        let native = pair.native.usage;
        let zero = pair.zero.usage;
        let tokens = compute_unit(native.tokens, zero.tokens);
        let bytes = compute_unit(native.bytes, zero.bytes);
        let calls = compute_unit(native.calls, zero.calls);

        let status = aggregate_status(&tokens.status, &bytes.status, &calls.status);

        // Build the report without the root first, hash it, then attach.
        let mut report = Self {
            schema: SAVINGS_REPORT_SCHEMA.to_owned(),
            version: SAVINGS_REPORT_VERSION,
            task: pair.native.task.clone(),
            machine: pair.native.machine.clone(),
            native,
            zero,
            tokens,
            bytes,
            calls,
            status,
            provenance_root: String::new(),
        };
        let root = report.compute_root();
        report.provenance_root = root;
        report.validate()?;
        Ok(report)
    }

    fn compute_root(&self) -> String {
        // Hash a projection with empty provenance_root so the root is not self-referential.
        let mut projection = self.clone();
        projection.provenance_root = String::new();
        provenance::provenance_root(&projection)
    }

    /// Canonical rendering (sorted keys, deterministic) of this report.
    pub fn canonical_render(&self) -> String {
        provenance::canonical_render(self)
    }

    /// Verify the deterministic provenance root.
    pub fn verify_root(&self) -> Result<(), ReportError> {
        let expected = self.compute_root();
        if expected == self.provenance_root {
            Ok(())
        } else {
            Err(ReportError::ProvenanceMismatch {
                expected,
                actual: self.provenance_root.clone(),
            })
        }
    }

    pub fn validate(&self) -> Result<(), ReportError> {
        if self.schema != SAVINGS_REPORT_SCHEMA {
            return Err(ReportError::SchemaMismatch(self.schema.clone()));
        }
        if self.version != SAVINGS_REPORT_VERSION {
            return Err(ReportError::VersionMismatch(self.version));
        }
        self.task
            .validate()
            .map_err(ReportError::InvalidObservation)?;
        self.machine
            .validate()
            .map_err(ReportError::InvalidObservation)?;
        self.native
            .validate()
            .map_err(ReportError::InvalidObservation)?;
        self.zero
            .validate()
            .map_err(ReportError::InvalidObservation)?;
        validate_hex_root(&self.provenance_root)?;
        // Ensure per-unit statuses are consistent with computed values.
        check_unit_consistency(self.tokens.clone())?;
        check_unit_consistency(self.bytes.clone())?;
        check_unit_consistency(self.calls.clone())?;
        Ok(())
    }

    /// Exact token numerator (saved tokens) and denominator (baseline tokens).
    pub fn token_fraction(&self) -> (u64, u64) {
        (self.tokens.numerator, self.tokens.denominator)
    }

    /// Exact byte numerator and denominator.
    pub fn byte_fraction(&self) -> (u64, u64) {
        (self.bytes.numerator, self.bytes.denominator)
    }

    /// Exact call numerator and denominator.
    pub fn call_fraction(&self) -> (u64, u64) {
        (self.calls.numerator, self.calls.denominator)
    }
}

fn compute_unit(baseline: u64, zero: u64) -> UnitSavings {
    if baseline == 0 {
        return UnitSavings {
            baseline,
            zero,
            numerator: 0,
            denominator: 0,
            status: SavingsStatusForUnit::Unknown {
                reason: "zero baseline denominator".into(),
            },
        };
    }
    if zero > baseline {
        // Negative savings is not a claim; treat as Unknown with exact values preserved.
        return UnitSavings {
            baseline,
            zero,
            numerator: 0,
            denominator: baseline,
            status: SavingsStatusForUnit::Unknown {
                reason: "negative savings: zero exceeds baseline".into(),
            },
        };
    }
    let numerator = baseline - zero;
    let denominator = baseline;
    let status = if numerator == 0 {
        SavingsStatusForUnit::Zero
    } else {
        SavingsStatusForUnit::Positive
    };
    UnitSavings {
        baseline,
        zero,
        numerator,
        denominator,
        status,
    }
}

fn aggregate_status(
    tokens: &SavingsStatusForUnit,
    bytes: &SavingsStatusForUnit,
    calls: &SavingsStatusForUnit,
) -> SavingsStatus {
    let any_unknown = matches!(
        (tokens, bytes, calls),
        (SavingsStatusForUnit::Unknown { .. }, _, _)
            | (_, SavingsStatusForUnit::Unknown { .. }, _)
            | (_, _, SavingsStatusForUnit::Unknown { .. })
    );
    if any_unknown {
        // surface the first unknown reason
        let reason = match tokens {
            SavingsStatusForUnit::Unknown { reason } => reason.clone(),
            _ => match bytes {
                SavingsStatusForUnit::Unknown { reason } => reason.clone(),
                _ => match calls {
                    SavingsStatusForUnit::Unknown { reason } => reason.clone(),
                    _ => "unknown".into(),
                },
            },
        };
        return SavingsStatus::Unknown { reason };
    }
    let any_positive = matches!(
        (tokens, bytes, calls),
        (SavingsStatusForUnit::Positive, _, _)
            | (_, SavingsStatusForUnit::Positive, _)
            | (_, _, SavingsStatusForUnit::Positive)
    );
    if any_positive {
        return SavingsStatus::Positive;
    }
    SavingsStatus::Zero
}

fn check_unit_consistency(unit: UnitSavings) -> Result<(), ReportError> {
    match &unit.status {
        SavingsStatusForUnit::Unknown { .. } => Ok(()),
        SavingsStatusForUnit::Zero => {
            if unit.numerator != 0 {
                return Err(ReportError::InconsistentUnit {
                    baseline: unit.baseline,
                    zero: unit.zero,
                    numerator: unit.numerator,
                    denominator: unit.denominator,
                });
            }
            if unit.denominator != unit.baseline {
                return Err(ReportError::InconsistentUnit {
                    baseline: unit.baseline,
                    zero: unit.zero,
                    numerator: unit.numerator,
                    denominator: unit.denominator,
                });
            }
            if unit.zero != unit.baseline {
                return Err(ReportError::InconsistentUnit {
                    baseline: unit.baseline,
                    zero: unit.zero,
                    numerator: unit.numerator,
                    denominator: unit.denominator,
                });
            }
            Ok(())
        }
        SavingsStatusForUnit::Positive => {
            if unit.numerator == 0 {
                return Err(ReportError::InconsistentUnit {
                    baseline: unit.baseline,
                    zero: unit.zero,
                    numerator: unit.numerator,
                    denominator: unit.denominator,
                });
            }
            if unit.denominator != unit.baseline {
                return Err(ReportError::InconsistentUnit {
                    baseline: unit.baseline,
                    zero: unit.zero,
                    numerator: unit.numerator,
                    denominator: unit.denominator,
                });
            }
            if unit.baseline != unit.numerator + unit.zero {
                return Err(ReportError::InconsistentUnit {
                    baseline: unit.baseline,
                    zero: unit.zero,
                    numerator: unit.numerator,
                    denominator: unit.denominator,
                });
            }
            Ok(())
        }
    }
}

fn validate_hex_root(value: &str) -> Result<(), ReportError> {
    if value.len() != 64 {
        return Err(ReportError::InvalidProvenanceRoot);
    }
    if !value
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ReportError::InvalidProvenanceRoot);
    }
    Ok(())
}

/// Typed, fail-closed report construction or verification failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReportError {
    Pair(PairError),
    InvalidObservation(crate::observation::ObservationError),
    SchemaMismatch(String),
    VersionMismatch(u16),
    InvalidProvenanceRoot,
    ProvenanceMismatch {
        expected: String,
        actual: String,
    },
    InconsistentUnit {
        baseline: u64,
        zero: u64,
        numerator: u64,
        denominator: u64,
    },
}

impl fmt::Display for ReportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pair(e) => write!(f, "pair error: {e}"),
            Self::InvalidObservation(e) => write!(f, "invalid observation: {e}"),
            Self::SchemaMismatch(s) => write!(f, "unexpected schema {s:?}"),
            Self::VersionMismatch(v) => write!(f, "unexpected version {v}"),
            Self::InvalidProvenanceRoot => write!(f, "provenance root must be 64 lowercase hex"),
            Self::ProvenanceMismatch { expected, actual } => write!(
                f,
                "provenance root mismatch: expected {expected}, actual {actual}"
            ),
            Self::InconsistentUnit {
                baseline,
                zero,
                numerator,
                denominator,
            } => write!(
                f,
                "inconsistent unit: baseline {baseline} zero {zero} numerator {numerator} denominator {denominator}"
            ),
        }
    }
}

impl Error for ReportError {}

impl From<PairError> for ReportError {
    fn from(err: PairError) -> Self {
        Self::Pair(err)
    }
}
