//! Canonical surface-to-domain lowering for the single-process ZSX core.
//!
//! `ZsxSession`, the `zsx` executable, and native harness bindings use this
//! lowering authority directly. No process-backed compatibility copy remains.

use serde_json::Value;
use zero_abi::raw_worker::EngineIdentity;
use zero_abi::{TOKEN_JOB_OPERATION, TokenJobPollRequest};
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
    ("fs", "multi_edit"),
    ("fs", "multi_read"),
    ("fs", "multi_list"),
    ("fs", "multi_search"),
    ("fs", "multi_ast_search"),
    ("graph", "blast"),
    ("graph", "query"),
    ("graph", "multi_query"),
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
    ("help", "catalog"),
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

/// Portable blob expand only. Query / node / file / path refs are not
/// expand targets (they used to hit GraphZero/FSZero parsers and dump
/// the payload into a garbled malformed-parse error).
const EXPAND_BLOB_OWNERS: &[(&str, EngineIdentity, &str, &str)] = &[
    ("fz://blob/", EngineIdentity::FsZero, "fs.expand", "ref"),
    ("gz://blob/", EngineIdentity::GraphZero, "expand", "reference"),
    ("tz://blob/", EngineIdentity::TokenZero, "expand", "ref"),
];

const EXPAND_SCHEMES_HELP: &str = "tz://blob, fz://blob, gz://blob";

const COMPOUND_OPS: &[(&str, &str)] = &[
    ("read", "fs.read"),
    ("search", "fs.search"),
    ("find", "fs.search"),
    ("grep", "fs.search"),
    ("list", "fs.ls"),
    ("tree", "fs.ls"),
    ("inventory", "fs.ls"),
    ("mutate", "fs.edit"),
    ("edit", "fs.edit"),
    ("verifiedEdit", "fs.edit"),
    ("write", "fs.write"),
    ("resolve", "fs.resolve"),
];

const FS_VECTOR_METHODS: &[(&str, &str, &str)] = &[
    ("multi_read", "fs.multiRead", "paths"),
    ("multi_list", "fs.multiList", "items"),
    ("multi_search", "fs.multiSearch", "queries"),
    ("multi_ast_search", "fs.multiAstSearch", "items"),
];

