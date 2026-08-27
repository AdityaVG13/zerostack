//! Shared input/output schema builders for the TokenZero operation ABI.
//!
//! These must stay structurally identical to the schemas advertised by
//! FastMCP (`catalog.rs`) and described for CodeMode bindings.

use serde_json::{Value, json};

use super::types::{
    ABI_DEFAULT_SHELL_TIMEOUT_SECS, ABI_HARD_MAX_WALL_MS, OperationArgs, OperationResults,
};

fn object_schema(properties: Value, required: &[&str]) -> Value {
    let mut schema =
        json!({ "type": "object", "additionalProperties": false, "properties": properties });
    if !required.is_empty() {
        schema
            .as_object_mut()
            .expect("schema object")
            .insert("required".to_string(), json!(required));
    }
    schema
}

fn mode_property() -> Value {
    json!({
        "type": "string",
        "enum": ["auto", "passthrough", "diagnostic", "structured", "dedupe", "diff-aware", "exact"],
        "default": "auto"
    })
}

fn path_value(description: &str) -> Value {
    json!({
        "type": ["string", "array"],
        "items": {"type": "string"},
        "description": description
    })
}

fn positive_usize(default: usize) -> Value {
    json!({ "type": "integer", "minimum": 1, "default": default })
}

fn line_property() -> Value {
    json!({"type": "integer", "minimum": 1})
}

fn string_alias_property() -> Value {
    json!({"type": "string", "minLength": 1})
}

fn fresh_property() -> Value {
    json!({
        "type": "boolean",
        "default": false,
        "description": "Bypass session dedup/diff for this call and always return the full render."
    })
}

pub fn no_args_schema() -> Value {
    object_schema(json!({}), &[])
}

pub fn read_schema() -> Value {
    object_schema(
        json!({
            "path": path_value("File path(s) under an allowed root."),
            "mode": mode_property(),
            "start_line": line_property(),
            "end_line": line_property(),
            "raw": {"type": "boolean", "default": false, "description": "Return contiguous text instead of a compact capsule."},
            "fresh": fresh_property(),
            "max_files": positive_usize(20),
            "max_visible_tokens": positive_usize(4000)
        }),
        &["path"],
    )
}

pub fn search_schema(query_description: &str) -> Value {
    object_schema(
        json!({
            "query": {
                "type": "string",
                "minLength": 1,
                "description": format!("{query_description} Provide this or `pattern`.")
            },
            "pattern": string_alias_property(),
            "path": path_value("Roots or files to search; defaults to the workspace root."),
            "mode": mode_property(),
            "fresh": fresh_property(),
            "max_files": positive_usize(20),
            "max_visible_tokens": positive_usize(4000)
        }),
        &[],
    )
}

pub fn batch_schema() -> Value {
    object_schema(
        json!({
            "ops": {
                "type": "array",
                "minItems": 1,
                "maxItems": 16,
                "description": "Sub-operations, each {tool, args}. Any TokenZero tool except batch itself; args match that tool's schema.",
                "items": {
                    "type": "object",
                    "properties": {
                        "tool": {"type": "string", "minLength": 1},
                        "args": {"type": "object"}
                    },
                    "required": ["tool"]
                }
            },
            "mode": mode_property()
        }),
        &["ops"],
    )
}

pub fn fetch_schema() -> Value {
    object_schema(
        json!({
            "url": {"type": "string", "minLength": 1, "description": "http(s) URL to fetch."},
            "ttl_seconds": {
                "type": "integer",
                "minimum": 0,
                "default": 86400,
                "description": "Serve a cached body younger than this without touching the network."
            },
            "fresh": {
                "type": "boolean",
                "default": false,
                "description": "Bypass the TTL cache and re-fetch."
            },
            "mode": mode_property(),
            "max_visible_tokens": positive_usize(4000)
        }),
        &["url"],
    )
}

pub fn recall_schema() -> Value {
    object_schema(
        json!({
            "query": {
                "type": "string",
                "minLength": 1,
                "description": "Literal case-insensitive substring to search for across stored payloads."
            },
            "max_hits": positive_usize(50),
            "mode": mode_property(),
            "max_visible_tokens": positive_usize(4000)
        }),
        &["query"],
    )
}

pub fn glob_schema() -> Value {
    object_schema(
        json!({
            "pattern": {"type": "string", "minLength": 1, "description": "Glob pattern to match file paths."},
            "path": path_value("Roots to inspect; defaults to the workspace root."),
            "include_hidden": {"type": "boolean", "default": false},
            "mode": mode_property(),
            "max_files": positive_usize(200),
            "max_visible_tokens": positive_usize(4000)
        }),
        &["pattern"],
    )
}

