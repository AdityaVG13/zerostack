//! Self-oracle (prior-commit) smoke adapter. Insta suites land later.

use crate::oracle::{ScenarioError, SubjectOutput};

/// Compare current canonical output to a blessed string.
pub fn assert_matches_blessed(output: &SubjectOutput, blessed: &str) -> Result<(), ScenarioError> {
    if output.canonical == blessed {
        Ok(())
    } else {
        Err(ScenarioError::new(
            "self",
            format!(
                "snapshot drift: expected {blessed:?} got {:?}",
                output.canonical
            ),
        ))
    }
}
