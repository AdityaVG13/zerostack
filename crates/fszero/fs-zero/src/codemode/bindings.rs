//! Step bindings — resolve `$id`, `$id.path`, `$stepN` in plan args.

use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct StepBinding {
    pub id: String,
    pub method: String,
    pub recovery_key: String,
    pub payload: Vec<u8>,
    pub path_hint: Option<String>,
}

#[derive(Debug, Default)]
pub struct BindingStore {
    by_id: HashMap<String, StepBinding>,
    by_step_index: HashMap<usize, String>,
}

impl BindingStore {
    pub fn register(&mut self, binding: StepBinding) {
        self.by_id.insert(binding.id.clone(), binding);
    }

    pub fn register_step_index(&mut self, step_index: usize, id: &str) {
        self.by_step_index.insert(step_index, id.to_string());
    }

    pub fn get(&self, handle: &str) -> Option<&StepBinding> {
        if let Some(id) = handle.strip_prefix("step") {
            if id.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(idx) = id.parse::<usize>() {
                    if let Some(bound_id) = self.by_step_index.get(&idx) {
                        return self.by_id.get(bound_id);
                    }
                }
            }
        }
        self.by_id.get(handle)
    }

    /// Resolve `$id` bindings in one step's args.
    ///
    /// `fs.multiAstSearch` item patterns are AST metavariables (`fn $NAME`), not
    /// step bindings, so those strings are passed through verbatim — otherwise
    /// every metavariable would be reported as an unknown binding.
    pub fn resolve_args(&self, method: &str, args: &Value) -> Result<Value, String> {
        if method != "fs.multiAstSearch" {
            return resolve_value(args, self);
        }
        let Some(map) = args.as_object() else {
            return resolve_value(args, self);
        };
        let mut out = serde_json::Map::new();
        for (key, value) in map {
            if key != "items" {
                out.insert(key.clone(), resolve_value(value, self)?);
                continue;
            }
            let Some(items) = value.as_array() else {
                out.insert(key.clone(), resolve_value(value, self)?);
                continue;
            };
            let mut resolved_items = Vec::with_capacity(items.len());
            for item in items {
                let Some(item_map) = item.as_object() else {
                    resolved_items.push(resolve_value(item, self)?);
                    continue;
                };
                let mut resolved_item = serde_json::Map::new();
                for (field, field_value) in item_map {
                    if field == "pattern" {
                        resolved_item.insert(field.clone(), field_value.clone());
                    } else {
                        resolved_item.insert(field.clone(), resolve_value(field_value, self)?);
                    }
                }
                resolved_items.push(Value::Object(resolved_item));
            }
            out.insert(key.clone(), Value::Array(resolved_items));
        }
        Ok(Value::Object(out))
    }
}

pub fn binding_from_parallel(
    branch_id: &str,
    method: &str,
    recovery_key: &str,
    payload: &[u8],
) -> StepBinding {
    StepBinding {
        id: branch_id.to_string(),
        method: method.to_string(),
        recovery_key: recovery_key.to_string(),
        payload: payload.to_vec(),
        path_hint: path_hint_for_method(method, payload, None),
    }
}

pub fn binding_from_call(
    id: &str,
    method: &str,
    recovery_key: &str,
    payload: &[u8],
    args: &Value,
) -> StepBinding {
    let path_arg = args
        .as_object()
        .and_then(|m| m.get("path"))
        .and_then(Value::as_str)
        .map(str::to_string);
    StepBinding {
        id: id.to_string(),
        method: method.to_string(),
        recovery_key: recovery_key.to_string(),
        payload: payload.to_vec(),
        path_hint: path_hint_for_method(method, payload, path_arg.as_deref()),
    }
}

fn path_hint_for_method(method: &str, payload: &[u8], path_arg: Option<&str>) -> Option<String> {
    if let Some(p) = path_arg {
        if !p.is_empty() {
            return Some(p.to_string());
        }
    }
    match method {
        "fs.read" | "fs.stat" => path_arg.map(str::to_string),
        "fs.search" => first_path_from_search_payload(payload),
        "fs.ls" => first_path_from_ls_payload(payload),
        _ => None,
    }
}