pub fn tree_schema() -> Value {
    object_schema(
        json!({
            "path": path_value("Roots to inspect; defaults to the workspace root."),
            "depth": positive_usize(2),
            "include_hidden": {"type": "boolean", "default": false},
            "mode": mode_property(),
            "max_files": positive_usize(200),
            "max_visible_tokens": positive_usize(4000)
        }),
        &[],
    )
}

pub fn edit_schema() -> Value {
    object_schema(
        json!({
            "path": {"type": "string", "minLength": 1, "description": "File path under an allowed root."},
            "edits": {
                "type": "array",
                "minItems": 1,
                "description": "Hunks applied in order against the evolving text; the batch is all-or-nothing.",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "find": {
                            "type": "string",
                            "description": "Exact text to replace; must match exactly once unless replace_all. With create=true the single hunk's find must be \"\" (empty)."
                        },
                        "replace": {
                            "type": "string",
                            "description": "Replacement text. With create=true this becomes the full new-file content."
                        },
                        "replace_all": {
                            "type": "boolean",
                            "default": false,
                            "description": "Replace every occurrence instead of requiring a unique match."
                        }
                    },
                    "required": ["find", "replace"]
                }
            },
            "create": {
                "type": "boolean",
                "default": false,
                "description": "Create a new file: requires exactly one hunk whose find is \"\" (empty); its replace becomes the full new-file content; fails if the file already exists."
            },
            "dry_run": {
                "type": "boolean",
                "default": false,
                "description": "Validate and render the hunk diff without writing."
            },
            "mode": mode_property(),
            "max_visible_tokens": positive_usize(4000)
        }),
        &["path", "edits"],
    )
}

pub fn shell_schema() -> Value {
    object_schema(
        json!({
            "command": {
                "type": "string",
                "minLength": 1,
                "description": "Command string to execute. Provide this or `argv`."
            },
            "argv": {
                "type": "array",
                "items": {"type": "string"},
                "minItems": 1,
                "description": "Argument vector executed without reparsing."
            },
            "cwd": {"type": "string", "description": "Working directory under an allowed root."},
            "mode": mode_property(),
            "rewrite": {
                "type": "string",
                "description": "Rewrite mode applied to `command` before execution when `argv` is omitted. Explicit `argv` is authoritative and skips command rewriting."
            },
            "no_rewrite": {"type": "boolean", "default": false},
            "stdin": {"type": "string"},
            "timeout_seconds": {
                "type": "integer",
                "minimum": 1,
                "default": ABI_DEFAULT_SHELL_TIMEOUT_SECS
            },
            "timeout_ms": {
                "type": "integer",
                "minimum": 1,
                "description": "Shell deadline in milliseconds. Takes precedence over timeout_seconds and preserves sub-second deadlines."
            }
        }),
        &[],
    )
}

pub fn text_schema(description: &str) -> Value {
    object_schema(
        json!({
            "text": {"type": "string", "minLength": 1, "description": description},
            "mode": mode_property()
        }),
        &["text"],
    )
}

pub fn expand_schema() -> Value {
    object_schema(
        json!({
            "ref": {
                "type": "string",
                "pattern": "^(tz|fz|gz)://",
                "description": "Exact recovery ref (tz://, fz://, or gz:// blob refs: same-store scheme alias; shared-CAS expand when attached; multi-OS/proven cross-engine portability not advertised pending multi-OS ZeroRef evidence; non-blob portable refs unsupported)."
            },
            "selector": {"type": "string", "description": "Recovery-store-specific selector."},
            "start_line": line_property(),
            "end_line": line_property(),
            "anchor_kind": {"type": "string", "description": "Anchor kind for symbol-aware recovery."},
            "symbol": {"type": "string", "description": "Symbol name for symbol-aware recovery."},
            "since": {
                "type": "string",
                "pattern": "^(tz|fz|gz)://",
                "description": "tz/fz/gz ref baseline for unified diff; errors if not recoverable."
            },
            "fresh": fresh_property()
        }),
        &["ref"],
    )
}

pub fn cache_pack_schema() -> Value {
    object_schema(
        json!({
            "scope": {
                "type": "string",
                "default": "agent",
                "description": "Cache-pack scope; use `agent`."
            }
        }),
        &[],
    )
}

