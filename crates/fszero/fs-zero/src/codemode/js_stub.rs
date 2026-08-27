//! Stub JS sandbox when `surface-codemode` is not compiled (fszero-mcp-only).
//!
//! Recipe and JSON-DAG plans still run; pure JS plans fail closed with a
//! precise diagnostic instead of linking the hub interpreter.

use super::host::ContractError;
use super::runtime::RuntimeOutcome;
use crate::core::FSZeroSession;

/// No interpreter linked -- creation count is always zero on MCP-only artifacts.
pub fn sandbox_runtime_creation_count() -> u64 {
    0
}

/// Test helper (no-op when the interpreter is not linked).
pub fn reset_sandbox_runtime_creation_count_for_tests() {}

/// Test helper (no-op when the interpreter is not linked).
pub fn inject_host_boundary_panic_for_test(_enable: bool) {}

pub fn execute_js_plan(_session: &mut FSZeroSession, _code: &str) -> RuntimeOutcome {
    let message = "CodeMode restricted interpreter was not compiled into this artifact \
(missing feature surface-codemode). Install fszero-codemode, or use a recipe / \
JSON-DAG plan. fszero-mcp never embeds the interpreter runtime.";
    RuntimeOutcome::failed(
        "js_unavailable",
        message,
        ContractError::validation(message),
    )
}
