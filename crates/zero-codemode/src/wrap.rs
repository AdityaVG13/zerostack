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

/// Wrapper source; the __SOURCE__ marker is replaced by the encoded plan.
///
/// A result that cannot be serialized degrades to a best-effort projection
/// instead of failing, so a plan's completed side effects are never masked by
/// a reporting failure.
const WRAPPER: &str = r#"(async () => {
"use strict";
const __source = __SOURCE__;
const __AsyncFunction = Object.getPrototypeOf(async function(){}).constructor;
const __plan = new __AsyncFunction("zero", '"use strict";' + String.fromCharCode(10) + __source);
const __value = await __plan(globalThis.zero);
const __read = (target, key) => {
  try {
    return { ok: true, value: target[key] };
  } catch (error) {
    return { ok: false, value: undefined };
  }
};
const __pushRef = (refs, value) => {
  if (refs.indexOf(value) === -1) refs.push(value);
};
const __collectRefs = (target, refs) => {
  if (target === null || typeof target !== "object") return;
  for (const key of ["ref", "refs"]) {
    const read = __read(target, key);
    if (!read.ok) continue;
    const value = read.value;
    if (typeof value === "string") __pushRef(refs, value);
    else if (Array.isArray(value)) {
      for (const item of value) if (typeof item === "string") __pushRef(refs, item);
    }
  }
};
const __degrade = (value) => {
  const refs = [];
  let projection;
  if (value === null || typeof value !== "object") {
    projection = String(value);
  } else {
    projection = {};
    __collectRefs(value, refs);
    let keys = [];
    try {
      keys = Object.keys(value);
    } catch (error) {
      keys = [];
    }
    for (const key of keys) {
      const read = __read(value, key);
      if (!read.ok) {
        projection[key] = "[unreadable]";
        continue;
      }
      const field = read.value;
      const kind = typeof field;
      if (field === null || kind === "boolean" || kind === "number" || kind === "string") {
        projection[key] = field;
        continue;
      }
      __collectRefs(field, refs);
      try {
        projection[key] = String(field);
      } catch (error) {
        projection[key] = "[unreadable]";
      }
    }
  }
  return { serialization_degraded: true, result: projection, refs };
};
try {
  const __json = JSON.stringify(__value);
  if (__json !== undefined) return __json;
} catch (error) {}
return JSON.stringify(__degrade(__value));
})()"#;

/// Wraps a plan as data, never as wrapper source, and extracts one JSON result.
pub fn wrap_plan(plan: &str, max_bytes: usize) -> Result<String, PlanError> {
    validate_plan(plan, max_bytes)?;
    let encoded =
        serde_json::to_string(plan).map_err(|error| PlanError::Encoding(error.to_string()))?;
    Ok(WRAPPER.replace("__SOURCE__", &encoded))
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
