//! Durable path-keyed memory in the recovery store.
//! Paths use `mem://`, separate from `z://blob` refs and `world_*` keys.

use super::recovery::RecoveryStore;
use std::path::{Component, Path};

const MEM_PREFIX: &str = "mem://";

#[inline]
fn mem_miss(key: impl std::fmt::Display) -> String {
    format!("memory miss: {key}")
}

#[inline]
fn empty_mem_path() -> Result<String, String> {
    Err("empty memory path".to_string())
}

/// Normalize a caller path into a stable `mem://...` store key.
pub fn mem_key(path: &str) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return empty_mem_path();
    }
    let without = trimmed
        .strip_prefix("mem://")
        .or_else(|| trimmed.strip_prefix("mem:/"))
        .or_else(|| trimmed.strip_prefix("mem:"))
        .unwrap_or(trimmed);
    let path = Path::new(without);
    let mut parts = Vec::new();
    for c in path.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => return Err("memory path must not contain ..".to_string()),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    if parts.is_empty() {
        return empty_mem_path();
    }
    Ok(format!("{MEM_PREFIX}{}", parts.join("/")))
}

fn rel_path(key: &str) -> String {
    key.trim_start_matches(MEM_PREFIX).to_string()
}

pub fn put_memory(store: &mut RecoveryStore, path: &str, data: &[u8]) -> Result<String, String> {
    let key = mem_key(path)?;
    //.8: payload + content ref + memory_paths index must commit as one
    // unit so a crash mid-put cannot leave orphan index/payload rows.
    let began = store.begin_exec_txn();
    let result = (|| {
        store.try_put_key(&key, data)?;
        let content_ref = store.try_put_content_ref(data)?;
        store.upsert_memory_path(&rel_path(&key), &key, &content_ref)?;
        Ok(content_ref)
    })();
    match result {
        Ok(content_ref) => {
            store.commit_exec_txn(began);
            Ok(content_ref)
        }
        Err(e) => {
            store.rollback_exec_txn(began);
            Err(e)
        }
    }
}

pub fn get_memory(store: &RecoveryStore, path: &str) -> Result<Vec<u8>, String> {
    let key = mem_key(path)?;
    store.get_payload(&key).ok_or_else(|| mem_miss(key))
}

fn remove_memory_key(store: &mut RecoveryStore, key: &str) -> Result<(), String> {
    let began = store.begin_exec_txn();
    let result = (|| {
        store.try_delete_key(key)?;
        store.delete_memory_path(&rel_path(key))?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            store.commit_exec_txn(began);
            Ok(())
        }
        Err(e) => {
            store.rollback_exec_txn(began);
            Err(e)
        }
    }
}

pub fn delete_memory(store: &mut RecoveryStore, path: &str) -> Result<(), String> {
    let key = mem_key(path)?;
    let rel = rel_path(&key);
    if !store.has_payload(&key) && !store.memory_path_exists(&rel) {
        return Err(mem_miss(key));
    }
    remove_memory_key(store, &key)
}

pub fn rename_memory(store: &mut RecoveryStore, from: &str, to: &str) -> Result<String, String> {
    let from_key = mem_key(from)?;
    let to_key = mem_key(to)?;
    if from_key == to_key {
        return Ok(rel_path(&to_key));
    }
    let bytes = store
        .get_payload(&from_key)
        .ok_or_else(|| mem_miss(&from_key))?;
    if store.has_payload(&to_key) {
        return Err(format!("memory exists: {to_key}"));
    }
    put_memory(store, to, &bytes)?;
    remove_memory_key(store, &from_key)?;
    Ok(rel_path(&to_key))
}

pub fn list_memory(store: &RecoveryStore, prefix: &str) -> Vec<String> {
    let root_only = prefix.trim().is_empty()
        || prefix.trim() == "/"
        || matches!(prefix.trim(), "mem:" | "mem:/" | "mem://");
    if root_only {
        return store.list_memory_paths("");
    }
    match mem_key(prefix) {
        Ok(k) => {
            let rel = rel_path(&k);
            store.list_memory_paths(&rel)
        }
        Err(_) => Vec::new(),
    }
}

/// Encode a path field for `|`-delimited memory wire specs (`put:path|content`,
/// `rename:from|to`) so literal `|` (and `%`) in paths round-trip.
pub fn encode_wire_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            '%' => out.push_str("%25"),
            '|' => out.push_str("%7C"),
            _ => out.push(c),
        }
    }
    out
}

/// Inverse of [`encode_wire_path`]. Unknown `%XX` sequences are left intact.
pub fn decode_wire_path(encoded: &str) -> String {
    let mut out = String::with_capacity(encoded.len());
    let mut rest = encoded;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        if after.len() >= 3 {
            let code = &after[1..3];
            if code.eq_ignore_ascii_case("7C") {
                out.push('|');
                rest = &after[3..];
                continue;
            }
            if code.eq_ignore_ascii_case("25") {
                out.push('%');
                rest = &after[3..];
                continue;
            }
        }
        out.push('%');
        rest = &after[1..];
    }
    out.push_str(rest);
    out
}

pub fn memory_put_wire(path: &str, content: &str) -> String {
    format!("put:{}|{}", encode_wire_path(path), content)
}

pub fn memory_rename_wire(from: &str, to: &str) -> String {
    format!("rename:{}|{}", encode_wire_path(from), encode_wire_path(to))
}
