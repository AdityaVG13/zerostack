//! CodeMode — native `fs.*` runtime on the kernel.
//!
//! The hub-backed restricted interpreter is linked only when feature
//! `surface-codemode` is enabled. MCP-only artifacts compile a stub that fails
//! closed for JS plans while keeping recipe/JSON-DAG paths in the shared core.

mod api;
mod bindings;
mod connector;
mod discovery;
mod host;
#[cfg(feature = "surface-codemode")]
mod js;
#[cfg(not(feature = "surface-codemode"))]
mod js_stub;
mod name_rank;
#[cfg(not(feature = "surface-codemode"))]
use js_stub as js;
mod limits;
mod parallel;
mod plan;
mod program;
mod recipes;
mod runtime;
mod transaction;
mod world_parse;
mod zero_result;

pub use api::{METHODS, MethodDef, describe as api_describe, is_kernel_method, search_methods};
pub use connector::{FsConnector, FsStep};
pub use discovery::{
    DESCRIBE_REF, SEARCH_REF, describe_signature, discovery_describe, discovery_search,
};
pub use host::{
    ContractError, RESPONSE_REF, TELEMETRY_REF, ack_with_refs, classify_error,
    codemode_tool_refs_for_describe, codemode_tool_refs_for_plan, codemode_tool_refs_for_search,
    finish, finish_error, payload_tool_result, payload_wire_value, payload_wire_value_with_session,
    plan_tool_result, reset_ok_ring_tick_for_tests,
};
pub use js::{
    execute_js_plan, inject_host_boundary_panic_for_test,
    reset_sandbox_runtime_creation_count_for_tests, sandbox_runtime_creation_count,
};
pub use limits::{
    CODEMODE_WALL_MS_ENVS, MAX_CODE_BYTES, MAX_LOGICAL_OPS, MAX_MEMORY_BYTES, MAX_MICROTASKS,
    MAX_OUTPUT_BYTES, MAX_PARALLEL_WIDTH, MAX_PHYSICAL_OPS, MAX_PLAN_STEPS, MAX_REFS_EMITTED,
    MAX_RESULT_REF_BYTES, MAX_WALL_MS, effective_max_wall_ms,
};
pub use plan::{execute_plan, looks_like_js_plan};
pub use program::{
    ParallelBranch, ParallelOnError, PlanStep, Program, Step, TransactionMode, bound_read_step,
    call_step, parallel_branch, parallel_step, parallel_step_with_needs, parse_program,
    validate_program,
};
pub use recipes::{explore_program, impact_program, refactor_program, try_recipe_with_session};
pub use runtime::fusion_eligible_methods;
pub use runtime::{ERROR_REF, RESULT_REF, RuntimeOutcome, STEPS_REF, StepLog, execute_program};
pub use transaction::{TransactionJournal, program_has_mutations};
pub use zero_result::{wrong_accessor_message, zero_result_from_fs_step, zero_result_to_wire};
