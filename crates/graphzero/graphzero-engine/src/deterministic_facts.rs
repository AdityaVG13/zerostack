//! RACC deterministic-facts-only contract guard. GraphZero results must be cacheable, which
//! requires every emitted fact to be a deterministic function of (snapshot, operator, args).

use serde::Serialize;
use serde_json::{Map, Value};

/// Fact kinds GraphZero is allowed to emit. Every value under a kind-bearing
/// key must be in this list: the set is parser/index-derived structure only
/// (symbols, edges, spans, files, git history), never semantic judgment.
pub const FACT_KIND_ALLOWLIST: &[&str] = &[
    // symbol kinds
    "function",
    "type",
    "module",
    "symbol",
    "file",
    // graph edge kinds
    "calls",
    "refs",
    "imports",
    "other",
    // reading-set / neighborhood roles
    "target",
    "caller",
    "callee",
    "dependency",
    "type_ref",
    "test",
    "seed",
    // snap-to-file target kinds: the operator/relation that produced the hit
    "blast",
    "def",
    "ref",
    // cross-repo edge provenance, derived from manifest/api-surface node names
    "symbol_edge",
    "workspace_edge",
    "api_surface_edge",
    // parser-derived silent-risk detections
    "string_key",
    "dynamic_dispatch",
    "cross_artifact",
    // memory kinds: operator-recorded annotations replayed verbatim from the
    // store, never judgments GraphZero synthesizes itself
    "decision",
    "invariant",
    "gotcha",
    "note",
    // git/worktree-derived planned-impact kinds
    "untracked_file",
    "removed_call",
    "added_call",
    "renamed_symbol",
];

/// Object keys whose value must name an allowlisted fact kind.
const KIND_KEYS: &[&str] = &["kind", "edge_kind", "provenance_kind", "relation"];

/// Object keys that are inherently nondeterministic across identical runs.
const FORBIDDEN_KEYS: &[&str] = &[
    "timestamp",
    "timestamp_ms",
    "generated_at",
    "created_at",
    "updated_at",
    "observed_at",
    "now",
    "wall_clock_ms",
    "elapsed_ms",
    "duration_ms",
    "latency_ms",
    "pid",
    "hostname",
    "temp_dir",
    "tmp_dir",
    "random_seed",
    "nonce",
    "uuid",
    "run_id",
];

/// Substrings that mark a machine-local absolute path.
const TEMP_PATH_MARKERS: &[&str] = &["/tmp/", "/var/folders/", "/private/var/folders/"];

/// Speculative-claim markers. GraphZero maps code to addressable structure;
/// semantic judgment belongs to the model, not to cached facts.
const SPECULATIVE_MARKERS: &[&str] = &[
    "probably",
    "presumably",
    "likely does",
    "seems to",
    "appears to",
    "might be",
    "may be doing",
    "i think",
    "we recommend",
    "should be refactored",
    "should probably",
    "consider refactoring",
    "code smell",
    "suspect that",
];

/// One contract breach, addressed by a JSON path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactViolation {
    pub path: String,
    pub rule: &'static str,
    pub detail: String,
}

impl std::fmt::Display for FactViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} [{}]: {}", self.path, self.rule, self.detail)
    }
}

/// Canonical byte form of a fact payload: object keys sorted, arrays kept in
/// emission order (operators are required to emit sorted arrays, so a changed
/// array order across runs is itself a violation this form exposes).
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

