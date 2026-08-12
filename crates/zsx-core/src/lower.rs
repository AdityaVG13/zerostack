//! Canonical surface-to-domain lowering for the single-process ZSX core.
//!
//! `ZsxSession`, the `zsx` executable, and native harness bindings use this
//! lowering authority directly. No process-backed compatibility copy remains.

use serde_json::Value;
use zero_abi::raw_worker::EngineIdentity;
use zero_abi::{TOKEN_JOB_OPERATION_V1, TokenJobPollRequestV1};
use zero_codemode::ConnectorError;

/// Every capability the aggregate ZSX surface registers, in stable order.
pub const METHODS: &[(&str, &str)] = &[
    ("fs", "plan"),
    ("fs", "structural"),
    ("fs", "compound"),
    ("fs", "world"),
    ("fs", "edit"),
    ("fs", "write"),
    ("fs", "transact"),
    ("fs", "read_many"),
    ("fs", "list_many"),
    ("fs", "search_many"),
    ("fs", "ast_search_many"),
    ("graph", "blast"),
    ("graph", "query"),
    ("graph", "orient"),
    ("graph", "recall"),
    ("graph", "verify"),
    ("graph", "snap"),
    ("graph", "reserve"),
    ("graph", "index"),
    ("graph", "remember"),
    ("token", "compact"),
    ("token", "expand"),
    ("token", "find"),
    ("token", "read"),
    ("token", "job"),
    ("token", "shell"),
    ("help", "search"),
];

/// Map a public surface name to its single engine.
pub fn engine_for(surface: &str) -> Result<EngineIdentity, ConnectorError> {
    match surface {
        "fs" => Ok(EngineIdentity::FsZero),
        "graph" => Ok(EngineIdentity::GraphZero),
        "token" => Ok(EngineIdentity::TokenZero),
        _ => Err(ConnectorError::new("unknown aggregate surface")),
    }
}

pub(crate) fn positional_args(input: &Value, first_key: &str, second_key: Option<&str>) -> Value {
    let Some(items) = input.as_array() else {
        if input.is_object() {
            return input.clone();
        }
        let mut object = serde_json::Map::new();
        object.insert(first_key.into(), input.clone());
        return Value::Object(object);
    };
    let mut object = serde_json::Map::new();
    if let Some(options) = items.get(1).and_then(Value::as_object) {
        object.extend(options.clone());
    }
    if let Some(first) = items.first() {
        object.insert(first_key.into(), first.clone());
    }
    if let (Some(key), Some(second)) = (second_key, items.get(1))
        && !second.is_object()
    {
        object.insert(key.into(), second.clone());
    }
    Value::Object(object)
}

pub(crate) fn vector_args(input: &Value, key: &str) -> Value {
    let mut object = serde_json::Map::new();
    if let Some(arguments) = input.as_array() {
        if arguments.len() == 2
            && arguments.first().is_some_and(Value::is_array)
            && arguments.get(1).is_some_and(Value::is_object)
        {
            object.extend(arguments[1].as_object().cloned().unwrap_or_default());
            object.insert(key.into(), arguments[0].clone());
        } else {
            object.insert(key.into(), input.clone());
        }
    } else {
        object.insert(key.into(), input.clone());
    }
    Value::Object(object)
}

#[derive(Clone, Copy)]
enum TokenOptionType {
    Bool,
    PositiveInteger,
    String,
    Mode,
}

const TOKEN_READ_OPTIONS: &[(&str, TokenOptionType)] = &[
    ("mode", TokenOptionType::Mode),
    ("start_line", TokenOptionType::PositiveInteger),
    ("end_line", TokenOptionType::PositiveInteger),
    ("raw", TokenOptionType::Bool),
    ("fresh", TokenOptionType::Bool),
    ("max_files", TokenOptionType::PositiveInteger),
    ("max_visible_tokens", TokenOptionType::PositiveInteger),
];

