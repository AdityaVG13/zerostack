//! Canonical ActionCache key envelope (tokenzero-canonical-key-envelope-ib5y).
//!
//! Hub owns comparison identity (`zero_abi::canonical_json`). TokenZero owns
//! the model_id / consistency_class envelope so MCP, CodeMode, CLI, and the
//! raw worker produce the same key for the same logical op.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokenzero_core::sha256_hex;
use tokenzero_recovery::{STORE_SCHEMA_MAJOR, STORE_SCHEMA_MINOR};
use zero_abi::canonical_json;

pub const ACTIONCACHE_KEY_SCHEMA: &str = "tokenzero.actioncache.key.v1";

/// Consistency class on the envelope. Default is the most conservative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsistencyClass {
    ExactHit,
    Swr,
    MustBlockRevalidate,
}

impl Default for ConsistencyClass {
    fn default() -> Self {
        Self::MustBlockRevalidate
    }
}

impl ConsistencyClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExactHit => "exact_hit",
            Self::Swr => "swr",
            Self::MustBlockRevalidate => "must_block_revalidate",
        }
    }

    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("exact_hit") => Self::ExactHit,
            Some("swr") => Self::Swr,
            _ => Self::MustBlockRevalidate,
        }
    }
}

/// Inputs one harness supplies when minting an ActionCache key.
#[derive(Debug, Clone)]
pub struct ActionCacheKeyInput<'a> {
    pub op: &'a str,
    pub args: &'a Value,
    pub store_root: &'a Path,
    pub model_id: Option<&'a str>,
    pub consistency_class: Option<ConsistencyClass>,
}

/// Byte-identical digest across harnesses for one logical op.
pub fn action_cache_key(input: ActionCacheKeyInput<'_>) -> String {
    let body = action_cache_envelope(input);
    sha256_hex(&canonical_json(&body))
}

/// Canonical envelope before hashing. Exposed so fixtures can assert shape.
pub fn action_cache_envelope(input: ActionCacheKeyInput<'_>) -> Value {
    let args = fill_op_defaults(input.op, canonicalize_value(input.args, input.store_root));
    let consistency = input.consistency_class.unwrap_or_default();
    json!({
        "schema": ACTIONCACHE_KEY_SCHEMA,
        "op": normalize_op(input.op),
        "args": args,
        "engine_version": env!("CARGO_PKG_VERSION"),
        "store_schema_major": STORE_SCHEMA_MAJOR,
        "store_schema_minor": STORE_SCHEMA_MINOR,
        "model_id": input.model_id.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(""),
        "consistency_class": consistency.as_str(),
    })
}

fn normalize_op(op: &str) -> &str {
    op.strip_prefix("tz_")
        .or_else(|| op.strip_prefix("zero.token."))
        .or_else(|| op.strip_prefix("zero."))
        .unwrap_or(op)
}

fn canonicalize_value(value: &Value, store_root: &Path) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, child) in map {
                out.insert(key.clone(), canonicalize_field(key, child, store_root));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| canonicalize_value(item, store_root))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn canonicalize_field(key: &str, value: &Value, store_root: &Path) -> Value {
    match (key, value) {
        ("path" | "cwd" | "root", Value::String(path)) => {
            Value::String(normalize_store_path(path, store_root))
        }
        ("path", Value::Array(paths)) => Value::Array(
            paths
                .iter()
                .map(|item| match item {
                    Value::String(path) => Value::String(normalize_store_path(path, store_root)),
                    other => canonicalize_value(other, store_root),
                })
                .collect(),
        ),
        _ => canonicalize_value(value, store_root),
    }
}

fn normalize_store_path(path: &str, store_root: &Path) -> String {
    let raw = Path::new(path);
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        store_root.join(raw)
    };
    let normalized = lexical_normalize(&joined);
    let root = lexical_normalize(store_root);
    if let Ok(rel) = normalized.strip_prefix(&root) {
        rel.to_string_lossy().replace('\\', "/")
    } else {
        path.replace('\\', "/")
    }
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut stack = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    stack.into_iter().collect()
}

fn fill_op_defaults(op: &str, mut args: Value) -> Value {
    let Some(map) = args.as_object_mut() else {
        return args;
    };
    let op = normalize_op(op);
    match op {
        "read" => {
            insert_default(map, "fresh", json!(false));
            insert_default(map, "raw", json!(false));
            insert_default(map, "mode", json!("auto"));
            insert_default(map, "start_line", Value::Null);
            insert_default(map, "end_line", Value::Null);
        }
        "expand" => {
            insert_default(map, "fresh", json!(false));
            insert_default(map, "raw", json!(false));
            insert_default(map, "selector", Value::Null);
            insert_default(map, "start_line", Value::Null);
            insert_default(map, "end_line", Value::Null);
            insert_default(map, "since", Value::Null);
        }
        "find" | "grep" => {
            insert_default(map, "fresh", json!(false));
            insert_default(map, "max_results", Value::Null);
        }
        _ => {}
    }
    args
}

fn insert_default(map: &mut Map<String, Value>, key: &str, value: Value) {
    map.entry(key.to_string()).or_insert(value);
}

