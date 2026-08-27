//! Typed in-process domain engine dispatcher (graphzero-o2uq.2).
//!
//! All user-facing adapters (lean FastMCP, CodeMode single-op bindings, CLI
//! compatibility paths, and the private raw worker) call [`dispatch`] directly.
//! This module must never import FastMCP, MCP transport types, or the CodeMode
//! Aggregate-host JavaScript policy stays at adapter edges; serialization and plan execution remain native.

mod context;
mod execute;
mod profile;

pub use context::{AdapterKind, CancellationToken, EngineContext};
pub use execute::{DOMAIN_EXECUTABLE_OPS, SURFACE_META_OPS, dispatch, dispatch_resolved};
pub use profile::{
    DispatchPhaseTimings, DispatchProfile, dispatch_phase_timing_enabled, dispatch_profiled,
    take_dispatch_phase_timings,
};

use crate::operation_abi::{DomainError, DomainResult};

/// Outcome of a single domain operation (success or typed error).
pub type DispatchOutcome = Result<DomainResult, DomainError>;

/// Private raw worker entry (same semantics as FastMCP/CodeMode single-op).
///
/// Thin domain dispatch alias used once a session has completed the
/// `zerostack.surface` handshake (see [`crate::surface_handshake`]).
/// Callers that need digest gating should use
/// [`crate::surface_handshake::PrivateRawWorker`] or
/// [`crate::surface_handshake::private_worker_dispatch_checked`].
///
/// This path never plans, starts a sandbox, or re-enters FastMCP.
pub fn private_worker_dispatch(
    ctx: &EngineContext,
    op: &str,
    args: &serde_json::Value,
) -> DispatchOutcome {
    debug_assert_eq!(ctx.adapter, AdapterKind::PrivateWorker);
    dispatch(ctx, op, args)
}