const TOKEN_SHELL_OPTIONS: &[(&str, TokenOptionType)] = &[
    ("cwd", TokenOptionType::String),
    ("mode", TokenOptionType::Mode),
    ("rewrite", TokenOptionType::String),
    ("no_rewrite", TokenOptionType::Bool),
    ("stdin", TokenOptionType::String),
    ("timeout_ms", TokenOptionType::PositiveInteger),
    ("timeout_seconds", TokenOptionType::PositiveInteger),
    ("background", TokenOptionType::Bool),
];

fn token_method_args(
    input: &Value,
    method: &str,
    first_key: &str,
    contract: &[(&str, TokenOptionType)],
) -> Result<Value, ConnectorError> {
    let mut args = serde_json::Map::new();
    if let Some(arguments) = input.as_array() {
        if !arguments.is_empty() && arguments.iter().all(Value::is_string) {
            // The host preserves a single argument's shape, so a string array is
            // the first value itself rather than a positional argument list.
            args.insert(first_key.into(), input.clone());
        } else {
            if arguments.is_empty() || arguments.len() > 2 {
                return Err(ConnectorError::new(format!(
                    "token.{method} requires one value and an optional options object"
                )));
            }
            args.insert(first_key.into(), arguments[0].clone());
            if let Some(options) = arguments.get(1) {
                let options = options.as_object().ok_or_else(|| {
                    ConnectorError::new(format!("token.{method} options must be an object"))
                })?;
                if options.contains_key(first_key) {
                    return Err(ConnectorError::new(format!(
                        "token.{method} options must not repeat {first_key}"
                    )));
                }
                args.extend(options.clone());
            }
        }
    } else if let Some(named) = input.as_object() {
        args = named.clone();
    } else {
        args.insert(first_key.into(), input.clone());
    }

    let first = args
        .get(first_key)
        .ok_or_else(|| ConnectorError::new(format!("token.{method} requires {first_key}")))?;
    let valid_first = first.as_str().is_some_and(|value| !value.is_empty())
        || first.as_array().is_some_and(|values| {
            !values.is_empty()
                && values
                    .iter()
                    .all(|value| value.as_str().is_some_and(|value| !value.is_empty()))
        });
    if !valid_first {
        return Err(ConnectorError::new(format!(
            "token.{method} {first_key} must be a string or non-empty string array"
        )));
    }

    for (key, value) in &args {
        if key == first_key {
            continue;
        }
        let Some((_, expected)) = contract.iter().find(|(name, _)| *name == key) else {
            let supported = contract
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
                .join(", ");
            let advice = if method == "shell" && key == "raw" {
                r#"; use { mode: "exact" } for exact shell output"#
            } else {
                ""
            };
            return Err(ConnectorError::new(format!(
                "token.{method} unknown option '{key}'; supported options: {supported}{advice}"
            )));
        };
        let valid = match expected {
            TokenOptionType::Bool => value.is_boolean(),
            TokenOptionType::PositiveInteger => value.as_u64().is_some_and(|number| number > 0),
            TokenOptionType::String => value.is_string(),
            TokenOptionType::Mode => value.as_str().is_some_and(|mode| {
                matches!(
                    mode,
                    "auto"
                        | "hybrid"
                        | "passthrough"
                        | "diagnostic"
                        | "critical"
                        | "structured"
                        | "fidelity"
                        | "dedupe"
                        | "diff-aware"
                        | "diff_aware"
                        | "diffaware"
                        | "exact"
                        | "lossy"
                )
            }),
        };
        if !valid {
            return Err(ConnectorError::new(format!(
                "token.{method} option '{key}' has an invalid value: {value}"
            )));
        }
    }
    Ok(Value::Object(args))
}