const PLAN_STOPWORDS: &[&str] = &[
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

fn unsupported_method(surface: &str) -> ConnectorError {
    ConnectorError::new(format!(
        "unsupported {surface} method; discover call shapes with zero.help.search({{query}})"
    ))
}

fn normalize_token_args(
    input: &Value,
    method: &str,
    first_key: &str,
) -> Result<serde_json::Map<String, Value>, ConnectorError> {
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
    Ok(args)
}

fn validate_token_option(expected: TokenOptionType, value: &Value) -> bool {
    const TOKEN_MODES: &[&str] = &[
        "auto",
        "hybrid",
        "passthrough",
        "diagnostic",
        "critical",
        "structured",
        "fidelity",
        "dedupe",
        "diff-aware",
        "diff_aware",
        "diffaware",
        "exact",
        "lossy",
    ];
    match expected {
        TokenOptionType::Bool => value.is_boolean(),
        TokenOptionType::PositiveInteger => value.as_u64().is_some_and(|number| number > 0),
        TokenOptionType::String => value.is_string(),
        TokenOptionType::Mode => value
            .as_str()
            .is_some_and(|mode| TOKEN_MODES.contains(&mode)),
    }
}

fn token_method_args(
    input: &Value,
    method: &str,
    first_key: &str,
    contract: &[(&str, TokenOptionType)],
) -> Result<Value, ConnectorError> {
    let mut args = normalize_token_args(input, method, first_key)?;
    if method == "shell"
        && let Some(timeout_ms) = args.remove("timeoutMs")
    {
        if args.insert("timeout_ms".into(), timeout_ms).is_some() {
            return Err(ConnectorError::new(
                "token.shell options must not include both 'timeoutMs' and 'timeout_ms'",
            ));
        }
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
        if !validate_token_option(*expected, value) {
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
    let request: TokenJobPollRequest = serde_json::from_value(candidate)
        .map_err(|error| ConnectorError::new(format!("invalid token.job arguments: {error}")))?;
    request
        .validate()
        .map_err(|error| ConnectorError::new(format!("invalid token.job arguments: {error}")))?;
    serde_json::to_value(request).map_err(|error| ConnectorError::new(error.to_string()))
}

fn short_ref(reference: &str) -> String {
    const MAX: usize = 72;
    if reference.len() <= MAX {
        return reference.to_owned();
    }
    let mut end = MAX;
    while end > 0 && !reference.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &reference[..end])
}

fn is_location_expand_ref(reference: &str) -> bool {
    reference.starts_with("fz://file/")
        || reference.starts_with("file://")
        || (!reference.contains("://") && !reference.starts_with("g:") && !reference.starts_with("q:"))
}

fn lower_token_expand(input: &Value) -> Result<(EngineIdentity, String, Value), ConnectorError> {
    let reference = input
        .as_array()
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .or_else(|| input.get("ref").and_then(Value::as_str))
        .or_else(|| input.as_str())
        .ok_or_else(|| ConnectorError::new("token.expand requires ref"))?;
    if is_location_expand_ref(reference) {
        let preview = short_ref(reference);
        return Err(ConnectorError::new(format!(
            "cannot expand location ref {preview}; expected a portable blob ({EXPAND_SCHEMES_HELP}). \
             For a HIT window use zero.fs.compound(\"read\", {{path, start_line, end_line}}) with #Lstart-Lend \
             (1-based inclusive). A bare #L25 is not a window — use #L25-L25 or start_line/end_line."
        )));
    }
    if let Some((_, engine, op, key)) = EXPAND_BLOB_OWNERS
        .iter()
        .find(|(prefix, _, _, _)| reference.starts_with(prefix))
    {
        if zero_ref::ZeroRef::parse(reference).is_err() {
            return Err(ConnectorError::new(format!(
                "cannot expand {}; expected {EXPAND_SCHEMES_HELP} with a 64-hex digest and optional #Bstart-end or #Lstart-end",
                short_ref(reference)
            )));
        }
        let mut args = serde_json::Map::new();
        args.insert((*key).into(), Value::String(reference.into()));
        return Ok((*engine, (*op).into(), Value::Object(args)));
    }
    if reference.starts_with("gz://query/") || reference.starts_with("q:") {
        return Err(ConnectorError::new(format!(
            "cannot expand query ref {}; expected a portable blob ({EXPAND_SCHEMES_HELP}). \
             Use the query result's evidence_ref / gz://blob/…, not gz://query/…",
            short_ref(reference)
        )));
    }
    if reference.starts_with("gz://") || reference.starts_with("g:") {
        return Err(ConnectorError::new(format!(
            "cannot expand graph ref {}; expected a portable blob ({EXPAND_SCHEMES_HELP})",
            short_ref(reference)
        )));
    }
    Err(ConnectorError::new(format!(
        "unsupported ref scheme on {}; valid expand schemes: {EXPAND_SCHEMES_HELP}",
        short_ref(reference)
    )))
}

fn lower_fs_plan(engine: EngineIdentity, input: &Value) -> (EngineIdentity, String, Value) {
    let goal = input
        .as_array()
        .and_then(|values| values.first())
        .or_else(|| input.get("goal"))
        .and_then(Value::as_str)
        .or_else(|| input.as_str())
        .unwrap_or_default();
    let queries: Vec<Value> = goal
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .map(str::to_ascii_lowercase)
        .filter(|term| term.len() >= 3 && !PLAN_STOPWORDS.contains(&term.as_str()))
        .take(8)
        .map(Value::String)
        .collect();
    if queries.is_empty() {
        return (engine, "fs.ls".into(), serde_json::json!({"arg":"."}));
    }
    (
        engine,
        "fs.multiSearch".into(),
        serde_json::json!({"queries":queries}),
    )
}

fn lower_fs_world(input: Value) -> Result<Value, ConnectorError> {
    if let Some(items) = input.as_array() {
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
            return Ok(Value::Object(object));
        }
        if matches!(action, "fork") || action.contains(':') {
            return Ok(serde_json::json!({"arg": action}));
        }
        return Ok(serde_json::json!({"action": action}));
    }
    if input.is_object() {
        return Ok(input);
    }
    if let Some(action) = input.as_str() {
        return Ok(serde_json::json!({"arg": action}));
    }
    Err(ConnectorError::new(
        "fs.world requires an action string or object",
    ))
}

/// Stable token in the connector error so plans can branch without
/// string-matching prose. The corrective path is `ctx.payload` /
/// `payload_utf8` or expand-then-write, never `JSON.stringify(result)`.
const NON_BYTE_WRITE_CONTENT: &str = "non_byte_provenance";

fn write_content_error() -> ConnectorError {
    ConnectorError::new(format!(
        "{NON_BYTE_WRITE_CONTENT}: fs.write content must be a UTF-8 string, not a tool-result object. \
         Extract bytes with ctx.payload(result) (payload_utf8) or expand a content ref in the same plan; \
         do not pass the connector result as content"
    ))
}

fn is_utf8_string_content(value: &Value) -> bool {
    match value {
        Value::String(text) => text != "[object Object]",
        _ => false,
    }
}

/// Common agent misspellings of `content`. Folded before dispatch so a
/// `contents:` write does not create an empty file.
const WRITE_CONTENT_ALIASES: &[&str] = &["contents", "body", "text"];
const WRITE_KNOWN_KEYS: &[&str] = &[
    "path",
    "base",
    "content",
    "op",
    "contents",
    "body",
    "text",
];
const MISSING_WRITE_CONTENT: &str = "missing_write_content";
const UNKNOWN_WRITE_FIELD: &str = "unknown_write_field";
const WRITE_CONTENT_CONFLICT: &str = "write_content_conflict";

/// Reject tool-result / provenance objects smuggled as `content`.
/// Missing `content` is allowed for `fs.edit` (find/replace). Writes go
/// through [`normalize_write_args`].
fn reject_non_byte_write_content(args: &Value) -> Result<(), ConnectorError> {
    let Some(map) = args.as_object() else {
        return Ok(());
    };
    match map.get("content") {
        None => Ok(()),
        Some(value) if is_utf8_string_content(value) => Ok(()),
        Some(_) => Err(write_content_error()),
    }
}

fn fold_write_content_aliases(args: &mut Value) -> Result<(), ConnectorError> {
    let Some(map) = args.as_object_mut() else {
        return Ok(());
    };
    let mut alias_hits: Vec<(String, Value)> = Vec::new();
    for key in WRITE_CONTENT_ALIASES {
        if let Some(value) = map.remove(*key) {
            alias_hits.push(((*key).to_string(), value));
        }
    }
    if alias_hits.is_empty() {
        return Ok(());
    }
    if let Some(existing) = map.get("content") {
        if alias_hits.iter().any(|(_, value)| value != existing) {
            let names = alias_hits
                .iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>()
                .join("/");
            return Err(ConnectorError::new(format!(
                "{WRITE_CONTENT_CONFLICT}: fs.write got both content and {names}; keep one UTF-8 string in content"
            )));
        }
        return Ok(());
    }
    let first = alias_hits[0].1.clone();
    if alias_hits.iter().any(|(_, value)| value != &first) {
        let names = alias_hits
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>()
            .join("/");
        return Err(ConnectorError::new(format!(
            "{WRITE_CONTENT_CONFLICT}: fs.write got conflicting {names}; keep one UTF-8 string in content"
        )));
    }
    map.insert("content".into(), first);
    Ok(())
}

fn reject_unknown_write_keys(args: &Value) -> Result<(), ConnectorError> {
    let Some(map) = args.as_object() else {
        return Ok(());
    };
    let mut unknown: Vec<&str> = map
        .keys()
        .map(String::as_str)
        .filter(|key| !WRITE_KNOWN_KEYS.contains(key))
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    unknown.sort_unstable();
    Err(ConnectorError::new(format!(
        "{UNKNOWN_WRITE_FIELD}: fs.write does not take {}; use content (also accepted: contents, body, text)",
        unknown.join(", ")
    )))
}

fn require_write_content_string(args: &Value) -> Result<(), ConnectorError> {
    let Some(map) = args.as_object() else {
        return Ok(());
    };
    match map.get("content") {
        None => Err(ConnectorError::new(format!(
            "{MISSING_WRITE_CONTENT}: fs.write needs a UTF-8 string in content \
             (also accepted: contents, body, text). Use content: \"\" to create an empty file"
        ))),
        Some(value) if is_utf8_string_content(value) => Ok(()),
        Some(_) => Err(write_content_error()),
    }
}

fn normalize_write_args(mut args: Value) -> Result<Value, ConnectorError> {
    fold_write_content_aliases(&mut args)?;
    reject_unknown_write_keys(&args)?;
    require_write_content_string(&args)?;
    Ok(args)
}

/// Fill `op` when a batch step omits it: find/replace → edit, else write.
fn default_batch_step_op(step: &mut Value) -> Result<(), ConnectorError> {
    let Some(map) = step.as_object_mut() else {
        return Err(ConnectorError::new(
            "fs.multi_edit / fs.transact steps must be objects",
        ));
    };
    if map.get("op").and_then(Value::as_str).is_some() {
        return Ok(());
    }
    let has_find = map.get("find").or_else(|| map.get("old")).is_some();
    let has_replace = map.get("replace").or_else(|| map.get("new")).is_some();
    if has_find && has_replace {
        map.insert("op".into(), Value::String("edit".into()));
        return Ok(());
    }
    if has_find || has_replace {
        return Err(ConnectorError::new(
            "fs.multi_edit edit step needs both find/old and replace/new",
        ));
    }
    if map.get("content").is_some()
        || WRITE_CONTENT_ALIASES
            .iter()
            .any(|key| map.get(*key).is_some())
    {
        map.insert("op".into(), Value::String("write".into()));
        return Ok(());
    }
    if map.get("path").is_some() {
        return Err(ConnectorError::new(
            "fs.multi_edit write step needs content (path-only is not an implicit write)",
        ));
    }
    Err(ConnectorError::new(
        "fs.multi_edit step needs op, find/replace, or content",
    ))
}

fn collect_batch_steps(input: Value) -> Result<Value, ConnectorError> {
    let mut steps = match input.as_array() {
        Some(items) if items.len() == 1 && items[0].is_array() => items[0]
            .as_array()
            .cloned()
            .unwrap_or_default(),
        Some(items) if !items.is_empty() && items.iter().all(Value::is_object) => items.clone(),
        _ => {
            return Err(ConnectorError::new(
                "fs.transact / fs.multi_edit take one non-empty array of step objects",
            ));
        }
    };
    if steps.is_empty() {
        return Err(ConnectorError::new(
            "fs.transact / fs.multi_edit take one non-empty array of step objects",
        ));
    }
    for step in &mut steps {
        default_batch_step_op(step)?;
    }
    Ok(Value::Array(steps))
}

fn normalize_transact_writes(steps: &mut Value) -> Result<(), ConnectorError> {
    let Some(items) = steps.as_array_mut() else {
        return Ok(());
    };
    for step in items {
        let is_write = step
            .get("op")
            .and_then(Value::as_str)
            .is_some_and(|op| op == "write");
        if is_write {
            *step = normalize_write_args(std::mem::take(step))?;
        }
    }
    Ok(())
}

fn exactly_one_options_object(input: Value) -> Result<Value, ConnectorError> {
    if let Some(items) = input.as_array() {
        if items.len() != 1 || !items[0].is_object() {
            return Err(ConnectorError::new(
                "fs.edit and fs.write take exactly one options object",
            ));
        }
        return Ok(items[0].clone());
    }
    if input.is_object() {
        return Ok(input);
    }
    Err(ConnectorError::new(
        "fs.edit and fs.write take exactly one options object",
    ))
}

/// Map help-advertised search aliases onto `fs.search`'s query/arg wire.
///
/// Callers pass `regex` / `pattern` (and optional `path`) because help
/// says `{query, path?}`. The kernel only reads `query`/`arg`; extra keys
/// were previously a silent miss, and regex-only calls failed "missing query".
/// `compound('list', {paths})` must become `fs.multiList` with `items`.
/// `compound('read', {paths})` keeps `paths` for `fs.multiRead`.
/// `compound('search', {query, path})` becomes `fs.multiSearch` so the
/// kernel's per-query `paths` filter actually scopes hits. Bare `fs.search`
/// ignores `path`.
fn lower_compound_search_scope(args: Value) -> Result<(String, Value), ConnectorError> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty() && *path != ".");
    let Some(path) = path else {
        return Ok(("fs.search".into(), args));
    };
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| ConnectorError::new("fs.compound search requires query"))?;
    let limit = args
        .get("max_results")
        .or_else(|| args.get("limit"))
        .cloned()
        .unwrap_or(serde_json::json!(80));
    Ok((
        "fs.multiSearch".into(),
        serde_json::json!({
            "queries": [{
                "query": query,
                "paths": [path],
                "limit": limit,
            }]
        }),
    ))
}

