use std::fmt;
use std::time::Duration;

/// Resource limits applied to one host execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostLimits {
    pub memory_bytes: usize,
    pub stack_bytes: usize,
    pub wall_timeout: Duration,
    pub instruction_budget: u64,
    pub microtask_ceiling: usize,
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
        max_plan_bytes: usize,
        max_json_bytes: usize,
    ) -> Result<Self, LimitError> {
        let limits = Self {
            memory_bytes,
            stack_bytes,
            wall_timeout,
            instruction_budget,
            microtask_ceiling,
            max_plan_bytes,
            max_json_bytes,
        };
        limits.validate()?;
        Ok(limits)
    }

    pub fn validate(&self) -> Result<(), LimitError> {
        if self.memory_bytes == 0 { return Err(LimitError::Zero("memory_bytes")); }
        if self.stack_bytes == 0 { return Err(LimitError::Zero("stack_bytes")); }
        if self.wall_timeout.is_zero() { return Err(LimitError::Zero("wall_timeout")); }
        if self.instruction_budget == 0 { return Err(LimitError::Zero("instruction_budget")); }
        if self.microtask_ceiling == 0 { return Err(LimitError::Zero("microtask_ceiling")); }
        if self.max_plan_bytes == 0 { return Err(LimitError::Zero("max_plan_bytes")); }
        if self.max_json_bytes == 0 { return Err(LimitError::Zero("max_json_bytes")); }
        Ok(())
    }
}

impl Default for HostLimits {
    fn default() -> Self {
        Self {
            memory_bytes: 64 * 1024 * 1024,
            stack_bytes: 512 * 1024,
            wall_timeout: Duration::from_secs(2),
            instruction_budget: 100_000,
            microtask_ceiling: 1_024,
            max_plan_bytes: 256 * 1024,
            max_json_bytes: 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitError { Zero(&'static str) }

impl fmt::Display for LimitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self { Self::Zero(field) => write!(f, "{field} must be nonzero") }
    }
}
impl std::error::Error for LimitError {}
