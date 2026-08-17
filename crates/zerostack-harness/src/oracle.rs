//! Five-mode greenfield `scenario()` + both-error comparator.

use crate::engine_identity::{
    ORACLE_CLIPPY, ORACLE_MIRI, ORACLE_PROPERTY_SUITE_V1, ORACLE_ROUND_TRIP, ORACLE_SPEC_V1,
    SUBJECT_IDENTITY_LABEL, assert_subject_ne_oracle, prior_commit_label,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScenarioError {
    pub class: String,
    pub message: String,
}

impl ScenarioError {
    pub fn new(class: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            class: class.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ScenarioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.class, self.message)
    }
}

impl std::error::Error for ScenarioError {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubjectState {
    pub seed: u64,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubjectOutput {
    pub canonical: String,
    pub kind: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalTool {
    Miri,
    Clippy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OracleMode {
    Spec { tag: &'static str },
    Property { name: &'static str },
    Self_ { commit_sha: String },
    RoundTrip { pair: &'static str },
    ExternalTool(ExternalTool),
}

impl OracleMode {
    pub fn identity_label(&self) -> String {
        match self {
            Self::Spec { .. } => ORACLE_SPEC_V1.to_owned(),
            Self::Property { .. } => ORACLE_PROPERTY_SUITE_V1.to_owned(),
            Self::Self_ { commit_sha } => prior_commit_label(commit_sha),
            Self::RoundTrip { .. } => ORACLE_ROUND_TRIP.to_owned(),
            Self::ExternalTool(ExternalTool::Miri) => ORACLE_MIRI.to_owned(),
            Self::ExternalTool(ExternalTool::Clippy) => ORACLE_CLIPPY.to_owned(),
        }
    }
}

/// Both-error = agreement regardless of message. One-error-one-OK = hard failure.
pub fn compare(
    label: &str,
    subject: Result<SubjectOutput, ScenarioError>,
    oracle: Result<(), ScenarioError>,
) {
    match (subject, oracle) {
        (Ok(_), Ok(())) => {}
        (Err(_), Err(_)) => {}
        (Ok(out), Err(oracle_err)) => panic!(
            "{label}: one-error-one-OK: subject ok, oracle err ({oracle_err}); subject_output={out:?}"
        ),
        (Err(subject_err), Ok(())) => {
            panic!("{label}: one-error-one-OK: subject err ({subject_err}), oracle ok")
        }
    }
}

/// Greenfield 30-line scenario. `oracle_check` is the constructed Oracle.
pub fn scenario<S, A, O>(setup: S, action: A, oracle_check: O, mode: OracleMode, label: &str)
where
    S: FnOnce() -> SubjectState,
    A: FnOnce(SubjectState) -> Result<SubjectOutput, ScenarioError>,
    O: FnOnce(&Result<SubjectOutput, ScenarioError>) -> Result<(), ScenarioError>,
{
    assert_subject_ne_oracle(SUBJECT_IDENTITY_LABEL, &mode.identity_label());
    let state = setup();
    let subject = action(state);
    let oracle = oracle_check(&subject);
    compare(label, subject, oracle);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_error_agrees_regardless_of_message() {
        compare(
            "both-error",
            Err(ScenarioError::new("io", "subject said left")),
            Err(ScenarioError::new("spec", "oracle said right")),
        );
    }

    #[test]
    #[should_panic(expected = "one-error-one-OK")]
    fn one_error_one_ok_is_hard_failure() {
        compare(
            "mixed",
            Ok(SubjectOutput {
                canonical: "ok".into(),
                kind: "marker",
            }),
            Err(ScenarioError::new("spec", "no")),
        );
    }

    #[test]
    fn spec_mode_label_is_not_subject() {
        let mode = OracleMode::Spec {
            tag: "SPEC-RES-001",
        };
        assert_ne!(mode.identity_label(), SUBJECT_IDENTITY_LABEL);
        assert_subject_ne_oracle(SUBJECT_IDENTITY_LABEL, &mode.identity_label());
    }
}
