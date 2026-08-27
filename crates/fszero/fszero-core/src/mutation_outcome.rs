use serde::{Deserialize, Serialize};

/// Engine-local classification for failures around an irreversible mutation boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationState {
    MutationFree,
    RolledBack,
    Changed,
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationOutcome {
    pub schema: &'static str,
    pub state: MutationState,
    pub boundary: String,
    pub path: Option<String>,
    pub pre_ref: Option<String>,
    pub post_ref: Option<String>,
    pub recovery_required: bool,
}
impl MutationOutcome {
    pub fn new(state: MutationState, boundary: impl Into<String>, path: Option<String>) -> Self {
        Self {
            schema: "fszero.mutation_outcome",
            state,
            boundary: boundary.into(),
            path,
            pre_ref: None,
            post_ref: None,
            recovery_required: state == MutationState::Indeterminate,
        }
    }
}
