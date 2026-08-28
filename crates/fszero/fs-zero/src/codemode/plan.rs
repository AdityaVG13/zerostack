//! CodeMode plan entry — parse → runtime → host.

use super::host::{finish, finish_error};
use super::js::execute_js_plan;
use super::program::{PlanStep, parse_program};
use super::recipes::try_recipe_with_session;
use super::runtime::{ERROR_REF, execute_program};
use crate::core::FSZeroSession;

/// Whether `plan` looks like JavaScript rather than a recipe/JSON-DAG.
/// Shared by full and stub JS modules so mcp-only and codemode agree.
pub fn looks_like_js_plan(plan: &str) -> bool {
    let trimmed = plan.trim_start();
    trimmed.starts_with("export default")
        || trimmed.starts_with("async function")
        || trimmed.starts_with("function")
        || trimmed.starts_with("fs.")
        || trimmed.starts_with("ctx.")
        || trimmed.starts_with("return ")
        || trimmed.starts_with("throw ")
        || trimmed.starts_with("let ")
        || trimmed.starts_with("const ")
        || trimmed.starts_with("var ")
        || trimmed.starts_with("await ")
        || trimmed.starts_with("for ")
        || trimmed.starts_with("async ")
        || trimmed.contains("({ fs")
        || trimmed.contains("fs.multiRead")
        || trimmed.contains("fs.multiList")
        || trimmed.contains("fs.multiAstSearch")
        || trimmed.contains("fs.multi_read")
        || trimmed.contains("fs.multi_list")
        || trimmed.contains("fs.multi_search")
        || trimmed.contains("fs.multi_ast_search")
        || trimmed.contains("await fs.")
        || trimmed.contains("await zero.")
        || trimmed.contains("=>")
}

/// Signature for the only audited NORMAL-receipt path. The first read and any
/// path/size/mtime change use FULL; only a repeated stable JSON read relaxes.
/// Search and other logical reads can persist indexes, so they stay FULL-safe.
fn relaxed_json_read_signature(session: &FSZeroSession, plan: &str) -> Option<String> {
    let trimmed = plan.trim_start();
    let program = parse_program(plan).ok()?;
    if !trimmed.starts_with('{')
        || program.steps.is_empty()
        || super::program::validate_program(&program).is_err()
    {
        return None;
    }
    let mut signature = plan.to_string();
    let mut append = |call: &str, args: &serde_json::Value| -> Option<()> {
        if call != "fs.read" {
            return None;
        }
        let path = args.get("path")?.as_str()?;
        let full = session
            .root
            .as_ref()
            .map_or_else(|| std::path::PathBuf::from(path), |root| root.join(path));
        let metadata = std::fs::metadata(full).ok()?;
        let modified = metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        signature.push_str(&format!("\n{path}:{}:{modified}", metadata.len()));
        Some(())
    };
    for step in &program.steps {
        match step {
            PlanStep::Call { call, args, .. } => append(call, args)?,
            PlanStep::Parallel { branches, .. } => {
                for branch in branches {
                    append(&branch.call, &branch.args)?;
                }
            }
        }
    }
    Some(signature)
}

