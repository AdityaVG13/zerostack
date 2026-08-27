//! Parallel plan branch execution — read-only threads + serialized world staging.

use super::program::{ParallelBranch, ParallelOnError};
use super::runtime::StepLog;
use crate::core::{
    FSZeroSession, OpCode, ParallelBranchWork, ParallelReadContext, execute_parallel_branch,
    execute_search_branch, is_parallel_branch_safe, kernel_visible_error, parse_budget_message,
    visible_ack,
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

pub struct ParallelBranchOutcome {
    pub branch_id: String,
    pub method: String,
    pub payload_key: String,
    pub payload: Vec<u8>,
    pub ok: bool,
}

pub struct ParallelGroupOutcome {
    pub ok: bool,
    pub steps: Vec<StepLog>,
    pub wall_us: u128,
    pub branches: Vec<ParallelBranchOutcome>,
}

pub fn execute_parallel_group(
    session: &mut FSZeroSession,
    group_index: usize,
    step_index: usize,
    branches: &[ParallelBranch],
    on_error: ParallelOnError,
) -> ParallelGroupOutcome {
    let started = Instant::now();
    let mut steps = Vec::new();
    let mut branch_outcomes = Vec::new();
    let mut ok = true;

    for branch in branches {
        if !is_parallel_branch_safe(&branch.call, &branch.args) {
            let detail = format!("not parallel-safe: {}", branch.call);
            steps.push(error_step(step_index, detail.clone()));
            return ParallelGroupOutcome {
                ok: false,
                steps,
                wall_us: started.elapsed().as_micros(),
                branches: branch_outcomes,
            };
        }
    }

    let readonly: Vec<_> = branches
        .iter()
        .filter(|b| is_readonly_parallel(b))
        .collect();
    let world_staging: Vec<_> = branches
        .iter()
        .filter(|b| is_world_staging(&b.call, &b.args))
        .collect();

    if !readonly.is_empty() {
        let readonly_outcome =
            execute_readonly_parallel(session, group_index, step_index, &readonly, on_error);
        ok &= readonly_outcome.ok;
        steps.extend(readonly_outcome.steps);
        branch_outcomes.extend(readonly_outcome.branches);
        if !readonly_outcome.ok && on_error == ParallelOnError::FailFast {
            return ParallelGroupOutcome {
                ok,
                steps,
                wall_us: started.elapsed().as_micros(),
                branches: branch_outcomes,
            };
        }
    }

    for (bi, branch) in world_staging.iter().enumerate() {
        let arg = branch.args.get("arg").and_then(serde_json::Value::as_str);
        let (ack, branch_ok, detail) = session.execute('W', arg);
        let payload_key = format!("codemode/p/{group_index}/{}/world", branch.id);
        let payload = detail.clone().unwrap_or_default().into_bytes();
        session.record_codemode_materialization(&payload);
        session.recovery.put_key(&payload_key, &payload);
        steps.push(StepLog {
            index: step_index,
            method: "fs.world".to_string(),
            ack: ack.clone(),
            ok: branch_ok,
            recovery_key: payload_key.clone(),
            detail: Some(format!(
                "parallel group={group_index} branch={} slot={bi} ref={payload_key} staged_world",
                branch.id
            )),
            parallel: true,
        });
        branch_outcomes.push(ParallelBranchOutcome {
            branch_id: branch.id.clone(),
            method: "fs.world".to_string(),
            payload_key,
            payload,
            ok: branch_ok,
        });
        ok &= branch_ok;
        if !branch_ok && on_error == ParallelOnError::FailFast {
            break;
        }
    }

    ParallelGroupOutcome {
        ok,
        steps,
        wall_us: started.elapsed().as_micros(),
        branches: branch_outcomes,
    }
}

fn is_readonly_parallel(branch: &ParallelBranch) -> bool {
    is_parallel_branch_safe(&branch.call, &branch.args)
        && !is_world_staging(&branch.call, &branch.args)
}

fn is_world_staging(call: &str, args: &serde_json::Value) -> bool {
    call == "fs.world"
        && args
            .get("arg")
            .and_then(serde_json::Value::as_str)
            .is_some_and(crate::core::world_arg_is_staging)
}

fn execute_readonly_parallel(
    session: &mut FSZeroSession,
    group_index: usize,
    step_index: usize,
    branches: &[&ParallelBranch],
    on_error: ParallelOnError,
) -> ParallelGroupOutcome {
    if branches.iter().any(|branch| branch.call == "fs.search") {
        if let Err(e) = session.ensure_index_built() {
            return fail_group(step_index, format!("index permit busy: {e}"));
        }
    }
    let Some(ctx) = ParallelReadContext::capture(session) else {
        return fail_group(step_index, "no root for parallel group".to_string());
    };

    // R-003 / fszero-tucs: workers never share RecoveryStore. Search runs on
    // the main session thread (may overlap worker ls/read/stat); other
    // read-only branches stay in thread::scope with an owned root context.
    //
    // INVARIANT: every join happens before this function returns; workers only
    // call execute_parallel_branch (no recovery). Main may mutate session.recovery
    // during the scope via execute_search_branch.
    let mut works = Vec::with_capacity(branches.len());
    let wall_start = std::time::Instant::now();
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(branches.len());
        for branch in branches.iter().filter(|b| b.call != "fs.search") {
            let branch_id = branch.id.clone();
            let call = branch.call.clone();
            let args = branch.args.clone();
            let ctx = ctx.clone();
            handles.push(scope.spawn(move || {
                catch_unwind(AssertUnwindSafe(|| {
                    execute_parallel_branch(&ctx, group_index, &branch_id, &call, &args)
                }))
                .unwrap_or_else(|_| panic_branch_work(&branch_id, &call))
            }));
        }
        for branch in branches.iter().filter(|b| b.call == "fs.search") {
            let branch_id = branch.id.clone();
            let args = branch.args.clone();
            works.push(
                catch_unwind(AssertUnwindSafe(|| {
                    execute_search_branch(session, group_index, &branch_id, &args)
                }))
                .unwrap_or_else(|_| panic_branch_work(&branch_id, "fs.search")),
            );
        }
        for handle in handles {
            works.push(handle.join().expect("parallel worker thread join"));
        }
    });
    let wall_us = wall_start.elapsed().as_micros();

    let mut steps = Vec::new();
    let mut branch_outcomes = Vec::new();
    let mut ok = true;
    for (bi, branch) in branches.iter().enumerate() {
        let Some(work) = works.iter().find(|w| w.branch_id == branch.id) else {
            let detail = format!("parallel branch '{}' failed to complete", branch.id);
            steps.push(error_step(step_index, detail));
            ok = false;
            if on_error == ParallelOnError::FailFast {
                break;
            }
            continue;
        };
        session.record_codemode_materialization(&work.payload);
        session.recovery.put_key(&work.payload_key, &work.payload);
        if let Ok(text) = std::str::from_utf8(&work.payload) {
            if let Some((dimension, cap, scanned)) = parse_budget_message(text) {
                let op = OpCode::from_char(work.op)
                    .map(|o| match o {
                        OpCode::Search => "S",
                        OpCode::Compound => "C",
                        OpCode::World => "W",
                        _ => "X",
                    })
                    .unwrap_or("X");
                session.store_budget_evidence(op, dimension, cap, scanned);
            }
        }
        session.op_count += 1;
        session.version += 1;
        let op = OpCode::from_char(work.op).unwrap_or(OpCode::Expand);
        let ack = if work.ok {
            visible_ack(op, Some(session.op_count))
        } else {
            kernel_visible_error(&work.kernel_message, op)
        };
        steps.push(StepLog {
            index: step_index,
            method: work.method.clone(),
            ack,
            ok: work.ok,
            recovery_key: work.payload_key.clone(),
            detail: Some(format!(
                "parallel group={group_index} branch={} slot={bi} ref={}",
                work.branch_id, work.payload_key
            )),
            parallel: true,
        });
        branch_outcomes.push(ParallelBranchOutcome {
            branch_id: work.branch_id.clone(),
            method: work.method.clone(),
            payload_key: work.payload_key.clone(),
            payload: work.payload.clone(),
            ok: work.ok,
        });
        ok &= work.ok;
        if !work.ok && on_error == ParallelOnError::FailFast {
            break;
        }
    }

    ParallelGroupOutcome {
        ok,
        steps,
        wall_us,
        branches: branch_outcomes,
    }
}

fn panic_branch_work(branch_id: &str, method: &str) -> ParallelBranchWork {
    ParallelBranchWork {
        branch_id: branch_id.to_string(),
        method: method.to_string(),
        op: 'X',
        kernel_message: "parallel branch panicked".to_string(),
        ok: false,
        payload_key: super::runtime::ERROR_REF.to_string(),
        payload: b"parallel branch panicked".to_vec(),
    }
}

fn error_step(index: usize, detail: String) -> StepLog {
    StepLog {
        index,
        method: "parallel".to_string(),
        ack: "X0".to_string(),
        ok: false,
        recovery_key: super::runtime::ERROR_REF.to_string(),
        detail: Some(detail),
        parallel: true,
    }
}

fn fail_group(step_index: usize, detail: String) -> ParallelGroupOutcome {
    ParallelGroupOutcome {
        ok: false,
        steps: vec![error_step(step_index, detail)],
        wall_us: 0,
        branches: Vec::new(),
    }
}

pub fn invoke_call(
    connector: &mut super::connector::FsConnector<'_>,
    call: &str,
    args: &serde_json::Value,
) -> super::connector::FsStep {
    connector.invoke(call, args)
}
