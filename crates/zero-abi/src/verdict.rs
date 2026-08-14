//! Shared trivalent epistemic verdict (ZS-KERNEL-004).
//!
//! `SafetyVerdictV1` is the single fail-closed truth value shared by engines:
//! `Safe`, `Unsafe`, or `Unknown`. The lattice law is fixed and total:
//!
//! ```text
//! Unsafe  dominates  Unknown  dominates  Safe
//! ```
//!
//! Promotion law: **nothing in this module ever upgrades `Unknown` or
//! `Unsafe` toward `Safe`.** `Unknown` always requires the frozen raw-baseline
//! fallback at the caller; it is never laundered into authority. `Safe` is
//! only ever produced by every required premise being positively established
//! (`from_premises` with all `Some(true)`), never by absence of evidence.

use std::{collections::BTreeSet, error::Error, fmt};

use serde::{Deserialize, Serialize};

pub const VERDICT_MAX_PREMISE_NAME_BYTES: usize = 256;

/// One required premise of a protected result.
///
/// `established` is trivalent on purpose: `Some(true)` means the premise was
/// positively established, `Some(false)` means it was positively falsified,
/// and `None` means the premise was missing or never evaluated. A missing
/// premise is `Unknown`, never silently treated as true.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PremiseV1 {
    pub name: String,
    pub established: Option<bool>,
}

impl PremiseV1 {
    /// Fail-closed construction: premise names must be nonempty, at most
    /// 256 bytes, and free of control characters.
    pub fn new(
        name: impl Into<String>,
        established: Option<bool>,
    ) -> Result<Self, VerdictBuildErrorV1> {
        let premise = Self {
            name: name.into(),
            established,
        };
        premise.validate()?;
        Ok(premise)
    }

    pub fn validate(&self) -> Result<(), VerdictBuildErrorV1> {
        if self.name.is_empty() {
            return Err(VerdictBuildErrorV1::EmptyName);
        }
        if self.name.len() > VERDICT_MAX_PREMISE_NAME_BYTES {
            return Err(VerdictBuildErrorV1::NameTooLong {
                actual: self.name.len(),
                maximum: VERDICT_MAX_PREMISE_NAME_BYTES,
            });
        }
        if self.name.chars().any(char::is_control) {
            return Err(VerdictBuildErrorV1::ControlCharacter);
        }
        Ok(())
    }
}

/// Fail-closed error for premise construction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerdictBuildErrorV1 {
    EmptyName,
    NameTooLong { actual: usize, maximum: usize },
    ControlCharacter,
}

impl fmt::Display for VerdictBuildErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => write!(formatter, "premise name must be nonempty"),
            Self::NameTooLong { actual, maximum } => {
                write!(formatter, "premise name is {actual} bytes, maximum {maximum}")
            }
            Self::ControlCharacter => write!(formatter, "premise name must be free of control characters"),
        }
    }
}

impl Error for VerdictBuildErrorV1 {}

/// Shared trivalent epistemic verdict.
///
/// Wire shape (snake_case): `"safe"`, `"unsafe"` with `reasons`, `"unknown"`
/// with `reasons`. Reasons are sorted and deduplicated.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SafetyVerdictV1 {
    Safe,
    Unsafe { reasons: Vec<String> },
    Unknown { reasons: Vec<String> },
}

impl SafetyVerdictV1 {
    /// The lattice meet: `Unsafe` dominates `Unknown` dominates `Safe`.
    ///
    /// Reasons from both sides are concatenated, deduplicated, and sorted.
    /// The meet is commutative, associative, and idempotent; `Safe` is the
    /// identity. A vacuous empty meet is `Safe` by lattice identity -- the
    /// vacuity danger lives in [`SafetyVerdictV1::from_premises`], which
    /// fails closed on an empty premise set.
    pub fn meet(self, other: SafetyVerdictV1) -> SafetyVerdictV1 {
        use SafetyVerdictV1::{Safe, Unknown, Unsafe};
        match (self, other) {
            (Safe, Safe) => Safe,
            (Unsafe { reasons: left }, Unsafe { reasons: right }) => {
                Unsafe { reasons: merge_reasons(left, right) }
            }
            (Unsafe { reasons }, _) | (_, Unsafe { reasons }) => {
                Unsafe { reasons: sort_dedup(reasons) }
            }
            (Unknown { reasons: left }, Unknown { reasons: right }) => {
                Unknown { reasons: merge_reasons(left, right) }
            }
            (Unknown { reasons }, Safe) | (Safe, Unknown { reasons }) => {
                Unknown { reasons: sort_dedup(reasons) }
            }
        }
    }

    /// Fold a sequence of verdicts under the lattice meet. Equivalent to
    /// repeated [`SafetyVerdictV1::meet`] with `Safe` as the starting value.
    pub fn meet_all(iter: impl IntoIterator<Item = SafetyVerdictV1>) -> SafetyVerdictV1 {
        iter.into_iter()
            .fold(SafetyVerdictV1::Safe, SafetyVerdictV1::meet)
    }

    /// Evaluate a premise set into one verdict.
    ///
    /// Fail-closed law: an empty premise set is `Unknown { reasons:
    /// ["no_premises"] }`, never vacuously `Safe`. A premise with
    /// `established: Some(true)` contributes `Safe`; `Some(false)` contributes
    /// `Unsafe`; `None` (missing or unevaluated) contributes `Unknown`. The
    /// contributions are folded under the lattice meet, so one falsified
    /// premise poisons the whole result, and one missing premise downgrades
    /// `Safe` to `Unknown` but never to `Unsafe`.
    pub fn from_premises(premises: &[PremiseV1]) -> SafetyVerdictV1 {
        if premises.is_empty() {
            return SafetyVerdictV1::Unknown {
                reasons: vec!["no_premises".into()],
            };
        }
        SafetyVerdictV1::meet_all(premises.iter().map(|premise| match premise.established {
            Some(true) => SafetyVerdictV1::Safe,
            Some(false) => SafetyVerdictV1::Unsafe {
                reasons: vec![premise.name.clone()],
            },
            None => SafetyVerdictV1::Unknown {
                reasons: vec![premise.name.clone()],
            },
        }))
    }

    /// Whether this verdict grants operational authority. Only `Safe` does;
    /// `Unsafe` and `Unknown` never do.
    pub fn grants_authority(&self) -> bool {
        matches!(self, SafetyVerdictV1::Safe)
    }

    /// Reasons carried by this verdict (empty for `Safe`).
    pub fn reasons(&self) -> &[String] {
        match self {
            SafetyVerdictV1::Safe => &[],
            SafetyVerdictV1::Unsafe { reasons } | SafetyVerdictV1::Unknown { reasons } => reasons,
        }
    }

    /// Stable short label: `safe`, `unsafe`, or `unknown`.
    pub fn label(&self) -> &'static str {
        match self {
            SafetyVerdictV1::Safe => "safe",
            SafetyVerdictV1::Unsafe { .. } => "unsafe",
            SafetyVerdictV1::Unknown { .. } => "unknown",
        }
    }
}

fn sort_dedup(mut reasons: Vec<String>) -> Vec<String> {
    reasons.sort();
    reasons.dedup();
    reasons
}

fn merge_reasons(mut left: Vec<String>, right: Vec<String>) -> Vec<String> {
    left.extend(right);
    let deduped: BTreeSet<String> = left.into_iter().collect();
    deduped.into_iter().collect()
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-abi/unit/verdict.rs"]
mod tests;
