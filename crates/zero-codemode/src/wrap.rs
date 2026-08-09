use std::fmt;

/// Validate source before handing it to the restricted AST interpreter.
pub fn validate_plan(plan: &str, max_bytes: usize) -> Result<(), PlanError> {
    if plan.trim().is_empty() {
        return Err(PlanError::Empty);
    }
    if plan.len() > max_bytes {
        return Err(PlanError::TooLarge {
            actual: plan.len(),
            maximum: max_bytes,
        });
    }
    if plan.contains('\0') {
        return Err(PlanError::Nul);
    }
    Ok(())
}

/// Compatibility helper retained for callers that used the old pre-runtime
/// validation API. It returns validated source unchanged; execution never
/// evaluates this string as host code.
pub fn wrap_plan(plan: &str, max_bytes: usize) -> Result<String, PlanError> {
    validate_plan(plan, max_bytes)?;
    Ok(plan.to_owned())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanError {
    Empty,
    TooLarge { actual: usize, maximum: usize },
    Nul,
    Encoding(String),
}
impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("plan is empty"),
            Self::TooLarge { actual, maximum } => {
                write!(f, "plan is {actual} bytes; maximum is {maximum}")
            }
            Self::Nul => f.write_str("plan contains a NUL byte"),
            Self::Encoding(message) => write!(f, "plan encoding failed: {message}"),
        }
    }
}
impl std::error::Error for PlanError {}
