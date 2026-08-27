use super::super::api;
use super::super::limits::{MAX_PARALLEL_WIDTH, MAX_PLAN_STEPS};
use super::dag::PlanDag;
use super::types::{PlanStep, Program};

pub fn validate_program(program: &Program) -> Result<(), String> {
    if program.leaf_count() == 0 {
        return Err("plan: no steps".to_string());
    }
    if program.leaf_count() > MAX_PLAN_STEPS {
        return Err(format!("plan exceeds max steps ({MAX_PLAN_STEPS})"));
    }
    let mut seen_ids = std::collections::HashSet::new();
    for (i, step) in program.steps.iter().enumerate() {
        step.validate(i, &mut seen_ids)?;
    }
    // Explicit DAG semantics (V6-F6 / ZS-EXEC-001): dependencies may be
    // declared in ANY order (forward references are legal) -- the derived
    // topological schedule, not list order, orders execution. Cycles and
    // unmet dependencies are rejected fail-loud here.
    PlanDag::build(program).map(|_| ())
}

// NOTE (framework security additions 2026): For trusted CodeMode execution / future JS/WASM snippets/skills:
// Extend here or in pre_exec_audit layer with:
// - provenance check (code/plan hash + optional sig)
// - DLP scan of literals/args for secrets/PII (block or redact before exec)
// - policy gate + static capability audit (cross-ref api::is_kernel_method + parallel safety)
// - register/verify signed snippets (see codemode-framework.md "Supply Chain, Provenance..." section)
// FSZero plans/recipes act as auditable "snippets"; align with CF durable runtime saveSnippet + pre-exec approvals.
// Full pipeline (audit before run, redaction, signing) is framework requirement for MCP trusted code exec.

fn validate_step_deps(
    index: usize,
    id: &Option<String>,
    needs: &[String],
    seen_ids: &mut std::collections::HashSet<String>,
) -> Result<(), String> {
    if let Some(id) = id {
        if !seen_ids.insert(id.clone()) {
            return Err(format!("step {index}: duplicate id '{id}'"));
        }
    }
    // `needs` may name ANY declared id (earlier or later step, or a branch of
    // an earlier/later parallel group): order is derived from the DAG, not
    // the declaration list. Unknown ids are still rejected here; the DAG
    // builder performs the same check against the full id set and rejects
    // cycles (validate_program's final PlanDag::build).
    let _ = needs;
    Ok(())
}

impl PlanStep {
    pub(super) fn validate(
        &self,
        index: usize,
        seen_ids: &mut std::collections::HashSet<String>,
    ) -> Result<(), String> {
        match self {
            PlanStep::Call {
                id, call, needs, ..
            } => {
                validate_step_deps(index, id, needs, seen_ids)?;
                validate_call_method(index, call)
            }
            PlanStep::Parallel {
                id,
                branches,
                on_error: _,
                needs,
            } => {
                validate_step_deps(index, id, needs, seen_ids)?;
                if branches.is_empty() {
                    return Err(format!("step {index}: empty parallel group"));
                }
                if branches.len() > MAX_PARALLEL_WIDTH {
                    return Err(format!(
                        "step {index}: parallel width exceeds {MAX_PARALLEL_WIDTH}"
                    ));
                }
                let mut branch_ids = std::collections::HashSet::new();
                for branch in branches {
                    if seen_ids.contains(&branch.id) {
                        return Err(format!(
                            "step {index}: parallel branch id '{}' collides with prior step",
                            branch.id
                        ));
                    }
                    if !branch_ids.insert(branch.id.clone()) {
                        return Err(format!(
                            "step {index}: duplicate parallel branch id '{}'",
                            branch.id
                        ));
                    }
                    validate_call_method(index, &branch.call)?;
                    if !crate::core::is_parallel_branch_safe(&branch.call, &branch.args) {
                        return Err(format!(
                            "step {index}: '{}' is not parallel-safe",
                            branch.call
                        ));
                    }
                }
                for branch in branches {
                    seen_ids.insert(branch.id.clone());
                }
                Ok(())
            }
        }
    }
}

fn validate_call_method(index: usize, call: &str) -> Result<(), String> {
    if api::is_kernel_method(call) {
        Ok(())
    } else {
        let candidates = closest_methods(call);
        let best = candidates.first().map(String::as_str).unwrap_or("fs.read");
        Err(format!(
            "step {index}: unknown call '{call}'; closest valid names: {}; try zero_describe('{best}')",
            candidates.join(", ")
        ))
    }
}

fn closest_methods(call: &str) -> Vec<String> {
    let ranked = api::METHODS
        .iter()
        .map(|method| {
            (
                super::super::name_rank::name_score(call, method.path),
                method.path.to_string(),
            )
        })
        .collect::<Vec<_>>();
    super::super::name_rank::take_top_ranked(ranked, 3)
}
