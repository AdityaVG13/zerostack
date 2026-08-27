//! Shared plan identity, path safety, text, and storage utilities.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use graphzero_store::ContentHash;
use graphzero_store::store::blob_store::BlobStore;
use graphzero_store::store::query::persist_query_json;

use serde_json::Value;

use super::types::MAX_RESULT_REF_BYTES;
static EXECUTION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn execution_id(plan: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let sequence = EXECUTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    // Contract-pinned form `cm://exec/<unix_millis>-<sha256_hex[..12]>` (G2).
    // Uniqueness under concurrent identical plans comes from hashing plan, pid,
    // sequence, and time identity into the 12-hex suffix.
    let mut input = Vec::with_capacity(plan.len() + 48);
    input.extend_from_slice(plan.as_bytes());
    input.extend_from_slice(b":");
    input.extend_from_slice(std::process::id().to_string().as_bytes());
    input.extend_from_slice(b":");
    input.extend_from_slice(sequence.to_string().as_bytes());
    input.extend_from_slice(b":");
    input.extend_from_slice(millis.to_string().as_bytes());
    let hash = ContentHash::of(&input).to_hex();
    format!("cm://exec/{millis}-{}", &hash[..12])
}

pub(crate) fn safe_execution_path_component(id: &str) -> String {
    // Strip the scheme prefix so refs use the bare `<millis>-<hash>` segment
    // instead of a sanitized `cm___exec_...` component (G2).
    let tail = id.strip_prefix("cm://exec/").unwrap_or(id);
    tail.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn now_rfc3339ish() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("unix_ms:{millis}")
}

// ── text helpers ──

pub(crate) fn first_chars_flat(s: &str, max_chars: usize) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

pub(crate) fn store_query_ref(store_root: &Path, value: &Value) -> Result<String, std::io::Error> {
    let json_text = serde_json::to_string(value).unwrap_or_default();
    store_query_json_ref(store_root, &json_text)
}

pub(crate) fn store_query_json_ref(
    store_root: &Path,
    json_text: &str,
) -> Result<String, std::io::Error> {
    let id = persist_query_json(store_root, json_text)?;
    Ok(format!("gz://query/{id}"))
}

/// Canonicalize a compact `q:<id>` alias into the typed `gz://query/<id>` form.
/// Refs handed to agents must stay typed so hub handoff can route them.
pub(crate) fn canonical_query_ref(reference: &str) -> String {
    reference
        .strip_prefix("q:")
        .map(|id| format!("gz://query/{id}"))
        .unwrap_or_else(|| reference.to_string())
}

pub(crate) fn compact_query_alias(reference: &str) -> String {
    reference
        .strip_prefix("gz://query/")
        .map(|id| format!("q:{id}"))
        .unwrap_or_else(|| reference.to_string())
}

pub(crate) fn store_blob_ref(store_root: &Path, bytes: &[u8]) -> Result<String, anyhow::Error> {
    if bytes.len() > MAX_RESULT_REF_BYTES {
        anyhow::bail!("result ref byte limit exceeded")
    }
    let store = BlobStore::open(store_root)?;
    let hash = store.put(bytes)?;
    Ok(format!("gz://blob/{}", hash.to_hex()))
}
