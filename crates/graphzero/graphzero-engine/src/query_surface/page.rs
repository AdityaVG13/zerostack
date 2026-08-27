//! Durable cursor pages for large query/blast result sets (graphzero-gtub).
//!
//! Cursors are `gz://query/<id>` spills, not RAM tokens, so they survive process
//! restart (RACC durability). A session LRU of 20 tracks recently advertised
//! cursors; eviction does not delete spilled bytes.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};

pub const PAGE_SCHEMA: &str = "graphzero.page.v1";
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
    Some(format!("gz://query/{id}"))
}

pub fn load_page(store_root: &Path, cursor: &str) -> Option<Value> {
    let id = cursor
        .strip_prefix("gz://query/")
        .or_else(|| cursor.strip_prefix("gz://q/"))
        .or_else(|| cursor.strip_prefix("q:"))?;
    if id.is_empty() || id.chars().any(|c| !c.is_ascii_hexdigit()) {
        return None;
    }
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
