use std::fmt;
use std::time::Duration;

const DEFAULT_WALL_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostLimits {
    pub memory_bytes: usize,
    pub stack_bytes: usize,
    pub wall_timeout: Duration,
    pub instruction_budget: u64,
    pub microtask_ceiling: usize,
    pub max_inflight_connector_calls: usize,
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
        for (name, zero) in [
            ("memory_bytes", self.memory_bytes == 0),
            ("stack_bytes", self.stack_bytes == 0),
            ("instruction_budget", self.instruction_budget == 0),
            ("microtask_ceiling", self.microtask_ceiling == 0),
            (
                "max_inflight_connector_calls",
                self.max_inflight_connector_calls == 0,
            ),
            ("max_connector_calls", self.max_connector_calls == 0),
            ("max_plan_bytes", self.max_plan_bytes == 0),
            ("max_json_bytes", self.max_json_bytes == 0),
        ] {
            if zero {
                return Err(LimitError::Zero(name));
            }
        }
        if self.wall_timeout.is_zero() {
            return Err(LimitError::Zero("wall_timeout"));
        }
        Ok(())
    }
}

impl Default for HostLimits {
    fn default() -> Self {
        Self {
            memory_bytes: 64 * 1024 * 1024,
            stack_bytes: 512 * 1024,
            wall_timeout: Duration::from_millis(DEFAULT_WALL_MS),
            instruction_budget: 100_000,
            microtask_ceiling: 1_024,
            max_inflight_connector_calls: crate::MAX_INFLIGHT_CONNECTOR_CALLS,
            max_connector_calls: crate::MAX_INFLIGHT_CONNECTOR_CALLS as u64 * 16,
            max_plan_bytes: 256 * 1024,
            max_json_bytes: 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitError {
    Zero(&'static str),
}

impl fmt::Display for LimitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero(field) => write!(formatter, "{field} must be nonzero"),
        }
    }
}

impl std::error::Error for LimitError {}
