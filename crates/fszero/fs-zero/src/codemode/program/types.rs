//! Plan types and step builders.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransactionMode {
    #[default]
    Auto,
    On,
    Off,
}

impl TransactionMode {
    pub(super) fn parse(raw: Option<&Value>) -> Self {
        match raw {
            Some(Value::Bool(true)) => Self::On,
            Some(Value::Bool(false)) => Self::Off,
            Some(Value::String(s)) if s == "on" => Self::On,
            Some(Value::String(s)) if s == "off" => Self::Off,
            _ => Self::Auto,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Program {
    pub steps: Vec<PlanStep>,
    pub label: String,
    pub transaction: TransactionMode,
}

#[derive(Debug, Clone)]
pub enum PlanStep {
    Call {
        id: Option<String>,
        call: String,
        args: Value,
        needs: Vec<String>,
    },
    Parallel {
        id: Option<String>,
        branches: Vec<ParallelBranch>,
        on_error: ParallelOnError,
        needs: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct ParallelBranch {
    pub id: String,
    pub call: String,
    pub args: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParallelOnError {
    FailFast,
    Collect,
}

impl ParallelOnError {
    pub(super) fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some("collect") => Self::Collect,
            _ => Self::FailFast,
        }
    }
}

/// Legacy single-call step — recipes compile to [`PlanStep::Call`].
#[derive(Debug, Clone)]
pub struct Step {
    pub call: String,
    pub args: Value,
}

impl Step {
    pub fn into_plan_step(self) -> PlanStep {
        PlanStep::Call {
            id: None,
            call: self.call,
            args: self.args,
            needs: Vec::new(),
        }
    }
}

impl Program {
    pub fn single(call: impl Into<String>, args: Value, label: impl Into<String>) -> Self {
        Self {
            steps: vec![call_step(call, args)],
            label: label.into(),
            transaction: TransactionMode::Auto,
        }
    }

    pub fn leaf_count(&self) -> usize {
        self.steps.iter().map(PlanStep::leaf_count).sum()
    }
}

impl PlanStep {
    pub fn leaf_count(&self) -> usize {
        match self {
            PlanStep::Call { .. } => 1,
            PlanStep::Parallel { branches, .. } => branches.len(),
        }
    }

    /// Declared data dependencies (ids of producer steps/branches).
    pub fn needs(&self) -> &[String] {
        match self {
            PlanStep::Call { needs, .. } | PlanStep::Parallel { needs, .. } => needs,
        }
    }
}

pub fn call_step(call: impl Into<String>, args: Value) -> PlanStep {
    PlanStep::Call {
        id: None,
        call: call.into(),
        args,
        needs: Vec::new(),
    }
}

/// Sequential call step with an explicit binding id (`$id.path` resolvable).
pub fn named_call_step(id: impl Into<String>, call: impl Into<String>, args: Value) -> PlanStep {
    PlanStep::Call {
        id: Some(id.into()),
        call: call.into(),
        args,
        needs: Vec::new(),
    }
}

pub fn parallel_step(branches: Vec<ParallelBranch>) -> PlanStep {
    parallel_step_with_needs(branches, Vec::new())
}

pub fn parallel_step_with_needs(branches: Vec<ParallelBranch>, needs: Vec<String>) -> PlanStep {
    PlanStep::Parallel {
        id: None,
        branches,
        on_error: ParallelOnError::FailFast,
        needs,
    }
}

pub fn bound_read_step(path_binding: &str, needs: Vec<String>) -> PlanStep {
    PlanStep::Call {
        id: None,
        call: "fs.read".to_string(),
        args: serde_json::json!({ "path": path_binding }),
        needs,
    }
}

pub fn parallel_branch(
    id: impl Into<String>,
    call: impl Into<String>,
    args: Value,
) -> ParallelBranch {
    ParallelBranch {
        id: id.into(),
        call: call.into(),
        args,
    }
}
