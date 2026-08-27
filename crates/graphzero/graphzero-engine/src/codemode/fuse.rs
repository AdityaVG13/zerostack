//! Measured CodeMode overhead removal and hot-path fusion (graphzero-o2uq.9).
//!
//! Optimizations (each has a kill test):
//! 1. **Binding table OnceLock** — registry-derived bindings built once per process
//!    for a given contract digest (no per-call table rebuild).
//! 2. **Fused multi-op dispatch** — N read-only domain ops share one `EngineContext`
//!    / snapshot handle; only the final plan path serializes (callers keep native
//!    `DomainResult` values in-process).
//! 3. **No intermediate MCP envelopes** between fused steps.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use crate::dispatcher::{AdapterKind, EngineContext, dispatch_profiled};
use crate::operation_abi::{DomainError, DomainErrorKind, DomainResult, contract_digest_hex};

use super::bindings::{BindingTable, binding_table_from_registry};

static BINDING_TABLE: OnceLock<BindingTable> = OnceLock::new();
/// Counts binding-table materializations (kill test for cache).
static BINDING_TABLE_BUILDS: AtomicU64 = AtomicU64::new(0);
/// Counts EngineContext constructions via fused helpers (for profiles).
static FUSED_CONTEXT_BUILDS: AtomicU64 = AtomicU64::new(0);

/// Cached binding table keyed implicitly by process-wide contract digest.
pub fn cached_binding_table() -> &'static BindingTable {
    BINDING_TABLE.get_or_init(|| {
        BINDING_TABLE_BUILDS.fetch_add(1, Ordering::SeqCst);
        let table = binding_table_from_registry();
        // Digest must match live registry (stale OnceLock would be a process upgrade bug).
        debug_assert_eq!(table.contract_digest, contract_digest_hex());
        table
    })
}

pub fn binding_table_build_count() -> u64 {
    BINDING_TABLE_BUILDS.load(Ordering::SeqCst)
}

pub fn fused_context_build_count() -> u64 {
    FUSED_CONTEXT_BUILDS.load(Ordering::SeqCst)
}

/// One step in a fused multi-op plan (native args; no MCP envelope).
#[derive(Clone, Debug)]
pub struct FusedStep {
    pub op: String,
    pub args: Value,
}

/// Result of a fused multi-op execution (single final serialization by caller).
#[derive(Clone, Debug)]
pub struct FusedOutcome {
    pub results: Vec<Result<DomainResult, DomainError>>,
    /// Wall ns for the whole fused batch.
    pub wall_ns: u128,
    /// Sum of per-op dispatcher wall ns.
    pub dispatcher_wall_ns_sum: u128,
    /// Number of EngineContext constructions (must be 1 for fused path).
    pub context_builds: u32,
}

/// Fuse N domain ops over **one** EngineContext (warm snapshot path).
///
/// Read-only ops run sequentially on the shared context. Mutation ops are allowed
/// but remain ordered on the same lane (no parallel mutation). Does not create a
/// JavaScript runtime or re-enter MCP.
pub fn fused_dispatch(
    repo_root: std::path::PathBuf,
    store_root: std::path::PathBuf,
    steps: &[FusedStep],
) -> FusedOutcome {
    let t0 = std::time::Instant::now();
    FUSED_CONTEXT_BUILDS.fetch_add(1, Ordering::SeqCst);
    let ctx = EngineContext::for_paths(repo_root, store_root, AdapterKind::CodeMode);
    let mut results = Vec::with_capacity(steps.len());
    let mut dispatcher_sum = 0u128;
    for step in steps {
        // Reject nested planner ops inside fused batch.
        if is_planner_meta(&step.op) {
            results.push(Err(DomainError::new(
                DomainErrorKind::Policy,
                format!("fused path refuses nested planner op '{}'", step.op),
            )
            .with_op(&step.op)));
            continue;
        }
        let (out, profile) = dispatch_profiled(&ctx, &step.op, &step.args);
        dispatcher_sum += profile.wall_ns;
        results.push(out);
    }
    FusedOutcome {
        results,
        wall_ns: t0.elapsed().as_nanos(),
        dispatcher_wall_ns_sum: dispatcher_sum,
        context_builds: 1,
    }
}

/// Unfused baseline: N separate contexts (the work fusion removes).
pub fn unfused_dispatch(
    repo_root: std::path::PathBuf,
    store_root: std::path::PathBuf,
    steps: &[FusedStep],
) -> FusedOutcome {
    let t0 = std::time::Instant::now();
    let mut results = Vec::with_capacity(steps.len());
    let mut dispatcher_sum = 0u128;
    let mut builds = 0u32;
    for step in steps {
        FUSED_CONTEXT_BUILDS.fetch_add(1, Ordering::SeqCst);
        builds += 1;
        let ctx =
            EngineContext::for_paths(repo_root.clone(), store_root.clone(), AdapterKind::CodeMode);
        let (out, profile) = dispatch_profiled(&ctx, &step.op, &step.args);
        dispatcher_sum += profile.wall_ns;
        results.push(out);
    }
    FusedOutcome {
        results,
        wall_ns: t0.elapsed().as_nanos(),
        dispatcher_wall_ns_sum: dispatcher_sum,
        context_builds: builds,
    }
}

fn is_planner_meta(op: &str) -> bool {
    matches!(
        op,
        "execute_code"
            | "gz_execute_code"
            | "codemode_search"
            | "gz_codemode_search"
            | "codemode_describe"
            | "gz_codemode_describe"
    )
}

/// Before/after profile for the context-build optimization.
#[derive(Clone, Debug, serde::Serialize)]
pub struct FusionProfile {
    pub n: usize,
    pub fused_context_builds: u32,
    pub unfused_context_builds: u32,
    pub fused_wall_ns: u128,
    pub unfused_wall_ns: u128,
    pub fused_dispatcher_ns: u128,
    pub unfused_dispatcher_ns: u128,
}

pub fn profile_fusion(
    repo: std::path::PathBuf,
    store: std::path::PathBuf,
    n: usize,
) -> FusionProfile {
    let steps: Vec<FusedStep> = (0..n)
        .map(|_| FusedStep {
            op: "search".into(),
            args: serde_json::json!({"query": "alpha", "budget": 1}),
        })
        .collect();
    let fused = fused_dispatch(repo.clone(), store.clone(), &steps);
    let unfused = unfused_dispatch(repo, store, &steps);
    FusionProfile {
        n,
        fused_context_builds: fused.context_builds,
        unfused_context_builds: unfused.context_builds,
        fused_wall_ns: fused.wall_ns,
        unfused_wall_ns: unfused.wall_ns,
        fused_dispatcher_ns: fused.dispatcher_wall_ns_sum,
        unfused_dispatcher_ns: unfused.dispatcher_wall_ns_sum,
    }
}

/// Semantic parity: fused and unfused produce the same ok/err kinds and ops.
pub fn fused_unfused_semantic_parity(
    repo: std::path::PathBuf,
    store: std::path::PathBuf,
    steps: &[FusedStep],
) -> bool {
    let fused = fused_dispatch(repo.clone(), store.clone(), steps);
    let unfused = unfused_dispatch(repo, store, steps);
    if fused.results.len() != unfused.results.len() {
        return false;
    }
    for (a, b) in fused.results.iter().zip(unfused.results.iter()) {
        match (a, b) {
            (Ok(ra), Ok(rb)) => {
                if ra.op != rb.op {
                    return false;
                }
            }
            (Err(ea), Err(eb)) => {
                if ea.kind != eb.kind {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}