fn remap_compound_paths_args(
    name: &str,
    mut args: Value,
) -> Result<(String, Value), ConnectorError> {
    match name {
        "read" => Ok(("fs.multiRead".into(), args)),
        "list" | "inventory" | "tree" => {
            if let Some(object) = args.as_object_mut() {
                if let Some(paths) = object.remove("paths") {
                    object.insert("items".into(), paths);
                }
            }
            Ok(("fs.multiList".into(), args))
        }
        _ => Err(ConnectorError::new(
            "fs.compound paths= is only valid for read/list/inventory",
        )),
    }
}

fn normalize_compound_search_args(args: Value) -> Value {
    let Value::Object(mut map) = args else {
        return args;
    };
    let has_query = map
        .get("query")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty())
        || map
            .get("arg")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty());
    if !has_query {
        for key in ["regex", "pattern", "q"] {
            if let Some(value) = map.get(key).cloned() {
                if value.as_str().is_some_and(|s| !s.is_empty()) {
                    map.insert("query".into(), value);
                    break;
                }
            }
        }
    }
    // Keep `path` on the object so a later kernel that honors it sees it.
    // fs.search still matches the query workspace-wide (kernel grammar
    // change is out of scope for this bead).
    Value::Object(map)
}

fn compound_name_and_args(input: &Value) -> Result<(&str, Value), ConnectorError> {
    if let Some(items) = input.as_array() {
        let name = items
            .first()
            .and_then(Value::as_str)
            .ok_or_else(|| ConnectorError::new("fs.compound requires name"))?;
        let args = items
            .get(1)
            .cloned()
            .unwrap_or(Value::Object(Default::default()));
        return Ok((name, args));
    }
    let name = input
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ConnectorError::new("fs.compound requires name"))?;
    let args = input
        .get("args")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    Ok((name, args))
}

