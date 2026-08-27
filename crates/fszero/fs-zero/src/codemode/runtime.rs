use super::bindings::{BindingStore, binding_from_call, binding_from_parallel};
use super::connector::{FsConnector, FsStep};
use super::limits::{MAX_PHYSICAL_OPS, effective_max_wall_ms};
use super::parallel::{execute_parallel_group, invoke_call};
use super::program::{ParallelBranch, PlanDag, PlanStep, Program};
use super::transaction::TransactionJournal;
use super::world_parse::world_id_from_kernel_message;
use crate::codemode::host::ContractError;
use crate::core::FSZeroSession;
use serde_json::Value;
use std::collections::HashSet;
use std::time::{Duration, Instant};

pub const STEPS_REF: &str = "codemode/steps";
pub const RESULT_REF: &str = "codemode/result";
pub const ERROR_REF: &str = "codemode/error";

/// Pure-read methods eligible for consecutive identical-op fusion (fszero-ncib.9).
const PURE_READONLY_METHODS: &[&str] = &[
    "fs.read",
    "fs.ls",
    "fs.stat",
    "fs.search",
    "fs.expand",
    "fs.history",
    "fs.memory.get",
    "fs.memory.ls",
];

fn is_pure_readonly_method(call: &str) -> bool {
    PURE_READONLY_METHODS.contains(&call)
}

/// Park ERROR_REF and append a failed X0 step (shared by dependency / bind failures).
fn record_x0_step(
    session: &mut FSZeroSession,
    steps_log: &mut Vec<StepLog>,
    index: usize,
    method: impl Into<String>,
    detail: String,
    parallel: bool,
) {
    session.recovery.put_key(ERROR_REF, detail.as_bytes());
    steps_log.push(StepLog {
        index,
        method: method.into(),
        ack: "X0".to_string(),
        ok: false,
        recovery_key: ERROR_REF.to_string(),
        detail: Some(detail),
        parallel,
    });
}

fn mark_step_x0(ok: &mut bool, last_ref: &mut String, last_ack: &mut String) {
    *ok = false;
    *last_ref = ERROR_REF.to_string();
    *last_ack = "X0".to_string();
}

/// Kill-test helper: fusion must only apply to pure-read methods.
pub fn fusion_eligible_methods() -> &'static [&'static str] {
    PURE_READONLY_METHODS
}

#[derive(Debug, Clone)]
pub struct StepLog {
    pub index: usize,
    pub method: String,
    pub ack: String,
    pub ok: bool,
    pub recovery_key: String,
    pub detail: Option<String>,
    pub parallel: bool,
}

#[derive(Debug, Clone)]
pub struct RuntimeOutcome {
    pub ok: bool,
    pub label: String,
    pub steps_run: usize,
    pub summary: String,
    pub primary_ref: String,
    pub steps: Vec<StepLog>,
    /// Derived plan DAG (nodes, edges, batch-parallel levels) for the receipt;
    /// `None` for JS plans and validation-failure outcomes.
    pub dag: Option<PlanDag>,
    pub internal_actions: u32,
    pub logical_ops: u32,
    pub physical_ops: u32,
    pub batched_ops: u32,
    pub parallel_groups: u32,
    pub parallel_wall_ms: u64,
    pub transaction_rolled_back: bool,
    pub wall_ms: u64,
    pub error: Option<ContractError>,
}

impl RuntimeOutcome {
    /// Zero-step failure outcome (validate/init/unavailable paths).
    pub fn failed(
        label: impl Into<String>,
        summary: impl Into<String>,
        error: ContractError,
    ) -> Self {
        Self {
            ok: false,
            label: label.into(),
            steps_run: 0,
            summary: summary.into(),
            primary_ref: ERROR_REF.to_string(),
            steps: Vec::new(),
            dag: None,
            internal_actions: 0,
            logical_ops: 0,
            physical_ops: 0,
            batched_ops: 0,
            parallel_groups: 0,
            parallel_wall_ms: 0,
            transaction_rolled_back: false,
            wall_ms: 0,
            error: Some(error),
        }
    }

    pub fn with_internal_actions(mut self, n: u32) -> Self {
        self.internal_actions = n;
        self
    }
}

