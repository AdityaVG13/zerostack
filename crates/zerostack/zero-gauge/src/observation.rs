//! Wave 16 token-savings measurement observations around the direct Zero path.
//!
//! Measurement-only: this module is never on the authority path. It records
//! native baseline versus model-visible Zero usage with exact units, task
//! identity, and machine fingerprint. It performs no engine work, admits no
//! I/O, and never influences dispatch or transaction decisions.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

/// Exact unit labels. Counts are bare u64 in the documented unit; there is
/// no conversion or float. Keep units exact and typed so a token count is
/// never summed with a byte count.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenUnit {
    /// Tokenizer tokens under the locked provider tokenizer.
    Tokens,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ByteUnit {
    /// Visible bytes presented to the model.
    Bytes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallUnit {
    /// Model-visible Zero calls (`z.read`/`z.find`/`z.edit`/`z.apply`/`z.run`/`z.state`).
    Calls,
}

const MAX_STRING_BYTES: usize = 256;

/// Machine fingerprint bound to every observation. All fields are required and
/// validated; an incomplete fingerprint yields `Unknown` in the report rather
/// than an invented claim. This fingerprint is measurement metadata only and
/// never gates authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineFingerprint {
    pub os: String,
    pub arch: String,
    pub cpu_model: String,
    pub kernel: String,
    pub rustc_version: String,
    /// Lowercase hex git SHA (40 or 64 chars) of the measured commit.
    pub git_sha: String,
    /// Cargo profile that produced the measurement (`release-perf`, `release`, etc.).
    pub cargo_profile: String,
}

impl MachineFingerprint {
    pub fn validate(&self) -> Result<(), ObservationError> {
        validate_string("os", &self.os)?;
        validate_string("arch", &self.arch)?;
        validate_string("cpu_model", &self.cpu_model)?;
        validate_string("kernel", &self.kernel)?;
        validate_string("rustc_version", &self.rustc_version)?;
        validate_string("cargo_profile", &self.cargo_profile)?;
        validate_git_sha(&self.git_sha)?;
        Ok(())
    }

    /// Deterministic equality for pairing: every field must match exactly.
    pub fn comparable(&self, other: &Self) -> bool {
        self == other
    }
}

/// Task identity bound to every observation. The same `task_id` (and optional
/// corpus digest) must match across a native/Zero pair for the pair to be
/// comparable. Task identity is measurement scoping, not engine authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskIdentity {
    /// Stable task identifier (scenario id, corpus file + query, etc.).
    pub task_id: String,
    /// Optional corpus or fixture digest that disambiguates the task content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corpus_sha: Option<String>,
}

impl TaskIdentity {
    pub fn validate(&self) -> Result<(), ObservationError> {
        validate_string("task_id", &self.task_id)?;
        if let Some(sha) = &self.corpus_sha {
            validate_hex_digest("corpus_sha", sha, &[64])?;
        }
        Ok(())
    }

    pub fn comparable(&self, other: &Self) -> bool {
        self == other
    }
}

/// One resource usage snapshot with exact units. All three counters are raw
/// u64 counts in their documented unit. No float, no percentage string.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasuredUsage {
    /// Tokenizer tokens (exact, under the locked tokenizer).
    pub tokens: u64,
    /// Visible bytes (exact).
    pub bytes: u64,
    /// Model-visible Zero calls (exact).
    pub calls: u64,
    /// Unit label for tokens (always `Tokens`).
    pub tokens_unit: TokenUnit,
    /// Unit label for bytes (always `Bytes`).
    pub bytes_unit: ByteUnit,
    /// Unit label for calls (always `Calls`).
    pub calls_unit: CallUnit,
}

impl MeasuredUsage {
    pub fn new(tokens: u64, bytes: u64, calls: u64) -> Self {
        Self {
            tokens,
            bytes,
            calls,
            tokens_unit: TokenUnit::Tokens,
            bytes_unit: ByteUnit::Bytes,
            calls_unit: CallUnit::Calls,
        }
    }
    pub fn validate(&self) -> Result<(), ObservationError> {
        // Units are type-checked by construction; counters are u64 so no
        // further range check is needed. This method exists for symmetry and
        // future stricter checks.
        Ok(())
    }
}

/// Which side of the paired measurement an observation represents.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationKind {
    /// Unoptimized native baseline (no `z.*` compression/projection).
    NativeBaseline,
    /// Model-visible Zero path (`z.read`/`z.find`/`z.edit`/`z.apply`/`z.run`/`z.state` direct).
    ZeroDirect,
}

/// One paired-measurement observation: task + machine + kind + exact usage.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub task: TaskIdentity,
    pub machine: MachineFingerprint,
    pub kind: ObservationKind,
    pub usage: MeasuredUsage,
}

impl Observation {
    pub fn validate(&self) -> Result<(), ObservationError> {
        self.task.validate()?;
        self.machine.validate()?;
        self.usage.validate()?;
        Ok(())
    }
}

fn validate_string(field: &'static str, value: &str) -> Result<(), ObservationError> {
    if value.is_empty() {
        return Err(ObservationError::EmptyField(field));
    }
    if value.len() > MAX_STRING_BYTES {
        return Err(ObservationError::FieldTooLong {
            field,
            len: value.len(),
        });
    }
    if value.chars().any(|c| c.is_control()) {
        return Err(ObservationError::ControlCharacter(field));
    }
    Ok(())
}

fn validate_git_sha(value: &str) -> Result<(), ObservationError> {
    if value.len() != 40 && value.len() != 64 {
        return Err(ObservationError::InvalidGitSha);
    }
    if !value
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ObservationError::InvalidGitSha);
    }
    Ok(())
}

fn validate_hex_digest(
    field: &'static str,
    value: &str,
    allowed: &[usize],
) -> Result<(), ObservationError> {
    if !allowed.contains(&value.len()) {
        return Err(ObservationError::FieldTooLong {
            field,
            len: value.len(),
        });
    }
    if !value.bytes().all(|b| b.is_ascii_hexdigit())
        || value.chars().any(|c| c.is_ascii_uppercase())
    {
        // require lowercase hex
        if value.bytes().any(|b| (b'A'..=b'F').contains(&b)) {
            return Err(ObservationError::InvalidHexDigest(field));
        }
        if !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ObservationError::InvalidHexDigest(field));
        }
    }
    Ok(())
}

/// Typed, fail-closed observation validation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObservationError {
    EmptyField(&'static str),
    FieldTooLong { field: &'static str, len: usize },
    ControlCharacter(&'static str),
    InvalidGitSha,
    InvalidHexDigest(&'static str),
}

impl fmt::Display for ObservationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyField(field) => write!(f, "observation field {field} must be nonempty"),
            Self::FieldTooLong { field, len } => {
                write!(
                    f,
                    "observation field {field} is {len} bytes, maximum {MAX_STRING_BYTES}"
                )
            }
            Self::ControlCharacter(field) => {
                write!(
                    f,
                    "observation field {field} must be free of control characters"
                )
            }
            Self::InvalidGitSha => write!(f, "git_sha must be 40 or 64 lowercase hex characters"),
            Self::InvalidHexDigest(field) => {
                write!(f, "field {field} must be lowercase hex")
            }
        }
    }
}

impl Error for ObservationError {}
