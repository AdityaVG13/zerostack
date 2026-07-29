use std::fmt;

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

/// Wraps a plan as data, never as wrapper source, and extracts one JSON result.
pub fn wrap_plan(plan: &str, max_bytes: usize) -> Result<String, PlanError> {
    validate_plan(plan, max_bytes)?;
    let encoded =
        serde_json::to_string(plan).map_err(|error| PlanError::Encoding(error.to_string()))?;
    Ok(format!(
        r#"(async () => {{
"use strict";
const __source = {encoded};
const __AsyncFunction = Object.getPrototypeOf(async function(){{}}).constructor;
const __plan = new __AsyncFunction("zero", "\"use strict\";\n" + __source);
const __value = await __plan(globalThis.zero);
const __json = JSON.stringify(__value);
if (__json === undefined) throw new TypeError("plan result is not JSON-serializable");
return __json;
}})()"#
    ))
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