/// Shared step-log serialization. Empty-plan `ok` differs by surface
/// (runtime early-fail vs JS success envelope vs host outcome). When a DAG
/// was derived, its structure (nodes, edges, batch-parallel levels) is part
/// of the receipt -- never an opaque linear list.
pub(crate) fn format_steps_body(
    steps: &[StepLog],
    empty_ok: bool,
    dag: Option<&PlanDag>,
) -> String {
    let body = if steps.is_empty() {
        format!("steps=0 ok={empty_ok}")
    } else {
        steps
            .iter()
            .map(|s| {
                let mut line = format!(
                    "step={} method={} ack={} ok={} ref={} parallel={}",
                    s.index, s.method, s.ack, s.ok, s.recovery_key, s.parallel
                );
                if let Some(detail) = &s.detail {
                    line.push_str(&format!(" detail={detail}"));
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    // The DAG line describes the whole plan, not a step: it leads the body as
    // a header so `last line == last step` stays true for receipt consumers.
    match dag {
        Some(dag) => format!("dag:{}\n{body}", dag.to_json()),
        None => body,
    }
}

fn write_early_refs(session: &mut FSZeroSession, steps: &[StepLog], summary: &str) {
    session
        .recovery
        .put_key(STEPS_REF, format_steps_body(steps, false, None).as_bytes());
    session.recovery.put_key(RESULT_REF, summary.as_bytes());
}

fn check_needs(
    session: &mut FSZeroSession,
    needs: &[String],
    step_index: usize,
    call: &str,
    steps_log: &mut Vec<StepLog>,
    completed_ids: &HashSet<String>,
    parallel: bool,
) -> bool {
    for need in needs {
        if !completed_ids.contains(need) {
            let detail = format!("unmet dependency '{need}' at step {step_index}");
            record_x0_step(session, steps_log, step_index, call, detail, parallel);
            return false;
        }
    }
    true
}

fn resolve_parallel_branches(
    session: &mut FSZeroSession,
    step_index: usize,
    branches: &[ParallelBranch],
    bindings: &BindingStore,
    steps_log: &mut Vec<StepLog>,
) -> Result<Vec<ParallelBranch>, String> {
    let mut resolved = Vec::with_capacity(branches.len());
    for branch in branches {
        match bindings.resolve_args(&branch.call, &branch.args) {
            Ok(args) => resolved.push(ParallelBranch {
                id: branch.id.clone(),
                call: branch.call.clone(),
                args,
            }),
            Err(detail) => {
                record_x0_step(
                    session,
                    steps_log,
                    step_index,
                    branch.call.clone(),
                    detail.clone(),
                    true,
                );
                return Err(detail);
            }
        }
    }
    Ok(resolved)
}

pub fn execute_program(session: &mut FSZeroSession, program: &Program) -> RuntimeOutcome {
    if let Err(reason) = super::program::validate_program(program) {
        session.recovery.put_key(ERROR_REF, reason.as_bytes());
        write_early_refs(
            session,
            &[],
            &format!("program:{} steps=0 error=validate", program.label),
        );
        return RuntimeOutcome::failed(
            program.label.clone(),
            format!("program:{} steps=0 error=validate", program.label),
            ContractError::validation(reason),
        );
    }
    // Explicit DAG semantics (V6-F6 / ZS-EXEC-001): derive the topological
    // schedule up front. Cycle rejection already happened inside
    // validate_program; this second build is the schedule authority (it can
    // only fail on internal inconsistency, never on a plan error).
    let dag = match PlanDag::build(program) {
        Ok(dag) => dag,
        Err(reason) => {
            session.recovery.put_key(ERROR_REF, reason.as_bytes());
            write_early_refs(
                session,
                &[],
                &format!("program:{} steps=0 error=dag", program.label),
            );
            return RuntimeOutcome::failed(
                program.label.clone(),
                format!("program:{} steps=0 error=dag", program.label),
                ContractError::validation(reason),
            );
        }
    };

    let start_ops = session.op_count;
    let wall_start = Instant::now();
    let wall_limit_ms = effective_max_wall_ms();
    let deadline = wall_start + Duration::from_millis(wall_limit_ms);
    let mut journal = TransactionJournal::for_program(program);
    let mut steps_log = Vec::new();
    let mut ok = true;
    let mut last_ref = RESULT_REF.to_string();
    let mut last_ack = String::new();
    let mut completed_ids = HashSet::new();
    let mut bindings = BindingStore::default();
    let mut parallel_groups = 0u32;
    let mut parallel_wall_ns = 0u128;
    let mut transaction_rolled_back = false;
    // Last physical pure-read result for consecutive identical-op fusion.
    let mut last_readonly_fuse: Option<(String, Value, FsStep)> = None;
    let mut fused_ops = 0u32;

    // Execution is sequential but the ORDER honors the DAG: dependents run
    // after producers even when a dependency is declared later in the list.
    for &declared_index in &dag.schedule_indices {
        let step = &program.steps[declared_index];
        let i = declared_index;
        if Instant::now() > deadline {
            let detail = format!(
                "max wall time exceeded: {wall_limit_ms}ms (codemode JS wall; raise via FSZERO_CODEMODE_WALL_MS or see fszero capabilities --json .budgets stage=codemode_js_wall) at step {i}"
            );
            record_x0_step(
                session,
                &mut steps_log,
                i,
                "deadline",
                detail.clone(),
                false,
            );
            mark_step_x0(&mut ok, &mut last_ref, &mut last_ack);
            break;
        }
        if session.op_count - start_ops >= MAX_PHYSICAL_OPS {
            let detail = format!("max physical ops exceeded: {MAX_PHYSICAL_OPS} at step {i}");
            record_x0_step(session, &mut steps_log, i, "limit", detail.clone(), false);
            mark_step_x0(&mut ok, &mut last_ref, &mut last_ack);
            break;
        }
        match step {
            PlanStep::Call {
                id,
                call,
                args,
                needs,
            } => {
                if !check_needs(
                    session,
                    needs,
                    i,
                    call,
                    &mut steps_log,
                    &completed_ids,
                    false,
                ) {
                    mark_step_x0(&mut ok, &mut last_ref, &mut last_ack);
                    break;
                }
                let resolved_args = match bindings.resolve_args(call, args) {
                    Ok(v) => v,
                    Err(detail) => {
                        mark_step_x0(&mut ok, &mut last_ref, &mut last_ack);
                        record_x0_step(session, &mut steps_log, i, call.clone(), detail, false);
                        break;
                    }
                };
                // Hot-path fusion (fszero-ncib.9): consecutive identical pure-read
                // calls reuse the prior FsStep (payload + recovery key) so N logical
                // ops collapse to 1 physical dispatch without re-auth/serialize.
                let result = if last_readonly_fuse.as_ref().is_some_and(|(c, a, s)| {
                    s.ok && c == call && a == &resolved_args && is_pure_readonly_method(call)
                }) {
                    fused_ops += 1;
                    last_readonly_fuse.as_ref().unwrap().2.clone()
                } else {
                    let mut connector = if journal.enabled() {
                        FsConnector::with_journal(session, &mut journal)
                    } else {
                        FsConnector::new(session)
                    };
                    let step = invoke_call(&mut connector, call, &resolved_args);
                    if step.ok && is_pure_readonly_method(call) {
                        last_readonly_fuse =
                            Some((call.clone(), resolved_args.clone(), step.clone()));
                    } else {
                        last_readonly_fuse = None;
                    }
                    step
                };
                ok &= result.ok;
                last_ref = result.recovery_key.to_string();
                last_ack = result.ack.clone();
                let bound_id = id.clone().unwrap_or_else(|| format!("step{i}"));
                if result.ok {
                    bindings.register(binding_from_call(
                        &bound_id,
                        &result.method,
                        &result.recovery_key,
                        &result.payload,
                        &resolved_args,
                    ));
                    bindings.register_step_index(i, &bound_id);
                    completed_ids.insert(bound_id);
                }
                steps_log.push(StepLog {
                    index: i,
                    method: result.method,
                    ack: result.ack,
                    ok: result.ok,
                    recovery_key: result.recovery_key.to_string(),
                    detail: result.detail,
                    parallel: false,
                });
                if !result.ok {
                    break;
                }
            }
            PlanStep::Parallel {
                id,
                branches,
                on_error,
                needs,
            } => {
                if !check_needs(
                    session,
                    needs,
                    i,
                    "parallel",
                    &mut steps_log,
                    &completed_ids,
                    true,
                ) {
                    mark_step_x0(&mut ok, &mut last_ref, &mut last_ack);
                    break;
                }
                let resolved_branches = match resolve_parallel_branches(
                    session,
                    i,
                    branches,
                    &bindings,
                    &mut steps_log,
                ) {
                    Ok(b) => b,
                    Err(detail) => {
                        mark_step_x0(&mut ok, &mut last_ref, &mut last_ack);
                        record_x0_step(session, &mut steps_log, i, "parallel", detail, true);
                        break;
                    }
                };
                let journal_error = resolved_branches.iter().find_map(|branch| {
                    let is_world = branch.call == "fs.world"
                        || branch.call == "fs.compound"
                            && branch.args.get("name").and_then(serde_json::Value::as_str)
                                == Some("world");
                    if !is_world {
                        return None;
                    }
                    match super::transaction::world_arg_from_args(&branch.args) {
                        Ok(Some(arg)) => journal.before_world(session, &arg).err(),
                        Ok(None) => None,
                        Err(error) => Some(error),
                    }
                });
                if let Some(detail) = journal_error {
                    mark_step_x0(&mut ok, &mut last_ref, &mut last_ack);
                    record_x0_step(session, &mut steps_log, i, "parallel", detail, true);
                    break;
                }
                let outcome = execute_parallel_group(
                    session,
                    parallel_groups as usize,
                    i,
                    &resolved_branches,
                    *on_error,
                );
                if journal.enabled() {
                    for branch in outcome
                        .branches
                        .iter()
                        .filter(|b| b.method == "fs.world" && b.ok)
                    {
                        if let Ok(text) = std::str::from_utf8(&branch.payload) {
                            if let Some(wid) = world_id_from_kernel_message(text) {
                                journal.record_world_created(&wid);
                            }
                        }
                    }
                }
                parallel_groups += 1;
                parallel_wall_ns += outcome.wall_us;
                ok &= outcome.ok;
                if let Some(last) = outcome.steps.last() {
                    last_ack = last.ack.clone();
                    last_ref = last.recovery_key.clone();
                }
                for branch in outcome.branches.iter() {
                    if branch.ok {
                        bindings.register(binding_from_parallel(
                            &branch.branch_id,
                            &branch.method,
                            &branch.payload_key,
                            &branch.payload,
                        ));
                        completed_ids.insert(branch.branch_id.clone());
                    }
                }
                if outcome.ok {
                    if let Some(group_id) = id {
                        completed_ids.insert(group_id.clone());
                    }
                }
                steps_log.extend(outcome.steps);
                if !outcome.ok {
                    break;
                }
            }
        }
    }

    session.recovery.put_key(
        STEPS_REF,
        format_steps_body(&steps_log, false, Some(&dag)).as_bytes(),
    );

    let last = steps_log
        .last()
        .map(|s| s.method.as_str())
        .unwrap_or("none");
    let summary = format!(
        "program:{} steps={} parallel_groups={} parallel_wall_ns={} last={last} ack={last_ack}",
        program.label,
        steps_log.len(),
        parallel_groups,
        parallel_wall_ns
    );
    session.recovery.put_key(RESULT_REF, summary.as_bytes());

    if !ok && journal.enabled() {
        match journal.rollback(session) {
            Ok(()) => transaction_rolled_back = true,
            Err(rollback_err) => {
                let existing = session
                    .expand(ERROR_REF)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default();
                let combined = if existing.is_empty() {
                    format!("rollback failed: {rollback_err}")
                } else {
                    format!("{existing}; rollback failed: {rollback_err}")
                };
                session.recovery.put_key(ERROR_REF, combined.as_bytes());
            }
        }
    }

    // fszero-quer: re-park a concrete failure reason under ERROR_REF so durable
    // exec-txn rollback + CLI expand still name the real cause (not a generic
    // "program execution failed" with a missing codemode/error key).
    let error = if !ok {
        let detail = session
            .expand(ERROR_REF)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                steps_log
                    .iter()
                    .rev()
                    .find_map(|s| s.detail.clone().filter(|d| !d.trim().is_empty()))
            })
            .unwrap_or_else(|| "program execution failed".to_string());
        session.recovery.put_key(ERROR_REF, detail.as_bytes());
        Some(ContractError::runtime(detail))
    } else {
        None
    };

    let steps_run = steps_log.len();
    let internal = session.op_count - start_ops;
    RuntimeOutcome {
        ok,
        label: program.label.clone(),
        steps_run,
        summary,
        primary_ref: last_ref,
        steps: steps_log,
        dag: Some(dag),
        internal_actions: internal,
        logical_ops: steps_run as u32,
        physical_ops: internal,
        batched_ops: fused_ops,
        parallel_groups,
        parallel_wall_ms: (parallel_wall_ns / 1000) as u64,
        transaction_rolled_back,
        error,
        wall_ms: wall_start.elapsed().as_millis() as u64,
    }
}
