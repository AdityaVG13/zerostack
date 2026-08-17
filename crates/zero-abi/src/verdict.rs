//! Shared trivalent epistemic verdict (ZS-KERNEL-004).
//!
//! `SafetyVerdict` is the single fail-closed truth value shared by engines:
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
pub struct Premise {
    pub name: String,
    pub established: Option<bool>,
}

impl Premise {
    /// Fail-closed construction: premise names must be nonempty, at most
    /// 256 bytes, and free of control characters.
    pub fn new(
        name: impl Into<String>,
        established: Option<bool>,
    ) -> Result<Self, VerdictBuildError> {
        let premise = Self {
            name: name.into(),
            established,
        };
        premise.validate()?;
        Ok(premise)
    }

    pub fn validate(&self) -> Result<(), VerdictBuildError> {
        if self.name.is_empty() {
            return Err(VerdictBuildError::EmptyName);
        }
        if self.name.len() > VERDICT_MAX_PREMISE_NAME_BYTES {
            return Err(VerdictBuildError::NameTooLong {
                actual: self.name.len(),
                maximum: VERDICT_MAX_PREMISE_NAME_BYTES,
            });
        }
        if self.name.chars().any(char::is_control) {
            return Err(VerdictBuildError::ControlCharacter);
        }
        Ok(())
    }
}

/// Fail-closed error for premise construction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerdictBuildError {
    EmptyName,
    NameTooLong { actual: usize, maximum: usize },
    ControlCharacter,
}

impl fmt::Display for VerdictBuildError {
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

impl Error for VerdictBuildError {}

/// Shared trivalent epistemic verdict.
///
/// Wire shape (snake_case): `"safe"`, `"unsafe"` with `reasons`, `"unknown"`
/// with `reasons`. Reasons are sorted and deduplicated.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SafetyVerdict {
    Safe,
    Unsafe { reasons: Vec<String> },
    Unknown { reasons: Vec<String> },
}

impl SafetyVerdict {
    /// The lattice meet: `Unsafe` dominates `Unknown` dominates `Safe`.
    ///
    /// Reasons from both sides are concatenated, deduplicated, and sorted.
    /// The meet is commutative, associative, and idempotent; `Safe` is the
    /// identity. A vacuous empty meet is `Safe` by lattice identity -- the
    /// vacuity danger lives in [`SafetyVerdict::from_premises`], which
    /// fails closed on an empty premise set.
    pub fn meet(self, other: SafetyVerdict) -> SafetyVerdict {
        use SafetyVerdict::{Safe, Unknown, Unsafe};
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
    /// repeated [`SafetyVerdict::meet`] with `Safe` as the starting value.
    pub fn meet_all(iter: impl IntoIterator<Item = SafetyVerdict>) -> SafetyVerdict {
        iter.into_iter()
            .fold(SafetyVerdict::Safe, SafetyVerdict::meet)
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
    pub fn from_premises(premises: &[Premise]) -> SafetyVerdict {
        if premises.is_empty() {
            return SafetyVerdict::Unknown {
                reasons: vec!["no_premises".into()],
            };
        }
        SafetyVerdict::meet_all(premises.iter().map(|premise| match premise.established {
            Some(true) => SafetyVerdict::Safe,
            Some(false) => SafetyVerdict::Unsafe {
                reasons: vec![premise.name.clone()],
            },
            None => SafetyVerdict::Unknown {
                reasons: vec![premise.name.clone()],
            },
        }))
    }

    /// Whether this verdict grants operational authority. Only `Safe` does;
    /// `Unsafe` and `Unknown` never do.
    pub fn grants_authority(&self) -> bool {
        matches!(self, SafetyVerdict::Safe)
    }

    /// Reasons carried by this verdict (empty for `Safe`).
    pub fn reasons(&self) -> &[String] {
        match self {
            SafetyVerdict::Safe => &[],
            SafetyVerdict::Unsafe { reasons } | SafetyVerdict::Unknown { reasons } => reasons,
        }
    }

    /// Stable short label: `safe`, `unsafe`, or `unknown`.
    pub fn label(&self) -> &'static str {
        match self {
            SafetyVerdict::Safe => "safe",
            SafetyVerdict::Unsafe { .. } => "unsafe",
            SafetyVerdict::Unknown { .. } => "unknown",
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

