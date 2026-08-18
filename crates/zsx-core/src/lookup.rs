//! Bounded indexed filename/path lookup for ZeroStack.
//!
//! Planner-free, approval-free, read-only lookup scoped to the approved
//! workspace root. Explicit `root`, explicit bounded `limit`, deterministic
//! lexicographic ordering, and no file-content reads. Any root escape or
//! missing root fails closed. This is the narrow indexed replacement for
//! broad `shell find` calls that timed out in papercut `pc_2178c942f3ff`.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;
use zero_codemode::ConnectorError;

const DEFAULT_LOOKUP_LIMIT: usize = 20;
const MAX_LOOKUP_LIMIT: usize = 100;
const MAX_LOOKUP_DEPTH: usize = 64;
const MAX_VISITED_ENTRIES: usize = 10_000;

fn tokenize_query(raw: &str) -> String {
    raw.trim().to_owned()
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match_at(&p, 0, &t, 0)
}

fn glob_match_at(p: &[char], pi: usize, t: &[char], ti: usize) -> bool {
    if pi == p.len() {
        return ti == t.len();
    }
    if p[pi] == '*' {
        if pi + 1 < p.len() && p[pi + 1] == '*' {
            if pi + 2 < p.len() && p[pi + 2] == '/' {
                if glob_match_at(p, pi + 3, t, ti) {
                    return true;
                }
            }
            return (ti..=t.len()).any(|idx| glob_match_at(p, pi + 1, t, idx));
        }
        if pi + 1 == p.len() {
            return t[ti..].iter().all(|c| *c != '/');
        }
        return (ti..=t.len())
            .any(|idx| (idx == t.len() || t[ti..idx].iter().all(|c| *c != '/')) && glob_match_at(p, pi + 1, t, idx));
    }
    if ti == t.len() {
        return false;
    }
    if p[pi] == '?' {
        return t[ti] != '/' && glob_match_at(p, pi + 1, t, ti + 1);
    }
    p[pi] == t[ti] && glob_match_at(p, pi + 1, t, ti + 1)
}

fn connector_err(msg: impl Into<String>) -> ConnectorError {
    ConnectorError::new(msg.into())
}

fn lexical_join_and_validate(workspace_root: &Path, root: &str) -> Result<PathBuf, ConnectorError> {
    if root.is_empty() {
        return Err(connector_err("fs.lookup requires non-empty root"));
    }
    let p = Path::new(root);
    if p.is_absolute() {
        return Err(connector_err(format!(
            "fs.lookup root must be workspace-relative, got absolute '{}'",
            root
        )));
    }
    let workspace_canon = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    let mut clean = PathBuf::new();
    for comp in workspace_canon.components() {
        clean.push(comp.as_os_str());
    }
    for comp in p.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => {
                return Err(connector_err(format!(
                    "fs.lookup root '{}' escapes approved root",
                    root
                )))
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !clean.pop() {
                    return Err(connector_err(format!(
                        "fs.lookup root '{}' escapes approved root",
                        root
                    )));
                }
                if !clean.starts_with(&workspace_canon) {
                    return Err(connector_err(format!(
                        "fs.lookup root '{}' escapes approved root",
                        root
                    )));
                }
            }
            Component::Normal(c) => clean.push(c),
        }
    }
    if !clean.starts_with(&workspace_canon) {
        return Err(connector_err(format!(
            "fs.lookup root '{}' escapes approved root",
            root
        )));
    }
    if clean.exists() {
        if let Ok(canon) = clean.canonicalize() {
            if !canon.starts_with(&workspace_canon) {
                return Err(connector_err(format!(
                    "fs.lookup root '{}' escapes approved root via symlink",
                    root
                )));
            }
            return Ok(canon);
        }
    }
    Ok(clean)
}