fn token_job_args(input: &Value) -> Result<Value, ConnectorError> {
    let candidate = if let Some(arguments) = input.as_array() {
        if arguments.is_empty() || arguments.len() > 2 {
            return Err(ConnectorError::new(
                "token.job requires an id and optional options object",
            ));
        }
        let id = arguments[0]
            .as_str()
            .ok_or_else(|| ConnectorError::new("token.job id must be a string"))?;
        let mut object = match arguments.get(1) {
            Some(Value::Object(options)) if !options.contains_key("id") => options.clone(),
            Some(Value::Object(_)) => {
                return Err(ConnectorError::new("token.job options must not repeat id"));
            }
            Some(_) => {
                return Err(ConnectorError::new("token.job options must be an object"));
            }
            None => serde_json::Map::new(),
        };
        object.insert("id".into(), Value::String(id.to_owned()));
        Value::Object(object)
    } else if let Some(id) = input.as_str() {
        serde_json::json!({"id":id})
    } else {
        input.clone()
    };
    let request: TokenJobPollRequestV1 = serde_json::from_value(candidate)
        .map_err(|error| ConnectorError::new(format!("invalid token.job arguments: {error}")))?;
    request
        .validate()
        .map_err(|error| ConnectorError::new(format!("invalid token.job arguments: {error}")))?;
    serde_json::to_value(request).map_err(|error| ConnectorError::new(error.to_string()))
}

