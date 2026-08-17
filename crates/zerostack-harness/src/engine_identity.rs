//! Subject≠Oracle discriminator. Not `zero_abi::raw_worker::EngineIdentity`.

use std::fmt;

/// Subject is always the current hub product at HEAD.
pub const SUBJECT_IDENTITY_LABEL: &str = "zerostack";

pub const ORACLE_SPEC_V1: &str = "spec-v1";
pub const ORACLE_PROPERTY_SUITE_V1: &str = "property-suite-v1";
pub const ORACLE_ROUND_TRIP: &str = "round-trip";
pub const ORACLE_MIRI: &str = "miri";
pub const ORACLE_CLIPPY: &str = "clippy";

const PRIOR_COMMIT_PREFIX: &str = "prior-commit-";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineRole {
    Subject,
    Oracle,
}

/// Harness-side identity. Distinct type from `raw_worker::EngineIdentity`
/// (`fszero` / `graphzero` / `tokenzero` dispatch).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineIdentity {
    pub role: EngineRole,
    pub label: String,
}

impl EngineIdentity {
    pub fn subject() -> Self {
        Self {
            role: EngineRole::Subject,
            label: SUBJECT_IDENTITY_LABEL.to_owned(),
        }
    }

    pub fn oracle(label: impl Into<String>) -> Self {
        Self {
            role: EngineRole::Oracle,
            label: label.into(),
        }
    }

    pub fn prior_commit(sha: &str) -> Self {
        Self::oracle(prior_commit_label(sha))
    }
}

impl fmt::Display for EngineIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let role = match self.role {
            EngineRole::Subject => "Subject",
            EngineRole::Oracle => "Oracle",
        };
        write!(f, "{role}::{}", self.label)
    }
}

pub fn prior_commit_label(sha: &str) -> String {
    let sha = sha.trim();
    if let Some(rest) = sha.strip_prefix(PRIOR_COMMIT_PREFIX) {
        format!("{PRIOR_COMMIT_PREFIX}{rest}")
    } else {
        format!("{PRIOR_COMMIT_PREFIX}{sha}")
    }
}

/// Allowed greenfield oracle labels. `prior-commit-<sha>` is the only prefix form.
pub fn oracle_label_is_allowed(label: &str) -> bool {
    matches!(
        label,
        ORACLE_SPEC_V1 | ORACLE_PROPERTY_SUITE_V1 | ORACLE_ROUND_TRIP | ORACLE_MIRI | ORACLE_CLIPPY
    ) || (label.starts_with(PRIOR_COMMIT_PREFIX)
        && label.len() > PRIOR_COMMIT_PREFIX.len()
        && label
            .as_bytes()
            .get(PRIOR_COMMIT_PREFIX.len()..)
            .is_some_and(|rest| {
                !rest.is_empty()
                    && rest
                        .iter()
                        .all(|b| b.is_ascii_hexdigit() || *b == b'-' || *b == b'_')
            }))
}

/// Comparator entry guard. Panics if Subject==Oracle or labels are empty.
pub fn assert_subject_ne_oracle(subject: &str, oracle: &str) {
    if subject.is_empty() || oracle.is_empty() {
        panic!("EngineIdentity unset: subject={subject:?} oracle={oracle:?}");
    }
    if subject == oracle {
        panic!("EngineIdentity collision: oracle being compared against itself ({subject})");
    }
    if subject != SUBJECT_IDENTITY_LABEL {
        panic!("Subject identity {subject:?} != expected {SUBJECT_IDENTITY_LABEL:?}");
    }
    if !oracle_label_is_allowed(oracle) {
        panic!(
            "Oracle identity {oracle:?} is not in {{spec-v1,property-suite-v1,prior-commit-<sha>,round-trip,miri,clippy}}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_label_is_zerostack() {
        assert_eq!(SUBJECT_IDENTITY_LABEL, "zerostack");
        assert_ne!(SUBJECT_IDENTITY_LABEL, ORACLE_SPEC_V1);
    }

    #[test]
    fn prior_commit_label_is_prefixed() {
        assert_eq!(prior_commit_label("abc123"), "prior-commit-abc123");
        assert!(oracle_label_is_allowed("prior-commit-abc123"));
    }

    #[test]
    #[should_panic(expected = "EngineIdentity collision")]
    fn self_comparison_panics() {
        assert_subject_ne_oracle("zerostack", "zerostack");
    }
}
