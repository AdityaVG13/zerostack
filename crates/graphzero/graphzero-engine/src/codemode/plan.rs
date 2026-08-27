//! Plan classification, recipe dispatch, and capability manifests.

use super::errors::validation_error;
use super::state::ExecutionState;
use super::steps::{run_blast_step, run_expand_step, run_query_step};
use super::types::{BindingResult, CodeModeError};

#[derive(Clone, Copy)]
pub(crate) enum PlanKind {
    Recipe,
    Json,
    Code,
}

impl PlanKind {
    pub(crate) fn from_form(form: &str) -> Option<Self> {
        match form {
            "recipe" => Some(Self::Recipe),
            "json" => Some(Self::Json),
            "js" | "code" => Some(Self::Code),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Recipe => "recipe",
            Self::Json => "json",
            Self::Code => "code",
        }
    }
}

// ── classification ──

/// Substrings that mark a plan as sandboxed JS/code (case-insensitive scan).
const CODE_MARKERS_LOWER: &[&str] = &[
    "fetch(",
    "process",
    "require(",
    "setinterval",
    "settimeout",
    "std::fs",
    "while(true",
    "while (true",
];

/// Substrings that mark a plan as code (case-sensitive scan; keep prior behavior).
const CODE_MARKERS_EXACT: &[&str] = &[
    "graph.",
    "zero.graph",
    "ctx.",
    "return",
    "throw",
    "await",
    "export default",
];

/// Classify recipe / JSON DAG / code plan forms without a long boolean ladder.
pub(crate) fn classify_plan(plan: &str) -> PlanKind {
    let trimmed = plan.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return PlanKind::Json;
    }
    let lower = trimmed.to_ascii_lowercase();
    let is_code = CODE_MARKERS_LOWER.iter().any(|m| lower.contains(m))
        || CODE_MARKERS_EXACT.iter().any(|m| trimmed.contains(m));
    if is_code {
        PlanKind::Code
    } else {
        PlanKind::Recipe
    }
}

// ── recipe execution ──

pub(crate) fn execute_recipe(
    state: &mut ExecutionState<'_>,
    recipe: &str,
) -> Result<BindingResult, CodeModeError> {
    let trimmed = recipe.trim();
    let (op, target) = trimmed
        .split_once(':')
        .ok_or_else(|| validation_error("recipe must be op:target", Some("recipe")))?;
    match op.trim() {
        "defs" | "symbol" => run_query_step(state, "recipe.defs", "symbol", target.trim()),
        "callers" => run_query_step(state, "recipe.callers", "callers", target.trim()),
        "reading_set" | "reading-set" | "readingset" => {
            run_query_step(state, "recipe.reading_set", "reading_set", target.trim())
        }
        "tests" => run_query_step(state, "recipe.tests", "search", &format!("test {target}")),
        "blast" => run_blast_step(state, "recipe.blast", target.trim(), None),
        "expand" => run_expand_step(state, "recipe.expand", target.trim(), 0),
        other => Err(validation_error(
            format!("unknown recipe {other}"),
            Some("recipe"),
        )),
    }
}