/// Lower one public `surface.method` call to its canonical domain operation.
///
/// Returns `(engine, operation, args)` exactly as the process compatibility
/// path does; expansion routing follows the ref scheme owner.
///
/// Bill (Wave F): F1–F5 + L1–L4 + C1/C2/C5 are justified-displacement
/// (named unique blocks). Remaining `lower` CC is the public surface/method
/// router. ΣCC is flat except the three shared tails (edit/write options,
/// compound name, unsupported method). F6 `normalize_token_job_candidate`
/// and L5 `engine_for` table are not done -- `help` must stay rejected.
pub fn lower(
    surface: &str,
    method: &str,
    input: Value,
) -> Result<(EngineIdentity, String, Value), ConnectorError> {
    let engine = engine_for(surface)?;
    if surface == "token" && method == "expand" {
        return lower_token_expand(&input);
    }
    if surface == "fs" && method == "plan" {
        return Ok(lower_fs_plan(engine, &input));
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
    // and-swap against current content). Write `content` is hub-gated:
    // a tool-result object must not stringify into a tracked file.
    // `contents`/`body`/`text` fold to `content`; unknown keys fail loud.
    if surface == "fs" && method == "write" {
        let args = normalize_write_args(exactly_one_options_object(input)?)?;
        return Ok((engine, "fs.write".into(), args));
    }
    if surface == "fs" && method == "edit" {
        let args = exactly_one_options_object(input)?;
        reject_non_byte_write_content(&args)?;
        return Ok((engine, "fs.edit".into(), args));
    }
    // All-or-nothing multi-file mutation. multi_edit is the dogfood name
    // (implicit op:edit|write); transact is the kernel.
    if surface == "fs" && matches!(method, "transact" | "multi_edit") {
        let mut steps = collect_batch_steps(input)?;
        normalize_transact_writes(&mut steps)?;
        return Ok((engine, "fs.transact".into(), serde_json::json!({"steps": steps})));
    }
    if surface == "fs" && method == "compound" {
        let (name, compound_args) = compound_name_and_args(&input)?;
        let compound_args = if matches!(name, "search" | "find" | "grep") {
            normalize_compound_search_args(compound_args)
        } else {
            compound_args
        };
        if matches!(name, "search" | "find" | "grep") {
            let (op, compound_args) = lower_compound_search_scope(compound_args)?;
            return Ok((engine, op, compound_args));
        }
        if compound_args.get("paths").is_some() {
            let (op, compound_args) = remap_compound_paths_args(name, compound_args)?;
            return Ok((engine, op, compound_args));
        }
        let op = COMPOUND_OPS
            .iter()
            .find(|(alias, _)| *alias == name)
            .map(|(_, op)| *op)
            .ok_or_else(|| {
                ConnectorError::new(
                    "unsupported planner-free fs.compound operation; discover call shapes with zero.help.search({query})",
                )
            })?;
        if matches!(op, "fs.write" | "fs.edit") && !compound_args.is_object() {
            return Err(ConnectorError::new(
                "fs.edit and fs.write take exactly one options object",
            ));
        }
        if op == "fs.write" {
            let compound_args = normalize_write_args(compound_args)?;
            return Ok((engine, op.into(), compound_args));
        }
        if op == "fs.edit" {
            reject_non_byte_write_content(&compound_args)?;
        }
        return Ok((engine, op.into(), compound_args));
    }
    if surface == "fs" && method == "world" {
        return Ok((engine, "fs.world".into(), lower_fs_world(input)?));
    }
    if surface == "fs" {
        let (op, key) = FS_VECTOR_METHODS
            .iter()
            .find(|(name, _, _)| *name == method)
            .map(|(_, op, key)| (*op, *key))
            .ok_or_else(|| unsupported_method("fs"))?;
        return Ok((engine, op.into(), vector_args(&input, key)));
    }
    if surface == "graph" {
        let args = match method {
            "blast" => positional_args(&input, "intent", None),
            "multi_query" => {
                let mut args = positional_args(&input, "surface", Some("targets"));
                if let Some(object) = args.as_object_mut() {
                    if !object.contains_key("targets") {
                        if let Some(query) = object.remove("query") {
                            object.insert("targets".into(), query);
                        }
                    }
                }
                args
            }
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
            _ => return Err(unsupported_method("graph")),
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
        "job" => (TOKEN_JOB_OPERATION, token_job_args(&input)?),
        "shell" => {
            let mut args = token_method_args(&input, "shell", "command", TOKEN_SHELL_OPTIONS)?;
            if let Some(object) = args.as_object_mut() {
                if object.get("command").is_some_and(Value::is_array) {
                    if let Some(argv) = object.remove("command") {
                        object.insert("argv".into(), argv);
                    }
                }
            }
            ("shell", args)
        }
        _ => return Err(unsupported_method("token")),
    };
    Ok((engine, op.into(), args))
}
