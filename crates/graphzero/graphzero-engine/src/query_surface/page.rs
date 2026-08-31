//! Durable `query/<id>` cursor pages for large query and blast result sets.
//! Stored pages survive process restart and never depend on RAM tokens.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};

pub const PAGE_SCHEMA: &str = "graphzero.page";
pub const SESSION_CURSOR_CAP: usize = 20;

fn session_cursors() -> &'static Mutex<HashMap<String, VecDeque<String>>> {
    static MAP: OnceLock<Mutex<HashMap<String, VecDeque<String>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn remember_session_cursor(session: Option<&str>, cursor: &str) {
    let Some(session) = session.filter(|s| !s.is_empty()) else {
        return;
    };
    let Ok(mut map) = session_cursors().lock() else {
        return;
    };
    let queue = map.entry(session.to_string()).or_default();
    queue.retain(|existing| existing != cursor);
    queue.push_back(cursor.to_string());
    while queue.len() > SESSION_CURSOR_CAP {
        queue.pop_front();
    }
}

pub fn spill_page(store_root: Option<&Path>, page: &Value) -> Option<String> {
    let root = store_root?;
    let body = serde_json::to_string(page).ok()?;
    let id = graphzero_store::store::query::persist_query_json(root, &body).ok()?;
    Some(format!("query/{id}"))
}

/// Extract the hexadecimal identity from a canonical or compact cursor.
pub fn query_cursor_id(cursor: &str) -> Option<&str> {
    let id = cursor
        .strip_prefix("query/")
        .or_else(|| cursor.strip_prefix("q:"))?;
    if id.is_empty() || id.chars().any(|c| !c.is_ascii_hexdigit()) {
        None
    } else {
        Some(id)
    }
}

pub fn load_page(store_root: &Path, cursor: &str) -> Option<Value> {
    let id = query_cursor_id(cursor)?;
    let path = store_root.join("queries").join(format!("{id}.json"));
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn page_document(kind: &str, payload: Value) -> Value {
    json!({
        "schema": PAGE_SCHEMA,
        "kind": kind,
        "payload": payload,
    })
}

pub fn payload_if_kind(page: &Value, kind: &str) -> Option<Value> {
    if page.get("schema").and_then(Value::as_str) != Some(PAGE_SCHEMA) {
        return None;
    }
    if page.get("kind").and_then(Value::as_str) != Some(kind) {
        return None;
    }
    page.get("payload").cloned()
}
