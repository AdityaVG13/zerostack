//! Property-oracle smoke adapter. Full proptest suites land in later phases.

use zero_ledger::FreshWorkVector;

use crate::oracle::ScenarioError;

/// Deterministic smoke: four-component sum is `total_tokens` on a small grid.
pub fn run_fresh_work_sum_property() -> Result<(), ScenarioError> {
    for fresh in 0u64..8 {
        for replayed in 0u64..8 {
            for recovery in 0u64..4 {
                let vector =
                    FreshWorkVector::new(fresh, replayed, recovery, 1).map_err(|error| {
                        ScenarioError::new("property", format!("FreshWorkVector::new: {error}"))
                    })?;
                let expected = fresh
                    .checked_add(replayed)
                    .and_then(|n| n.checked_add(recovery))
                    .and_then(|n| n.checked_add(1))
                    .ok_or_else(|| ScenarioError::new("property", "overflow"))?;
                if vector.total_tokens() != expected {
                    return Err(ScenarioError::new(
                        "property",
                        format!(
                            "sum {} != {expected} for ({fresh},{replayed},{recovery},1)",
                            vector.total_tokens()
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

pub fn run_named(name: &str) -> Result<(), ScenarioError> {
    match name {
        "fresh_work_sum" => run_fresh_work_sum_property(),
        other => Err(ScenarioError::new(
            "property",
            format!("unknown property {other}"),
        )),
    }
}
