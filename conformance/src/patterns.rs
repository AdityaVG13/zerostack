//! Contract ref and execution-id patterns (G2).

use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

static EXECUTION_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^cm://exec/[0-9]+-[0-9a-f]{12}$").expect("execution_id regex"));

/// `^cm://exec/<unix_millis>-<sha256_hex[..12]>`
pub fn execution_id_re() -> &'static Regex {
    &EXECUTION_ID
}

pub fn substrate_ref_re(ns: &str) -> Regex {
    let pattern = format!(
        r"^{ns}://(blob/[0-9a-f]{{64}}|codemode/execution/[^/]+/(code|steps|telemetry|result|error))$"
    );
    Regex::new(&pattern).unwrap_or_else(|e| panic!("invalid ns for ref regex: {e}"))
}

/// Walk JSON and collect every string that looks like a substrate ref or execution id.
pub fn collect_refs_in_value(ns: &str, value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => {
            if s.starts_with(&format!("{ns}://")) || s.starts_with("cm://exec/") {
                out.push(s.clone());
            }
        }
        Value::Array(a) => {
            for v in a {
                collect_refs_in_value(ns, v, out);
            }
        }
        Value::Object(m) => {
            for v in m.values() {
                collect_refs_in_value(ns, v, out);
            }
        }
        _ => {}
    }
}

pub fn validate_refs_in_response(ns: &str, body: &Value) -> Result<(), String> {
    let ref_re = substrate_ref_re(ns);
    let exec_re = execution_id_re();
    let mut refs = Vec::new();
    collect_refs_in_value(ns, body, &mut refs);
    for r in refs {
        if r.starts_with("cm://exec/") {
            if !exec_re.is_match(&r) {
                return Err(format!("invalid execution_id: {r}"));
            }
            continue;
        }
        if !ref_re.is_match(&r) {
            return Err(format!("invalid substrate ref: {r}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn execution_id_accepts_contract_shape_and_rejects_short_hash() {
        assert!(execution_id_re().is_match("cm://exec/1719859200123-abcdef012345"));
        assert!(!execution_id_re().is_match("cm://exec/1719859200123-abc"));
    }

    #[test]
    fn substrate_ref_re_accepts_blob_and_execution_parts() {
        let re = substrate_ref_re("gz");
        let hash64 = "a".repeat(64);
        assert!(re.is_match(&format!("gz://blob/{hash64}")));
        assert!(re.is_match("gz://codemode/execution/cm_exec_01/telemetry"));
        assert!(!re.is_match("gz://codemode/execution/cm_exec_01"));
        assert!(!re.is_match("gz://seq/foo/bar"));
    }

    #[test]
    fn validate_refs_in_response_flags_bare_execution_root() {
        let body = json!({ "telemetry_ref": "gz://codemode/execution/id-only/telemetry" });
        assert!(validate_refs_in_response("gz", &body).is_ok());
        let bad = json!({ "note": "gz://codemode/execution/no-part" });
        assert!(validate_refs_in_response("gz", &bad).is_err());
    }
}