pub fn rewrite_schema() -> Value {
    object_schema(
        json!({
            "command": {
                "type": "string",
                "minLength": 1,
                "description": "Command string to rewrite. Provide this or `argv`."
            },
            "argv": {
                "type": "array",
                "items": {"type": "string"},
                "minItems": 1,
                "description": "Argument vector to rewrite."
            },
            "mode": {"type": "string", "default": "safe", "description": "Rewrite policy mode."}
        }),
        &[],
    )
}

pub fn execute_code_schema() -> Value {
    object_schema(
        json!({
            "plan": {"type": "string", "maxLength": 65536},
            "form": {"type": "string", "enum": ["recipe", "json", "js", "auto"]},
            "root": {"type": "string", "description": "Execute root; must remain under the server allowlist."},
            "cwd": {"type": "string", "description": "Alias for root."},
            "workspace": {"type": "string", "description": "Alias for root."},
            "allowed_root": {
                "oneOf": [
                    {"type": "string"},
                    {"type": "array", "items": {"type": "string"}}
                ]
            },
            "allowed_roots": {
                "oneOf": [
                    {"type": "string"},
                    {"type": "array", "items": {"type": "string"}}
                ]
            },
            "limits": {
                "type": "object",
                "properties": {
                    "max_wall_ms": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": ABI_HARD_MAX_WALL_MS
                    },
                    "hard_max_wall_ms": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": ABI_HARD_MAX_WALL_MS
                    }
                },
                "additionalProperties": {"type": "integer", "minimum": 0}
            }
        }),
        &["plan"],
    )
}

pub fn codemode_search_schema() -> Value {
    object_schema(
        json!({
            "query": {"type": "string", "minLength": 1},
            "limit": {"type": "integer", "minimum": 1, "maximum": 50}
        }),
        &["query"],
    )
}

pub fn codemode_describe_schema() -> Value {
    object_schema(
        json!({
            "name": {"type": "string", "minLength": 1}
        }),
        &["name"],
    )
}

pub fn report_tool_issue_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "tool": {"type": "string", "minLength": 1, "description": "Tool/surface name (zero_execute accepted)."},
            "summary": {"type": "string", "minLength": 1, "description": "Short issue summary."},
            "detail": {"type": "string", "description": "Optional detail / repro."}
        },
        "required": ["tool", "summary"],
        "additionalProperties": true
    })
}

/// Default normalized domain result envelope (success + typed error union).
pub fn default_results() -> OperationResults {
    OperationResults {
        schema: json!({
            "oneOf": [
                {
                    "type": "object",
                    "description": "DomainResult success envelope",
                    "properties": {
                        "value": {},
                        "refs": { "type": "array", "items": { "type": "string" } },
                        "op": { "type": "string", "minLength": 1 },
                        "telemetry": { "type": "object" }
                    },
                    "required": ["value", "op"]
                },
                {
                    "type": "object",
                    "description": "DomainError envelope",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": [
                                "validation", "policy", "sandbox", "runtime", "substrate",
                                "busy", "approval", "cancelled", "deadline_exceeded",
                                "not_found", "unauthorized", "invalid_pattern", "invalid_ref",
                                "invalid_url", "hunk_not_found", "ambiguous_hunk", "no_op_hunk"
                            ]
                        },
                        "message": { "type": "string" },
                        "retryable": { "type": "boolean" },
                        "op": { "type": "string" },
                        "recovery_ref": { "type": "string" }
                    },
                    "required": ["kind", "message", "retryable"]
                }
            ]
        }),
    }
}

/// Ref-first success shape used by material/read/search ops.
pub fn ref_first_results() -> OperationResults {
    OperationResults {
        schema: json!({
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "value": {
                            "type": "object",
                            "properties": {
                                "text": { "type": "string" },
                                "status": { "type": "string" }
                            }
                        },
                        "refs": {
                            "type": "array",
                            "items": { "type": "string", "minLength": 1 },
                            "minItems": 0
                        },
                        "op": { "type": "string", "minLength": 1 },
                        "telemetry": { "type": "object" }
                    },
                    "required": ["value", "op", "refs"]
                },
                {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string" },
                        "message": { "type": "string" },
                        "retryable": { "type": "boolean" },
                        "op": { "type": "string" },
                        "recovery_ref": { "type": "string" }
                    },
                    "required": ["kind", "message", "retryable"]
                }
            ]
        }),
    }
}

pub fn args(schema: Value) -> OperationArgs {
    OperationArgs { schema }
}