fn first_path_from_search_payload(payload: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(payload).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("HIT ") {
            let target = rest.split_whitespace().next()?;
            let path = target.split('#').next()?.trim();
            if !path.is_empty() {
                return Some(path.to_string());
            }
        }
        for prefix in ["DEF: ", "ASGREP: ", "CALLER: "] {
            if let Some(rest) = line.strip_prefix(prefix) {
                let fk = rest.split(':').next()?.trim();
                if !fk.is_empty() {
                    return Some(fk.to_string());
                }
            }
        }
        if let Some((fk, _)) = line.split_once(':') {
            let fk = fk.trim();
            if fk.contains('/') || fk.contains('.') {
                return Some(fk.to_string());
            }
        }
    }
    None
}

fn first_path_from_ls_payload(payload: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(payload).ok()?;
    for line in text.lines() {
        let line = line.trim().trim_end_matches('/');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        return Some(line.to_string());
    }
    None
}

fn first_path_from_stat_payload(payload: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(payload).ok()?;
    text.lines().find_map(|line| {
        line.strip_prefix("path=")
            .map(str::trim)
            .map(str::to_string)
    })
}

fn resolve_value(value: &Value, store: &BindingStore) -> Result<Value, String> {
    match value {
        Value::String(s) => Ok(Value::String(resolve_string(s, store)?)),
        Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                out.push(resolve_value(item, store)?);
            }
            Ok(Value::Array(out))
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                out.insert(k.clone(), resolve_value(v, store)?);
            }
            Ok(Value::Object(out))
        }
        _ => Ok(value.clone()),
    }
}

fn resolve_string(raw: &str, store: &BindingStore) -> Result<String, String> {
    if !raw.contains('$') {
        return Ok(raw.to_string());
    }
    if raw.starts_with('$') && !raw.contains(' ') {
        return resolve_binding_token(raw, store);
    }
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            out.push(ch);
            continue;
        }
        let mut token = String::from('$');
        while let Some(&next) = chars.peek() {
            if next.is_alphanumeric() || next == '_' || next == '.' {
                token.push(next);
                chars.next();
            } else {
                break;
            }
        }
        let resolved = resolve_binding_token(&token, store)?;
        out.push_str(&resolved);
    }
    Ok(out)
}

fn resolve_binding_token(token: &str, store: &BindingStore) -> Result<String, String> {
    let body = token
        .strip_prefix('$')
        .ok_or_else(|| format!("bad binding token: {token}"))?;
    let (handle, field) = if let Some((h, f)) = body.split_once('.') {
        (h, Some(f))
    } else {
        (body, None)
    };
    let binding = store
        .get(handle)
        .ok_or_else(|| format!("unknown binding '${handle}'"))?;
    resolve_field(binding, field)
}

fn resolve_field(binding: &StepBinding, field: Option<&str>) -> Result<String, String> {
    match field {
        None => default_binding_value(binding),
        Some("ref") => Ok(binding.recovery_key.clone()),
        Some("path") => binding
            .path_hint
            .clone()
            .or_else(|| first_path_from_search_payload(&binding.payload))
            .or_else(|| first_path_from_stat_payload(&binding.payload))
            .ok_or_else(|| format!("binding '{}' has no path", binding.id)),
        Some("payload") => Ok(String::from_utf8_lossy(&binding.payload).into_owned()),
        Some(other) => Err(format!(
            "unknown binding field '{other}' on '{}'",
            binding.id
        )),
    }
}

fn default_binding_value(binding: &StepBinding) -> Result<String, String> {
    match binding.method.as_str() {
        "fs.read" | "fs.stat" => resolve_field(binding, Some("path")),
        "fs.search" => resolve_field(binding, Some("path")),
        "fs.ls" => binding
            .path_hint
            .clone()
            .or_else(|| first_path_from_ls_payload(&binding.payload))
            .ok_or_else(|| format!("binding '{}' has no ls path", binding.id)),
        _ => Ok(binding.recovery_key.clone()),
    }
}