/// Lower one public `surface.method` call to its canonical domain operation.
///
/// Returns `(engine, operation, args)` exactly as the process compatibility
/// path does; expansion routing follows the ref scheme owner.
pub fn lower(
    surface: &str,
    method: &str,
    input: Value,
) -> Result<(EngineIdentity, String, Value), ConnectorError> {
    let engine = engine_for(surface)?;
    if surface == "token" && method == "expand" {
        let reference = input
            .as_array()
            .and_then(|values| values.first())
            .and_then(Value::as_str)
            .or_else(|| input.get("ref").and_then(Value::as_str))
            .or_else(|| input.as_str())
            .ok_or_else(|| ConnectorError::new("token.expand requires ref"))?;
        let (engine, op, key) = if reference.starts_with("fz://") {
            (EngineIdentity::FsZero, "fs.expand", "ref")
        } else if reference.starts_with("gz://")
            || reference.starts_with("g:")
            || reference.starts_with("q:")
        {
            (EngineIdentity::GraphZero, "expand", "reference")
        } else if reference.starts_with("tz://") {
            (EngineIdentity::TokenZero, "expand", "ref")
        } else {
            return Err(ConnectorError::new("unsupported ref scheme"));
        };
        let mut args = serde_json::Map::new();
        args.insert(key.into(), Value::String(reference.into()));
        return Ok((engine, op.into(), Value::Object(args)));
    }
    if surface == "fs" && method == "plan" {
        let goal = input
            .as_array()
            .and_then(|values| values.first())
            .or_else(|| input.get("goal"))
            .and_then(Value::as_str)
            .or_else(|| input.as_str())
            .unwrap_or_default();
        let stop = [
            "about",
            "context",
            "discover",
            "entrypoint",
            "files",
            "find",
            "from",
            "into",
            "load",
            "locate",
            "map",
            "repo",
            "repository",
            "the",
            "this",
            "with",
        ];
        let queries: Vec<Value> = goal
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .map(str::to_ascii_lowercase)
            .filter(|term| term.len() >= 3 && !stop.contains(&term.as_str()))
            .take(8)
            .map(Value::String)
            .collect();
        if queries.is_empty() {
            return Ok((engine, "fs.ls".into(), serde_json::json!({"arg":"."})));
        }
        return Ok((
            engine,
            "fs.searchMany".into(),
            serde_json::json!({"queries":queries}),
        ));
    }
    if surface == "fs" && method == "structural" {
        let values = input.as_array();
        let query = values
            .and_then(|items| items.first())
            .or_else(|| input.get("query"))
            .and_then(Value::as_str)
            .or_else(|| input.as_str())
            .ok_or_else(|| ConnectorError::new("fs.structural requires query"))?;
        let target = values
            .and_then(|items| items.get(1))
            .or_else(|| input.get("target"))
            .and_then(Value::as_str);
        let query = target
            .map(|target| format!("{query}:{target}"))
            .unwrap_or_else(|| query.to_owned());
        return Ok((
            engine,
            "fs.search".into(),
            serde_json::json!({"query":query}),
        ));
    }
    // Direct mutation surface: zero.fs.edit({...}) / zero.fs.write({...}).
    // One options object (positionally or bare) passed through unchanged so
    // the FSZero dispatcher owns arg semantics, including the CAS `base`
    // gate (null = must-not-exist create, fz://blob/<sha256> = compare-
    // and-swap against current content).
    if surface == "fs" && (method == "edit" || method == "write") {
        let args = if let Some(items) = input.as_array() {
            if items.len() != 1 || !items[0].is_object() {
                return Err(ConnectorError::new(
                    "fs.edit and fs.write take exactly one options object",
                ));
            }
            items[0].clone()
        } else if input.is_object() {
            input
        } else {
            return Err(ConnectorError::new(
                "fs.edit and fs.write take exactly one options object",
            ));
        };
        return Ok((engine, format!("fs.{method}"), args));
    }
    // All-or-nothing multi-step mutation: zero.fs.transact([step, ...]) or
    // zero.fs.transact(step, ...). Steps are objects; FSZero owns semantics.
    if surface == "fs" && method == "transact" {
        let steps = match input.as_array() {
            Some(items) if items.len() == 1 && items[0].is_array() => items[0].clone(),
            Some(items) if !items.is_empty() && items.iter().all(Value::is_object) => {
                Value::Array(items.clone())
            }
            _ => {
                return Err(ConnectorError::new(
                    "fs.transact takes one non-empty array of step objects",
                ));
            }
        };
        return Ok((engine, "fs.transact".into(), serde_json::json!({"steps": steps})));
    }
    if surface == "fs" && method == "compound" {
        let (name, compound_args) = if let Some(items) = input.as_array() {
            (
                items
                    .first()
                    .and_then(Value::as_str)
                    .ok_or_else(|| ConnectorError::new("fs.compound requires name"))?,
                items
                    .get(1)
                    .cloned()
                    .unwrap_or(Value::Object(Default::default())),
            )
        } else {
            (
                input
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ConnectorError::new("fs.compound requires name"))?,
                input
                    .get("args")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default())),
            )
        };
        let op = match name {
            "read" => "fs.read",
            "search" | "find" | "grep" => "fs.search",
            "list" | "tree" | "inventory" => "fs.ls",
            "mutate" | "edit" | "verifiedEdit" => "fs.edit",
            "write" => "fs.write",
            "resolve" => "fs.resolve",
            _ => {
                return Err(ConnectorError::new(
                    "unsupported planner-free fs.compound operation; discover call shapes with zero.help.search({query})",
                ));
            }
        };
        return Ok((engine, op.into(), compound_args));
    }
    if surface == "fs" && method == "world" {
        let args = if let Some(items) = input.as_array() {
            if items.is_empty() || items.len() > 2 {
                return Err(ConnectorError::new(
                    "fs.world requires an action and optional options object",
                ));
            }
            let action = items[0]
                .as_str()
                .ok_or_else(|| ConnectorError::new("fs.world action must be a string"))?;
            if let Some(options) = items.get(1) {
                let mut object = options
                    .as_object()
                    .cloned()
                    .ok_or_else(|| ConnectorError::new("fs.world options must be an object"))?;
                if object.contains_key("action") || object.contains_key("arg") {
                    return Err(ConnectorError::new(
                        "fs.world options must not repeat action or arg",
                    ));
                }
                object.insert("action".into(), Value::String(action.into()));
                Value::Object(object)
            } else if matches!(action, "fork") || action.contains(':') {
                serde_json::json!({"arg": action})
            } else {
                serde_json::json!({"action": action})
            }
        } else if input.is_object() {
            input
        } else if let Some(action) = input.as_str() {
            serde_json::json!({"arg": action})
        } else {
            return Err(ConnectorError::new(
                "fs.world requires an action string or object",
            ));
        };
        return Ok((engine, "fs.world".into(), args));
    }
    if surface == "fs" {
        let (op, key) = match method {
            "read_many" => ("fs.readMany", "paths"),
            "list_many" => ("fs.listMany", "items"),
            "search_many" => ("fs.searchMany", "queries"),
            "ast_search_many" => ("fs.astSearchMany", "items"),
            _ => return Err(ConnectorError::new("unsupported fs method; discover call shapes with zero.help.search({query})")),
        };
        return Ok((engine, op.into(), vector_args(&input, key)));
    }
    if surface == "graph" {
        let args = match method {
            "blast" => positional_args(&input, "intent", None),
            "query" | "orient" => positional_args(&input, "surface", Some("query")),
            "recall" => positional_args(&input, "query", None),
            "verify" => positional_args(&input, "target", Some("claim")),
            "snap" => positional_args(&input, "query", Some("budget")),
            "reserve" => positional_args(&input, "action", None),
            "remember" => {
                let fact = input
                    .as_array()
                    .and_then(|values| values.first())
                    .cloned()
                    .unwrap_or(input.clone());
                if fact.is_object() {
                    fact
                } else {
                    serde_json::json!({"text":fact})
                }
            }
            "index" => Value::Object(Default::default()),
            _ => return Err(ConnectorError::new("unsupported graph method; discover call shapes with zero.help.search({query})")),
        };
        return Ok((engine, method.into(), args));
    }
    let (op, args) = match method {
        "compact" => {
            let value = input
                .as_array()
                .and_then(|values| values.first())
                .cloned()
                .unwrap_or(input);
            let text = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            ("ingest", serde_json::json!({"text":text}))
        }
        "find" => ("find", positional_args(&input, "query", Some("path"))),
        "read" => (
            "read",
            token_method_args(&input, "read", "path", TOKEN_READ_OPTIONS)?,
        ),
        "job" => (TOKEN_JOB_OPERATION_V1, token_job_args(&input)?),
        "shell" => (
            "shell",
            token_method_args(&input, "shell", "command", TOKEN_SHELL_OPTIONS)?,
        ),
        _ => return Err(ConnectorError::new("unsupported token method; discover call shapes with zero.help.search({query})")),
    };
    Ok((engine, op.into(), args))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assert_lower(
        surface: &str,
        method: &str,
        input: Value,
        engine: EngineIdentity,
        op: &str,
        args: Value,
    ) {
        let lowered = lower(surface, method, input).unwrap();
        assert_eq!(lowered.0, engine);
        assert_eq!(lowered.1, op);
        assert_eq!(lowered.2, args);
    }

    #[test]
    fn fs_methods_use_canonical_domain_operations() {
        let plan = lower("fs", "plan", json!("map widget entrypoint")).unwrap();
        assert_eq!(plan.0, EngineIdentity::FsZero);
        assert_eq!(plan.1, "fs.searchMany");
        assert_eq!(plan.2["queries"], json!(["widget"]));
        assert_lower(
            "fs",
            "structural",
            json!(["callers", "Widget"]),
            EngineIdentity::FsZero,
            "fs.search",
            json!({"query":"callers:Widget"}),
        );
        assert_lower(
            "fs",
            "compound",
            json!(["read", {"path":"src/lib.rs"}]),
            EngineIdentity::FsZero,
            "fs.read",
            json!({"path":"src/lib.rs"}),
        );
        assert_lower(
            "fs",
            "world",
            json!(["commit", {"world":"W7"}]),
            EngineIdentity::FsZero,
            "fs.world",
            json!({"action":"commit","world":"W7"}),
        );
        assert_lower(
            "fs",
            "world",
            json!("newbatch:a.txt:a|A;;b.txt:b|B"),
            EngineIdentity::FsZero,
            "fs.world",
            json!({"arg":"newbatch:a.txt:a|A;;b.txt:b|B"}),
        );
        assert_lower(
            "fs",
            "read_many",
            json!([["a.rs"], {"max_bytes":32}]),
            EngineIdentity::FsZero,
            "fs.readMany",
            json!({"paths":["a.rs"],"max_bytes":32}),
        );
        assert_lower(
            "fs",
            "search_many",
            json!(["one", "two"]),
            EngineIdentity::FsZero,
            "fs.searchMany",
            json!({"queries":["one","two"]}),
        );
    }

    #[test]
    fn graph_and_token_methods_use_bare_domain_operations() {
        assert_lower(
            "graph",
            "blast",
            json!(["Widget", {"depth":2}]),
            EngineIdentity::GraphZero,
            "blast",
            json!({"intent":"Widget","depth":2}),
        );
        assert_lower(
            "graph",
            "query",
            json!(["symbol", "Widget"]),
            EngineIdentity::GraphZero,
            "query",
            json!({"surface":"symbol","query":"Widget"}),
        );
        assert_lower(
            "token",
            "shell",
            json!(["printf ok", {"timeout_seconds":1}]),
            EngineIdentity::TokenZero,
            "shell",
            json!({"command":"printf ok","timeout_seconds":1}),
        );
        assert_lower(
            "token",
            "find",
            json!("Widget"),
            EngineIdentity::TokenZero,
            "find",
            json!({"query":"Widget"}),
        );
        assert_lower(
            "token",
            "expand",
            json!("g:42"),
            EngineIdentity::GraphZero,
            "expand",
            json!({"reference":"g:42"}),
        );
        assert_lower(
            "token",
            "expand",
            json!("q:abc"),
            EngineIdentity::GraphZero,
            "expand",
            json!({"reference":"q:abc"}),
        );
    }

    #[test]
    fn fs_edit_and_write_lower_to_fszero_with_args_passthrough() {
        assert!(METHODS.contains(&("fs", "edit")));
        assert!(METHODS.contains(&("fs", "write")));
        assert_lower(
            "fs",
            "edit",
            json!([{"path":"a.rs","find":"old","replace":"new",
                    "base":"fz://blob/aa"}]),
            EngineIdentity::FsZero,
            "fs.edit",
            json!({"path":"a.rs","find":"old","replace":"new",
                   "base":"fz://blob/aa"}),
        );
        assert_lower(
            "fs",
            "write",
            json!([{"path":"b.txt","content":"x","base":null}]),
            EngineIdentity::FsZero,
            "fs.write",
            json!({"path":"b.txt","content":"x","base":null}),
        );
        // Bare object (non-positional) form also passes through.
        assert_lower(
            "fs",
            "write",
            json!({"path":"c.txt","content":"y"}),
            EngineIdentity::FsZero,
            "fs.write",
            json!({"path":"c.txt","content":"y"}),
        );
        for input in [json!([]), json!(["c.txt"]), json!([{}, {}]), json!("c.txt")] {
            assert!(lower("fs", "edit", input.clone()).is_err());
            assert!(lower("fs", "write", input).is_err());
        }

        assert!(METHODS.contains(&("fs", "transact")));
        let steps = json!([
            {"op":"edit","path":"a.rs","find":"x","replace":"y"},
            {"op":"write","path":"b.txt","content":"z","base":null}
        ]);
        assert_lower(
            "fs",
            "transact",
            json!([steps.clone()]),
            EngineIdentity::FsZero,
            "fs.transact",
            json!({"steps": steps.clone()}),
        );
        // Spread form: zero.fs.transact(step, step).
        assert_lower(
            "fs",
            "transact",
            steps.clone(),
            EngineIdentity::FsZero,
            "fs.transact",
            json!({"steps": steps}),
        );
        for input in [json!([]), json!("steps"), json!(["a", "b"])] {
            assert!(lower("fs", "transact", input).is_err());
        }
    }

    #[test]
    fn token_read_and_shell_options_are_strict_and_forwarded_once() {
        assert!(METHODS.contains(&("token", "read")));
        assert_lower(
            "token",
            "read",
            json!(["fresh-raw.txt", {
                "mode":"exact","start_line":1,"end_line":2,"raw":true,
                "fresh":true,"max_files":1,"max_visible_tokens":512
            }]),
            EngineIdentity::TokenZero,
            "read",
            json!({
                "path":"fresh-raw.txt","mode":"exact","start_line":1,
                "end_line":2,"raw":true,"fresh":true,"max_files":1,
                "max_visible_tokens":512
            }),
        );
        assert_lower(
            "token",
            "read",
            json!(["one.txt", "two.txt"]),
            EngineIdentity::TokenZero,
            "read",
            json!({"path":["one.txt","two.txt"]}),
        );
        assert_lower(
            "token",
            "shell",
            json!([["printf", "ok"], {
                "cwd":".","mode":"exact","rewrite":"off","no_rewrite":true,
                "stdin":"input","timeout_ms":25,"timeout_seconds":1,"background":false
            }]),
            EngineIdentity::TokenZero,
            "shell",
            json!({
                "command":["printf","ok"],"cwd":".","mode":"exact",
                "rewrite":"off","no_rewrite":true,"stdin":"input",
                "timeout_ms":25,"timeout_seconds":1,"background":false
            }),
        );

        for input in [
            json!(["file", {"unknown":true}]),
            json!(["file", {"fresh":"yes"}]),
            json!(["file", {"max_files":0}]),
            json!(["file", {}, "extra"]),
        ] {
            assert!(lower("token", "read", input).is_err());
        }
        let shell_raw = lower(
            "token",
            "shell",
            json!(["printf must-not-run", {"raw":true}]),
        )
        .unwrap_err()
        .to_string();
        assert!(shell_raw.contains("unknown option 'raw'"), "{shell_raw}");
        assert!(shell_raw.contains(r#"mode: "exact""#), "{shell_raw}");
        assert!(lower("token", "shell", json!(["printf ok", {"timeout_ms":0}])).is_err());
        assert!(lower("token", "shell", json!(["printf ok", {}, "extra"])).is_err());
    }

    #[test]
    fn token_job_lowering_uses_the_shared_typed_request() {
        assert!(METHODS.contains(&("token", "job")));
        assert_lower(
            "token",
            "job",
            json!("tzjob-7"),
            EngineIdentity::TokenZero,
            "job",
            json!({"id":"tzjob-7","waitMs":30000,"since":0,"tailBytes":8192}),
        );
        assert_lower(
            "token",
            "job",
            json!(["tzjob-7", {"waitMs":25,"since":9,"tailBytes":64}]),
            EngineIdentity::TokenZero,
            "job",
            json!({"id":"tzjob-7","waitMs":25,"since":9,"tailBytes":64}),
        );
        assert!(lower("token", "job", json!(["tzjob-7", {"extra":true}])).is_err());
        assert!(lower("token", "job", json!(["tzjob-7", {"tailBytes":0}])).is_err());
        assert!(lower("token", "job", json!(["tzjob-7", {}, "extra"])).is_err());
    }

    #[test]
    fn expansion_routes_to_the_ref_owner() {
        for (reference, engine, op, key) in [
            ("fz://blob/00", EngineIdentity::FsZero, "fs.expand", "ref"),
            (
                "gz://blob/00",
                EngineIdentity::GraphZero,
                "expand",
                "reference",
            ),
            ("tz://blob/00", EngineIdentity::TokenZero, "expand", "ref"),
        ] {
            let mut expected = serde_json::Map::new();
            expected.insert(key.into(), Value::String(reference.into()));
            assert_lower(
                "token",
                "expand",
                json!(reference),
                engine,
                op,
                Value::Object(expected),
            );
        }
        assert!(lower("token", "expand", json!("https://invalid")).is_err());
    }
}
