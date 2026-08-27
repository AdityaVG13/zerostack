//! JSON / budget rendering for blast capsules, including budget=1 `next` hints.

use super::super::types::{BlastError, BlastRadiusCapsule};

pub fn blast_to_json_budget(
    capsule: &BlastRadiusCapsule,
    budget: usize,
    store_root: Option<&std::path::Path>,
) -> Result<String, BlastError> {
    let full = blast_to_json(capsule)?;
    if budget > 1 {
        return Ok(full);
    }
    let id = store_root
        .and_then(|root| graphzero_store::store::query::persist_query_json(root, &full).ok())
        .unwrap_or_else(|| {
            use graphzero_store::ContentHash;
            let h = ContentHash::of(full.as_bytes()).to_hex();
            h[..16].to_string()
        });
    Ok(graphzero_store::store::query::query_shell(&id))
}

/// Domain `Value` for blast without string→parse when budget > 1.
///
/// Budget=1 success envelopes attach additive `next` expand/capsule/export hints.
/// The spilled capsule bytes (expand exact payload) are unchanged.
pub fn blast_to_value_budget(
    capsule: &BlastRadiusCapsule,
    budget: usize,
    store_root: Option<&std::path::Path>,
) -> Result<serde_json::Value, BlastError> {
    crate::deterministic_facts::debug_assert_deterministic_facts("blast_radius", capsule);
    if budget > 1 {
        return page_blast_capsule(capsule, budget, store_root);
    }
    let full = serde_json::to_string(capsule)
        .map_err(|error| BlastError::Serialization(error.to_string()))?;
    let id = store_root
        .and_then(|root| graphzero_store::store::query::persist_query_json(root, &full).ok())
        .unwrap_or_else(|| {
            use graphzero_store::ContentHash;
            let h = ContentHash::of(full.as_bytes()).to_hex();
            h[..16].to_string()
        });
    let shell = graphzero_store::store::query::query_shell(&id);
    // Expand the spilled blast capsule (`q:<id>` / raw), not target_ref evidence.
    let next = vec![
        format!("graphzero expand {shell}"),
        "graphzero blast ... --format capsule".to_string(),
        "graphzero blast ... --export PATH".to_string(),
    ];
    Ok(wrap_blast_budget_one_shell(&shell, next))
}

fn page_blast_capsule(
    capsule: &BlastRadiusCapsule,
    budget: usize,
    store_root: Option<&std::path::Path>,
) -> Result<serde_json::Value, BlastError> {
    let cap = budget.min(32);
    if capsule.break_sites.len() <= cap {
        return serde_json::to_value(capsule)
            .map_err(|error| BlastError::Serialization(error.to_string()));
    }
    let mut page = capsule.clone();
    let rest = page.break_sites.split_off(cap);
    page.next_cursor = None;
    let mut tail = capsule.clone();
    tail.break_sites = rest;
    tail.next_cursor = None;
    let doc = crate::query_surface::page_document(
        "blast",
        serde_json::to_value(&tail).unwrap_or(serde_json::Value::Null),
    );
    if let Some(cursor) = crate::query_surface::spill_page(store_root, &doc) {
        crate::query_surface::remember_session_cursor(None, &cursor);
        page.next_cursor = Some(cursor);
    }
    serde_json::to_value(&page).map_err(|error| BlastError::Serialization(error.to_string()))
}

pub fn resume_blast_cursor(
    store_root: &std::path::Path,
    cursor: &str,
    budget: usize,
) -> Option<serde_json::Value> {
    let page = crate::query_surface::load_page(store_root, cursor)?;
    let payload = crate::query_surface::payload_if_kind(&page, "blast")?;
    let capsule: BlastRadiusCapsule = serde_json::from_value(payload).ok()?;
    page_blast_capsule(&capsule, budget, Some(store_root)).ok()
}

fn wrap_blast_budget_one_shell(shell: &str, next: Vec<String>) -> serde_json::Value {
    match serde_json::from_str::<serde_json::Value>(shell) {
        Ok(serde_json::Value::Object(mut map)) => {
            if !next.is_empty() {
                map.insert("next".into(), serde_json::json!(next));
            }
            serde_json::Value::Object(map)
        }
        Ok(other) => other,
        Err(_) => {
            if next.is_empty() {
                serde_json::json!({ "raw": shell })
            } else {
                serde_json::json!({ "raw": shell, "next": next })
            }
        }
    }
}

pub fn blast_to_json(capsule: &BlastRadiusCapsule) -> Result<String, BlastError> {
    // RACC caching contract: blast capsules are cached fact payloads.
    crate::deterministic_facts::debug_assert_deterministic_facts("blast_radius", capsule);
    serde_json::to_string(capsule).map_err(|error| BlastError::Serialization(error.to_string()))
}

pub fn blast_from_json(s: &str) -> Result<BlastRadiusCapsule, serde_json::Error> {
    serde_json::from_str(s)
}
