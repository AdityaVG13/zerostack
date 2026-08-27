//! Per-artifact-class canonical forms (fszero-i5px / omega gauge-fixing substrate).
//!
//! Canonicalization is pure identity-preserving for semantic equality: sorted
//! keys, stable ordering, normalized insignificant whitespace. It never claims
//! unlabeled Q99 savings.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactClass {
    RepoMap,
    OrientPack,
    SearchResults,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalizeError {
    InvalidJson(String),
    WrongShape(String),
}

impl std::fmt::Display for CanonicalizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson(s) => write!(f, "invalid json: {s}"),
            Self::WrongShape(s) => write!(f, "wrong shape: {s}"),
        }
    }
}
impl std::error::Error for CanonicalizeError {}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

pub fn digest_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex_encode(h.finalize().as_slice())
}

/// Normalize insignificant whitespace (collapse runs of space/tab; trim lines).
fn normalize_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.lines() {
        let mut last_space = false;
        for ch in line.trim().chars() {
            if ch == ' ' || ch == '\t' {
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
            } else {
                out.push(ch);
                last_space = false;
            }
        }
        out.push('\n');
    }
    out
}

/// Canonical JSON object: BTreeMap key order, recursive.
fn canonicalize_json_value(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let mut ordered = serde_json::Map::new();
            let mut keys: Vec<_> = map.keys().cloned().collect();
            keys.sort();
            for k in keys {
                ordered.insert(k.clone(), canonicalize_json_value(&map[&k]));
            }
            serde_json::Value::Object(ordered)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(canonicalize_json_value).collect())
        }
        serde_json::Value::String(s) => {
            serde_json::Value::String(normalize_ws(s).trim().to_string())
        }
        other => other.clone(),
    }
}

/// Repo-map: `{ "files": [ { "path", "digest" }, ... ] }` sorted by path.
pub fn canonicalize_repo_map(raw: &str) -> Result<Vec<u8>, CanonicalizeError> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| CanonicalizeError::InvalidJson(e.to_string()))?;
    let files = v
        .get("files")
        .and_then(|x| x.as_array())
        .ok_or_else(|| CanonicalizeError::WrongShape("repo-map needs files[]".into()))?;
    let mut by_path: BTreeMap<String, String> = BTreeMap::new();
    for f in files {
        let path = f
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| CanonicalizeError::WrongShape("file.path".into()))?
            .to_string();
        let digest = f
            .get("digest")
            .and_then(|p| p.as_str())
            .unwrap_or("")
            .to_string();
        by_path.insert(path, digest);
    }
    let files: Vec<serde_json::Value> = by_path
        .into_iter()
        .map(|(path, digest)| serde_json::json!({ "path": path, "digest": digest }))
        .collect();
    let out = serde_json::json!({ "class": "repo-map", "files": files });
    Ok(serde_json::to_vec(&out).expect("serialize"))
}

/// Orient-pack: object with sorted keys; string values whitespace-normalized.
pub fn canonicalize_orient_pack(raw: &str) -> Result<Vec<u8>, CanonicalizeError> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| CanonicalizeError::InvalidJson(e.to_string()))?;
    if !v.is_object() {
        return Err(CanonicalizeError::WrongShape(
            "orient-pack needs object".into(),
        ));
    }
    let mut canon = canonicalize_json_value(&v);
    if let serde_json::Value::Object(ref mut m) = canon {
        m.insert("class".into(), serde_json::json!("orient-pack"));
    }
    Ok(serde_json::to_vec(&canon).expect("serialize"))
}

/// Search-results: hits sorted by (path, start, end); query string normalized.
pub fn canonicalize_search_results(raw: &str) -> Result<Vec<u8>, CanonicalizeError> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| CanonicalizeError::InvalidJson(e.to_string()))?;
    let query = v.get("query").and_then(|q| q.as_str()).unwrap_or("").trim();
    let hits = v
        .get("hits")
        .and_then(|h| h.as_array())
        .ok_or_else(|| CanonicalizeError::WrongShape("search-results needs hits[]".into()))?;
    let mut rows: Vec<(String, u64, u64, String)> = Vec::new();
    for h in hits {
        let path = h
            .get("path")
            .and_then(|p| p.as_str())
            .unwrap_or("")
            .to_string();
        let start = h.get("start").and_then(|x| x.as_u64()).unwrap_or(0);
        let end = h.get("end").and_then(|x| x.as_u64()).unwrap_or(start);
        let snippet = h
            .get("snippet")
            .and_then(|s| s.as_str())
            .map(normalize_ws)
            .unwrap_or_default();
        rows.push((path, start, end, snippet));
    }
    rows.sort();
    let hits: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(path, start, end, snippet)| {
            serde_json::json!({
                "path": path,
                "start": start,
                "end": end,
                "snippet": snippet.trim(),
            })
        })
        .collect();
    let out = serde_json::json!({
        "class": "search-results",
        "query": normalize_ws(query).trim(),
        "hits": hits,
    });
    Ok(serde_json::to_vec(&out).expect("serialize"))
}

pub fn canonicalize(class: ArtifactClass, raw: &str) -> Result<Vec<u8>, CanonicalizeError> {
    match class {
        ArtifactClass::RepoMap => canonicalize_repo_map(raw),
        ArtifactClass::OrientPack => canonicalize_orient_pack(raw),
        ArtifactClass::SearchResults => canonicalize_search_results(raw),
    }
}
