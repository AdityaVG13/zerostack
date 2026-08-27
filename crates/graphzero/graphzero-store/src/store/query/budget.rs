//! Ref-first budget policy: spill overflow to gz://query/<id> refs (ref-contract §7).

use std::path::Path;

use crate::ContentHash;

use super::super::expand::json_escape;
use super::types::CapsuleMatch;

pub fn tokens_for_utf8(bytes: &[u8]) -> usize {
    bytes.len().div_ceil(4)
}

pub fn tokens_for_str(s: &str) -> usize {
    tokens_for_utf8(s.as_bytes())
}

pub fn persist_query_json(store_root: &Path, full: &str) -> Result<String, std::io::Error> {
    persist_query_json_inner(store_root, full).map(|(id, _)| id)
}

fn persist_query_json_inner(
    store_root: &Path,
    full: &str,
) -> Result<(String, bool), std::io::Error> {
    let bytes = full.as_bytes();
    let hash = ContentHash::of(bytes);
    let hash_hex = hash.to_hex();
    let id = hash_hex[..16].to_string();
    let dir = store_root.join("queries");
    std::fs::create_dir_all(&dir)?;
    let json_path = dir.join(format!("{id}.json"));
    let digest_path = dir.join(format!("{id}.sha256"));
    let query_artifacts_match =
        file_bytes_match(&json_path, bytes) && file_bytes_match(&digest_path, hash_hex.as_bytes());
    if !query_artifacts_match {
        super::super::atomic_write_file(&json_path, bytes)?;
        // Full-hash sidecar so expand can resolve via BlobStore/SharedCas after the
        // legacy queries/<id>.json spill is removed (graphzero-m3wx). Query ids are
        // only a 16-hex prefix; SharedCas requires the exact 64-hex identity.
        super::super::atomic_write_file(&digest_path, hash_hex.as_bytes())?;
    }

    // A complete same-content spill is immutable and needs no new fsync or CAS
    // publication. Verify every replica before taking that fast path so corrupt
    // content-addressed bytes still fail loudly.
    let store = crate::BlobStore::open(store_root).map_err(io_other)?;
    let legacy_matches = store.get(&hash).map_err(io_other)?.as_deref() == Some(bytes);
    let cas = crate::SharedCas::open_labeled(store_root, "cas-local");
    let shared_matches = match cas.get_verified(&hash_hex) {
        Ok(existing) => existing == bytes,
        Err(crate::ExternalResolveError::NotFound) => false,
        Err(error) => return Err(io_other(error)),
    };
    let replicas_match = legacy_matches && shared_matches;
    if !replicas_match {
        store.put(bytes).map_err(io_other)?;
    }

    let mut refs_match = true;
    for ref_id in [format!("gz://query/{id}"), format!("q:{id}")] {
        if !ref_points_to_store(&ref_id, store_root) {
            refs_match = false;
            super::super::ref_index::record_ref(&ref_id, store_root).map_err(io_other)?;
        }
    }

    // Opt-in capsule spill provenance (graphzero-3wbh.2).
    if super::super::provenance::provenance_enabled() {
        let _ = super::super::provenance::attach_capsule_build_provenance(
            store_root,
            &hash_hex,
            &id,
            bytes.len() as u32,
            Some(bytes),
        );
    }
    Ok((id, query_artifacts_match && replicas_match && refs_match))
}

fn file_bytes_match(path: &Path, expected: &[u8]) -> bool {
    std::fs::read(path)
        .map(|actual| actual == expected)
        .unwrap_or(false)
}

fn ref_points_to_store(ref_id: &str, store_root: &Path) -> bool {
    let Some(indexed) = super::super::ref_index::lookup_store(ref_id) else {
        return false;
    };
    match (indexed.canonicalize(), store_root.canonicalize()) {
        (Ok(indexed), Ok(expected)) => indexed == expected,
        _ => false,
    }
}

fn io_other(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

pub fn spill_id_for_json(store_root: Option<&Path>, full: &str) -> String {
    store_root
        .and_then(|root| persist_query_json(root, full).ok())
        .unwrap_or_else(|| {
            let h = ContentHash::of(full.as_bytes()).to_hex();
            h[..16].to_string()
        })
}

fn estimate_match_len(m: &CapsuleMatch) -> usize {
    m.defs.len() * 120 + m.edges.len() * 140 + 80 + m.name.len()
}

fn match_rank(m: &CapsuleMatch) -> usize {
    m.edges.len() * 10 + m.defs.len() * 5 + m.name.len()
}

pub fn knapsack_matches(
    matches: &[CapsuleMatch],
    budget_bytes: usize,
) -> (Vec<CapsuleMatch>, usize) {
    if matches.is_empty() {
        return (Vec::new(), 0);
    }
    let mut order: Vec<usize> = (0..matches.len()).collect();
    order.sort_by(|&a, &b| match_rank(&matches[b]).cmp(&match_rank(&matches[a])));
    let fallback_idx = order[0];
    let mut kept = Vec::new();
    let mut shown = 0usize;
    for idx in order {
        let m = &matches[idx];
        let est = estimate_match_len(m);
        if shown + est > budget_bytes && !kept.is_empty() {
            continue;
        }
        shown = shown.saturating_add(est);
        kept.push(m.clone());
    }
    if kept.is_empty() {
        kept.push(matches[fallback_idx].clone());
    }
    kept.sort_by(|a, b| a.name.cmp(&b.name));
    let omitted = matches.len().saturating_sub(kept.len());
    (kept, omitted)
}

pub fn append_accounting(
    json: &mut String,
    visible_tokens: usize,
    full_tokens: usize,
    expand_verified: bool,
) {
    let savings = if expand_verified {
        full_tokens.saturating_sub(visible_tokens)
    } else {
        0
    };
    json.truncate(json.len().saturating_sub(1));
    json.push_str(&format!(
        ",\"accounting\":{{\"visible_tokens\":{visible_tokens},\"full_tokens\":{full_tokens},\"savings_tokens\":{savings},\"expand_verified\":{expand_verified}}}}}"
    ));
}

pub fn compact_truncated_budgeted(
    query: &str,
    snapshot_id: u64,
    budget: usize,
    id: &str,
    full_tokens: usize,
) -> String {
    format!(
        "{{\"query\":\"{}\",\"snapshot\":{snapshot_id},\"budget\":{budget},\"truncated\":{{\"full_ref\":\"gz://query/{id}\"}},\"accounting\":{{\"visible_tokens\":0,\"full_tokens\":{full_tokens},\"savings_tokens\":0,\"expand_verified\":false}}}}",
        json_escape(query)
    )
}

pub fn enforce_visible_byte_cap(
    out: &mut String,
    budget_bytes: usize,
    full_tokens: usize,
    query: &str,
    snapshot_id: u64,
    budget: usize,
    id: &str,
) {
    append_accounting(out, tokens_for_str(out), full_tokens, false);
    if out.len() <= budget_bytes {
        return;
    }
    *out = compact_truncated_budgeted(query, snapshot_id, budget, id, full_tokens);
}

/// Post-expand savings per ref-contract §7.
pub fn savings_tokens_after_expand(visible_json: &str, full_bytes: &[u8]) -> usize {
    let visible = tokens_for_str(visible_json);
    let full = tokens_for_utf8(full_bytes);
    full.saturating_sub(visible)
}