/// Canonical byte form of any serializable operator result.
pub fn canonical_facts<T: Serialize>(value: &T) -> String {
    let json = serde_json::to_value(value).expect("fact payload must serialize to JSON");
    canonical_json(&json)
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String((*key).clone()).to_string());
                out.push(':');
                write_canonical(&map[*key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// Audit a serializable operator result against the deterministic-facts contract.
pub fn audit_facts<T: Serialize>(value: &T) -> Vec<FactViolation> {
    match serde_json::to_value(value) {
        Ok(json) => audit_value(&json),
        Err(error) => vec![FactViolation {
            path: "$".into(),
            rule: "unserializable",
            detail: error.to_string(),
        }],
    }
}

/// Audit a JSON fact payload against the deterministic-facts contract.
pub fn audit_value(value: &Value) -> Vec<FactViolation> {
    let mut violations = Vec::new();
    walk(value, "$", &mut violations);
    violations
}

fn walk(value: &Value, path: &str, out: &mut Vec<FactViolation>) {
    match value {
        Value::Object(map) => walk_object(map, path, out),
        Value::Array(items) => {
            for (idx, item) in items.iter().enumerate() {
                walk(item, &format!("{path}[{idx}]"), out);
            }
        }
        Value::String(text) => check_string(text, path, out),
        _ => {}
    }
}

fn walk_object(map: &Map<String, Value>, path: &str, out: &mut Vec<FactViolation>) {
    for (key, child) in map {
        let child_path = format!("{path}.{key}");
        if FORBIDDEN_KEYS.contains(&key.as_str()) {
            out.push(FactViolation {
                path: child_path.clone(),
                rule: "nondeterministic_key",
                detail: format!("key {key} cannot be a function of (snapshot, operator, args)"),
            });
        }
        if KIND_KEYS.contains(&key.as_str())
            && let Value::String(kind) = child
            && !FACT_KIND_ALLOWLIST.contains(&kind.as_str())
        {
            out.push(FactViolation {
                path: child_path.clone(),
                rule: "unknown_fact_kind",
                detail: format!(
                    "fact kind {kind} is not in the deterministic allowlist {FACT_KIND_ALLOWLIST:?}"
                ),
            });
        }
        walk(child, &child_path, out);
    }
}

fn check_string(text: &str, path: &str, out: &mut Vec<FactViolation>) {
    if let Some(marker) = TEMP_PATH_MARKERS.iter().find(|m| text.contains(**m)) {
        out.push(FactViolation {
            path: path.into(),
            rule: "absolute_temp_path",
            detail: format!("machine-local path marker {marker} in {text}"),
        });
    }
    if contains_timestamp(text) {
        out.push(FactViolation {
            path: path.into(),
            rule: "wall_clock_timestamp",
            detail: format!("date/time literal in {text}"),
        });
    }
    if contains_uuid(text) {
        out.push(FactViolation {
            path: path.into(),
            rule: "random_identifier",
            detail: format!("uuid-shaped identifier in {text}"),
        });
    }
    let lower = text.to_ascii_lowercase();
    if let Some(marker) = SPECULATIVE_MARKERS.iter().find(|m| lower.contains(**m)) {
        out.push(FactViolation {
            path: path.into(),
            rule: "speculative_claim",
            detail: format!("speculative marker {marker} in {text}"),
        });
    }
}

/// Detects YYYY-MM-DD and HH:MM:SS shaped literals.
fn contains_timestamp(text: &str) -> bool {
    let b = text.as_bytes();
    let digit = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    for i in 0..b.len() {
        if digit(i)
            && digit(i + 1)
            && digit(i + 2)
            && digit(i + 3)
            && b.get(i + 4) == Some(&b'-')
            && digit(i + 5)
            && digit(i + 6)
            && b.get(i + 7) == Some(&b'-')
            && digit(i + 8)
            && digit(i + 9)
        {
            return true;
        }
        if digit(i)
            && digit(i + 1)
            && b.get(i + 2) == Some(&b':')
            && digit(i + 3)
            && digit(i + 4)
            && b.get(i + 5) == Some(&b':')
            && digit(i + 6)
            && digit(i + 7)
        {
            return true;
        }
    }
    false
}

/// Detects 8-4-4-4-12 hex identifiers (uuid / random session id shapes).
fn contains_uuid(text: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let chars: Vec<char> = text.chars().collect();
    'start: for start in 0..chars.len() {
        let mut idx = start;
        for (group, len) in GROUPS.iter().enumerate() {
            if group > 0 {
                if chars.get(idx) != Some(&'-') {
                    continue 'start;
                }
                idx += 1;
            }
            for _ in 0..*len {
                match chars.get(idx) {
                    Some(c) if c.is_ascii_hexdigit() => idx += 1,
                    _ => continue 'start,
                }
            }
        }
        // Reject a longer hex run (blob digests) that merely contains this shape.
        let tail_is_hex = chars.get(idx).is_some_and(char::is_ascii_hexdigit);
        let head_is_hex = start > 0 && chars.get(start - 1).is_some_and(char::is_ascii_hexdigit);
        if !tail_is_hex && !head_is_hex {
            return true;
        }
    }
    false
}

/// Panics with every violation when the payload breaches the contract.
pub fn assert_deterministic_facts<T: Serialize>(label: &str, payload: &T) {
    let violations = audit_facts(payload);
    if !violations.is_empty() {
        let rendered = violations
            .iter()
            .map(FactViolation::to_string)
            .collect::<Vec<_>>()
            .join("\n  ");
        panic!(
            "deterministic-facts contract violated by {label} ({} violation(s)):\n  {rendered}",
            violations.len()
        );
    }
}

/// Contract check enforced in debug and test builds only; compiled out of
/// release so cached-result production stays on the fast path.
#[inline]
pub fn debug_assert_deterministic_facts<T: Serialize>(label: &str, payload: &T) {
    #[cfg(debug_assertions)]
    assert_deterministic_facts(label, payload);
    #[cfg(not(debug_assertions))]
    {
        let _ = (label, payload);
    }
}
