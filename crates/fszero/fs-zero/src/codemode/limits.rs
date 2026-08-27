//! CodeMode plan/runtime limits — echoed values must be enforced.

pub const MAX_PLAN_STEPS: usize = 64;
/// In-plan JS/parallel fanout. Cap at 2 so concurrent CodeMode sessions
/// share the family-wide analysis slot budget without stacking cores.
pub const MAX_PARALLEL_WIDTH: usize = 2;
pub const MAX_LOGICAL_OPS: u32 = 1000;
pub const MAX_PHYSICAL_OPS: u32 = 256;
// The generic wall policy is host-owned since the hub receive (ZeroStack
// ceff69c): `zero-codemode` limits.rs is canonical for MAX_WALL_MS /
// CODEMODE_WALL_MS_ENVS / effective_max_wall_ms. Surface builds re-export
// that table; MCP-only builds (no `surface-codemode`, so `zero-codemode` is
// not linked) keep the same values as local fallbacks.
#[cfg(feature = "surface-codemode")]
pub use zero_codemode::{CODEMODE_WALL_MS_ENVS, MAX_WALL_MS, effective_max_wall_ms};

/// Default per-plan JS/runtime wall (ms). Raised from 250 so first-try agent
/// plans are not killed before useful work (R-022 / fszero-ic6k.6).
/// Override with `FSZERO_CODEMODE_WALL_MS` (or family aliases).
#[cfg(not(feature = "surface-codemode"))]
pub const MAX_WALL_MS: u64 = 2_000;
/// Env names that raise the codemode JS/runtime wall (not host permit wall).
#[cfg(not(feature = "surface-codemode"))]
pub const CODEMODE_WALL_MS_ENVS: &[&str] = &[
    "FSZERO_CODEMODE_WALL_MS",
    "ZEROSTACK_CODEMODE_WALL_MS",
    "TOKENZERO_CODEMODE_WALL_MS",
];
/// Effective wall: env override when set and parseable, else [`MAX_WALL_MS`].
/// Floor 1ms so a mis-set 0 does not disable the deadline.
#[cfg(not(feature = "surface-codemode"))]
pub fn effective_max_wall_ms() -> u64 {
    for key in CODEMODE_WALL_MS_ENVS {
        if let Ok(v) = std::env::var(key) {
            if let Ok(n) = v.trim().parse::<u64>() {
                return n.max(1);
            }
        }
    }
    MAX_WALL_MS
}
pub const MAX_MICROTASKS: u32 = 4096;
pub const MAX_MEMORY_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_RESULT_REF_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_REFS_EMITTED: usize = 256;
pub const MAX_CODE_BYTES: usize = 64 * 1024;