/// Execute a CodeMode plan — visible result is always 1 token (`C` or `X0`).
///
/// The whole execution (runtime step writes + finish()'s ~9 persist keys)
/// runs inside ONE store transaction, and WAL truncation happens on a
/// post-execution cadence — never mid-request via autocheckpoint (profiled
/// as periodic ~300ms/request fsync+read storms on warm servers).
///
pub fn execute_plan(session: &mut FSZeroSession, plan: &str) -> String {
    // CodeMode telemetry is per execution. Without this reset the recovery
    // counters are session-cumulative, so a later ref-only plan inherits bytes
    // materialized by unrelated earlier work and cannot support honest savings
    // accounting.
    session.recovery.reset_metrics();
    session.reset_codemode_measurement();
    // Exact-serve marks are scoped to one execution: a later plan that merely
    // re-reads the same bytes must still pay the novelty budget (b4yg).
    session.clear_exact_served_content();
    {
        // Watch mode: apply pending filesystem events before the plan runs so
        // every step (including parallel read branches) sees a fresh index.
        session.drain_watch_events();
        // Exclude watch reconciliation from the plan's materialization ledger.
        session.recovery.reset_metrics();
        session.reset_codemode_measurement();
        // In-memory recovery needs no SQLite txn/WAL cadence (hot test path).
        session.codemode_edit_plan = true;
        let read_signature = relaxed_json_read_signature(session, plan);
        let use_relaxed = read_signature.as_ref().is_some_and(|signature| {
            session.codemode_relaxed_read_signature.as_ref() == Some(signature)
        });
        if read_signature.is_none() {
            session.codemode_relaxed_read_signature = None;
        }
        let began = if !session.recovery.is_durable() {
            false
        } else if use_relaxed {
            session.recovery.begin_exec_txn_relaxed()
        } else {
            session.recovery.begin_exec_txn()
        };
        let mut ack = execute_plan_inner(session, plan);
        let mut success = !ack.starts_with("X0");
        let intents = std::mem::take(&mut session.pending_edit_intents);
        let mut finalization_error = None;
        let mut deferred_receipt_commit = false;
        if success {
            for id in &intents {
                if let Err(error) = session.recovery.clear_edit_intent(*id) {
                    finalization_error = Some(format!("edit intent finalization failed: {error}"));
                    success = false;
                    break;
                }
            }
        }
        if began {
            if success && use_relaxed && session.codemode_defer_wire_receipt {
                // plan_tool_result appends the durable envelope blob, then
                // commits this same NORMAL receipt transaction before reply.
                deferred_receipt_commit = true;
                session.codemode_relaxed_read_signature = read_signature.clone();
            } else if success {
                session.recovery.commit_exec_txn(true);
                if let Some(error) = session.recovery.take_store_error() {
                    session.codemode_relaxed_read_signature = None;
                    finalization_error = Some(format!("edit evidence commit failed: {error}"));
                    success = false;
                } else if read_signature.is_some() {
                    session.codemode_relaxed_read_signature = read_signature.clone();
                }
            } else {
                // fszero-quer: rollback drops the pending overlay that held
                // ERROR_REF (and the rest of finish()'s audit keys). Capture
                // the failure reason first and re-park it after so CLI expand
                // still names the real cause instead of "no recorded reason".
                let failure_reason = session.expand(ERROR_REF);
                session.recovery.rollback_exec_txn(true);
                if let Some(bytes) = failure_reason {
                    session.recovery.put_key(ERROR_REF, &bytes);
                }
            }
        }
        if !success && !intents.is_empty() {
            match session.root.clone() {
                Some(root) => match session.recovery.reconcile_edit_intents(&root) {
                    Ok(()) => {
                        if let Err(error) = session.build_index() {
                            finalization_error =
                                Some(format!("edit index reconciliation failed: {error}"));
                        }
                        if !matches!(
                            session
                                .last_mutation_outcome
                                .as_ref()
                                .map(|outcome| outcome.state),
                            Some(crate::core::MutationState::MutationFree)
                        ) {
                            session.last_mutation_outcome =
                                Some(crate::core::MutationOutcome::new(
                                    crate::core::MutationState::RolledBack,
                                    "plan_finalize",
                                    Some(root.display().to_string()),
                                ));
                        }
                    }
                    Err(error) => {
                        finalization_error = Some(format!("edit recovery indeterminate: {error}"));
                        session.last_mutation_outcome = Some(crate::core::MutationOutcome::new(
                            crate::core::MutationState::Indeterminate,
                            "plan_finalize",
                            Some(root.display().to_string()),
                        ));
                    }
                },
                None => finalization_error = Some("edit recovery requires a workspace root".into()),
            }
        }
        session.codemode_edit_plan = false;
        if let Some(error) = finalization_error {
            ack = finish_error(session, &error);
        }
        if !deferred_receipt_commit {
            session.recovery.maintain_wal_cadence();
        }
        ack
    }
}

fn execute_plan_inner(session: &mut FSZeroSession, plan: &str) -> String {
    let trimmed = plan.trim_start();
    let program = if let Some(p) = try_recipe_with_session(plan, session) {
        p
    } else if trimmed.starts_with('{') {
        match parse_program(plan) {
            Ok(p) => p,
            Err(e) => return finish_error(session, &e),
        }
    } else if looks_like_js_plan(plan) {
        let outcome = execute_js_plan(session, plan);
        return finish(session, &outcome);
    } else {
        match parse_program(plan) {
            Ok(p) => p,
            // The statement grammar is a convenience for pipe-style micro
            // plans; anything it cannot parse MUST run in the real JS
            // sandbox instead of erroring (or worse: the old lossy
            // extraction, which silently dropped object args — same bug
            // class as tokenzero's lenient-parser corruption).
            Err(_) => {
                let outcome = execute_js_plan(session, plan);
                return finish(session, &outcome);
            }
        }
    };
    if let Err(e) = super::program::validate_program(&program) {
        return finish_error(session, &e);
    }
    let outcome = execute_program(session, &program);
    finish(session, &outcome)
}