pub fn lookup_search(workspace_root: &Path, input: &Value) -> Result<Value, ConnectorError> {
    let map = match input {
        Value::Object(m) => m.clone(),
        Value::Array(items) => {
            let mut m = serde_json::Map::new();
            if let Some(v) = items.first() {
                if let Some(s) = v.as_str() {
                    m.insert("root".into(), Value::String(s.to_owned()));
                } else {
                    m.insert("root".into(), v.clone());
                }
            }
            if let Some(v) = items.get(1) {
                if let Some(s) = v.as_str() {
                    m.insert("query".into(), Value::String(s.to_owned()));
                } else {
                    m.insert("query".into(), v.clone());
                }
            }
            if let Some(v) = items.get(2) {
                m.insert("limit".into(), v.clone());
            }
            m
        }
        Value::String(s) => {
            let mut m = serde_json::Map::new();
            m.insert("root".into(), Value::String(s.clone()));
            m
        }
        _ => return Err(connector_err("fs.lookup requires {root, query?, limit?}")),
    };

    let root_val = map
        .get("root")
        .or_else(|| map.get("path"))
        .or_else(|| map.get("dir"))
        .cloned();
    let query_val = map
        .get("query")
        .or_else(|| map.get("pattern"))
        .or_else(|| map.get("name"))
        .or_else(|| map.get("filename"))
        .or_else(|| map.get("glob"))
        .or_else(|| map.get("q"))
        .cloned();
    let limit_val = map
        .get("limit")
        .or_else(|| map.get("max_results"))
        .or_else(|| map.get("maxResults"))
        .or_else(|| map.get("max"))
        .cloned();

    let root_keys = ["root", "path", "dir"];
    let mut seen_root: Option<String> = None;
    for k in root_keys {
        if let Some(v) = map.get(k).and_then(Value::as_str) {
            if let Some(prev) = &seen_root {
                if prev != v {
                    return Err(connector_err(format!(
                        "fs.lookup conflicting roots '{}' vs '{}'",
                        prev, v
                    )));
                }
            } else {
                seen_root = Some(v.to_owned());
            }
        }
    }

    let root = match root_val {
        Some(Value::String(s)) => {
            if s.trim().is_empty() {
                return Err(connector_err("fs.lookup root must be non-empty"));
            }
            s
        }
        Some(v) => {
            if let Some(s) = v.as_str() {
                if s.trim().is_empty() {
                    return Err(connector_err("fs.lookup root must be non-empty"));
                }
                s.to_owned()
            } else {
                return Err(connector_err("fs.lookup root must be a string"));
            }
        }
        None => return Err(connector_err("fs.lookup requires root")),
    };

    let query = match query_val {
        Some(Value::String(s)) => {
            let q = tokenize_query(&s);
            if q.is_empty() { None } else { Some(q) }
        }
        Some(Value::Null) => None,
        Some(v) => {
            if let Some(s) = v.as_str() {
                let q = tokenize_query(s);
                if q.is_empty() { None } else { Some(q) }
            } else {
                return Err(connector_err("fs.lookup query must be a string"));
            }
        }
        None => None,
    };

    let limit = match limit_val {
        Some(Value::Number(n)) => {
            let Some(u) = n.as_u64() else {
                return Err(connector_err("fs.lookup limit must be a positive integer"));
            };
            if u == 0 {
                return Err(connector_err("fs.lookup limit must be >= 1"));
            }
            if u > MAX_LOOKUP_LIMIT as u64 {
                return Err(connector_err(format!(
                    "fs.lookup limit {} exceeds max {}",
                    u, MAX_LOOKUP_LIMIT
                )));
            }
            u as usize
        }
        Some(Value::String(s)) => {
            let Ok(u): Result<usize, _> = s.parse() else {
                return Err(connector_err("fs.lookup limit must be a positive integer"));
            };
            if u == 0 || u > MAX_LOOKUP_LIMIT {
                return Err(connector_err(format!(
                    "fs.lookup limit {} out of range 1..={}",
                    u, MAX_LOOKUP_LIMIT
                )));
            }
            u
        }
        Some(Value::Null) | None => DEFAULT_LOOKUP_LIMIT,
        Some(_) => return Err(connector_err("fs.lookup limit must be a positive integer")),
    };

    let target = lexical_join_and_validate(workspace_root, &root)?;
    let workspace_canon = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());

    let meta = std::fs::symlink_metadata(&target);
    let is_file = meta.as_ref().map(|m| m.is_file()).unwrap_or(false);
    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
    if meta.is_err() {
        let out = serde_json::json!({
            "operation": "fs.lookup",
            "ok": true,
            "root": root,
            "query": query,
            "limit": limit,
            "count": 0,
            "total": 0,
            "truncated": false,
            "paths": [],
            "results": [],
            "entries": [],
            "files": []
        });
        return Ok(out);
    }

    let mut results: Vec<String> = Vec::new();
    let mut visited = 0usize;

    if is_file {
        let rel = target
            .strip_prefix(&workspace_canon)
            .unwrap_or(target.as_path())
            .to_string_lossy()
            .replace('\\', "/");
        let file_name = target.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let matched = if let Some(q) = &query {
            if q.contains('*') || q.contains('?') {
                glob_match(q, &rel) || glob_match(q, file_name)
            } else {
                rel.contains(q) || file_name.contains(q)
            }
        } else {
            true
        };
        if matched {
            results.push(rel);
        }
    } else if is_dir {
        let mut stack: Vec<(PathBuf, usize)> = vec![(target.clone(), 0)];
        while let Some((dir, depth)) = stack.pop() {
            if depth > MAX_LOOKUP_DEPTH {
                continue;
            }
            if visited > MAX_VISITED_ENTRIES {
                break;
            }
            let rd = match std::fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            let mut entries: Vec<(PathBuf, bool)> = Vec::new();
            for ent in rd {
                let ent = match ent { Ok(e) => e, Err(_) => continue };
                let ft = match ent.file_type() { Ok(f) => f, Err(_) => continue };
                if ft.is_symlink() {
                    continue;
                }
                let p = ent.path();
                let is_dir = ft.is_dir();
                entries.push((p, is_dir));
                visited += 1;
                if visited > MAX_VISITED_ENTRIES {
                    break;
                }
            }
            entries.sort_by(|a, b| a.0.file_name().cmp(&b.0.file_name()));
            for (path, is_dir_entry) in entries {
                let rel = path
                    .strip_prefix(&workspace_canon)
                    .unwrap_or(path.as_path())
                    .to_string_lossy()
                    .replace('\\', "/");
                let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let matched = if let Some(q) = &query {
                    if q.contains('*') || q.contains('?') {
                        glob_match(q, &rel) || glob_match(q, file_name)
                    } else {
                        rel.contains(q) || file_name.contains(q)
                    }
                } else {
                    true
                };
                if matched {
                    results.push(rel.clone());
                }
                if is_dir_entry && depth + 1 <= MAX_LOOKUP_DEPTH {
                    stack.push((path, depth + 1));
                }
            }
        }
    }

    results.sort();
    let total = results.len();
    let truncated = total > limit;
    let truncated_results: Vec<String> = results.into_iter().take(limit).collect();
    let count = truncated_results.len();

    let out = serde_json::json!({
        "operation": "fs.lookup",
        "ok": true,
        "root": root,
        "query": query,
        "limit": limit,
        "count": count,
        "total": total,
        "truncated": truncated,
        "paths": truncated_results.clone(),
        "results": truncated_results.clone(),
        "entries": truncated_results.clone(),
        "files": truncated_results.clone()
    });
    Ok(out)
}
