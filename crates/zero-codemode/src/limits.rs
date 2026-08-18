use std::fmt;
use std::time::Duration;
/// Default CodeMode JS/runtime wall in milliseconds.
///
/// Received from `FSZero/crates/fs-zero/src/codemode/limits.rs` on
/// 2026-08-17 so every engine adapter shares one host-owned wall policy.
pub const MAX_WALL_MS: u64 = 2_000;

/// Environment variables that may override the shared CodeMode wall.
pub const CODEMODE_WALL_MS_ENVS: &[&str] = &[
    "FSZERO_CODEMODE_WALL_MS",
    "ZEROSTACK_CODEMODE_WALL_MS",
    "TOKENZERO_CODEMODE_WALL_MS",
    "GRAPHZERO_CODEMODE_MAX_WALL_MS",
];

/// Effective wall with a 1ms floor; malformed values fall through.
pub fn effective_max_wall_ms() -> u64 {
    for key in CODEMODE_WALL_MS_ENVS {
        if let Ok(value) = std::env::var(key) {
            if let Ok(parsed) = value.trim().parse::<u64>() {
                return parsed.max(1);
            }
        }
    }
    MAX_WALL_MS
}

/// Resource limits applied to one host execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostLimits {
    pub memory_bytes: usize,
    pub stack_bytes: usize,
    pub wall_timeout: Duration,
    pub instruction_budget: u64,
    pub microtask_ceiling: usize,
    /// Maximum connector calls admitted concurrently. This bounds queueing,
    /// not the total logical operations an execution may perform.
    pub max_inflight_connector_calls: usize,
    /// Maximum connector calls admitted per execution in total. Every
    /// admitted dispatch (direct `zero.*`, `z.invoke`, and each
    /// `z.parallel` fan-out spec) counts; the next dispatch past the bound
    /// fails typed. This is the total-call dimension of the budget vector;
    /// the in-flight bound above is the concurrency dimension.
    pub max_connector_calls: u64,
    pub max_plan_bytes: usize,
    pub max_json_bytes: usize,
}

impl HostLimits {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        memory_bytes: usize,
        stack_bytes: usize,
        wall_timeout: Duration,
        instruction_budget: u64,
        microtask_ceiling: usize,
        max_inflight_connector_calls: usize,
        max_connector_calls: u64,
        max_plan_bytes: usize,
        max_json_bytes: usize,
    ) -> Result<Self, LimitError> {
        let limits = Self {
            memory_bytes,
            stack_bytes,
            wall_timeout,
            instruction_budget,
            microtask_ceiling,
            max_inflight_connector_calls,
            max_connector_calls,
            max_plan_bytes,
            max_json_bytes,
        };
        limits.validate()?;
        Ok(limits)
    }

    pub fn validate(&self) -> Result<(), LimitError> {
        if self.memory_bytes == 0 {
            return Err(LimitError::Zero("memory_bytes"));
        }
        if self.stack_bytes == 0 {
            return Err(LimitError::Zero("stack_bytes"));
        }
        if self.wall_timeout.is_zero() {
            return Err(LimitError::Zero("wall_timeout"));
        }
        if self.instruction_budget == 0 {
            return Err(LimitError::Zero("instruction_budget"));
        }
        if self.microtask_ceiling == 0 {
            return Err(LimitError::Zero("microtask_ceiling"));
        }
        if self.max_inflight_connector_calls == 0 {
            return Err(LimitError::Zero("max_inflight_connector_calls"));
        }
        if self.max_connector_calls == 0 {
            return Err(LimitError::Zero("max_connector_calls"));
        }
        if self.max_plan_bytes == 0 {
            return Err(LimitError::Zero("max_plan_bytes"));
        }
        if self.max_json_bytes == 0 {
            return Err(LimitError::Zero("max_json_bytes"));
        }
        Ok(())
    }
}

impl Default for HostLimits {
    fn default() -> Self {
        Self {
            memory_bytes: 64 * 1024 * 1024,
            stack_bytes: 512 * 1024,
            wall_timeout: Duration::from_millis(effective_max_wall_ms()),
            instruction_budget: 100_000,
            microtask_ceiling: 1_024,
            max_inflight_connector_calls: crate::MAX_INFLIGHT_CONNECTOR_CALLS,
            max_connector_calls: crate::MAX_INFLIGHT_CONNECTOR_CALLS as u64 * 16,
            max_plan_bytes: 256 * 1024,
            max_json_bytes: 1024 * 1024,
        }
    }
}

/// One named output/wall arrangement. These are **not** one product-wide
/// ceiling: CONTRACT echo, `HostLimits::default`, the zsx session visible
/// budget, and the zsx-core connector host are four different laws.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputWallArrangement {
    pub name: &'static str,
    pub memory_bytes: usize,
    pub wall_ms: u64,
    pub output_bytes: usize,
    pub owner: &'static str,
}

/// Authority decision B (honest Stop): four arrangement-local budgets.
/// Adding a fifth constructor requires a fifth row here.
pub const OUTPUT_WALL_ARRANGEMENTS: &[OutputWallArrangement] = &[
    OutputWallArrangement {
        name: "capability-manifest-echo",
        memory_bytes: 32 * 1024 * 1024,
        wall_ms: 250,
        output_bytes: 64 * 1024,
        owner: "conformance/CONTRACT.md §3 limits echo",
    },
    OutputWallArrangement {
        name: "host-limits-default",
        memory_bytes: 64 * 1024 * 1024,
        wall_ms: 2_000,
        output_bytes: 1024 * 1024,
        owner: "HostLimits::default max_json_bytes / wall / memory",
    },
    OutputWallArrangement {
        name: "zsx-session-visible",
        memory_bytes: 0,
        wall_ms: 0,
        output_bytes: 12 * 1024,
        owner: "zsx-core SESSION_VISIBLE_RESULT_BYTES",
    },
    OutputWallArrangement {
        name: "zsx-connector-host",
        memory_bytes: 128 * 1024 * 1024,
        wall_ms: 30_000,
        output_bytes: 16 * 1024 * 1024,
        owner: "zsx-core connector::host_limits",
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitError {
    Zero(&'static str),
}

impl fmt::Display for LimitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero(field) => write!(f, "{field} must be nonzero"),
        }
    }
}
impl std::error::Error for LimitError {}
