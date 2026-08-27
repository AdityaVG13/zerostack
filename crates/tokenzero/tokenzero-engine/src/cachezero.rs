//! Shadow ActionCache observation: classify and journal, never serve.

use crate::TokenZeroEngine;
use crate::action_cache_key::{ActionCacheKeyInput, ConsistencyClass, action_cache_key};
use serde_json::Value;
use tokenzero_core::{ToolResponse, sha256_hex};
use tokenzero_recovery::{
    ActionCacheEntry, ActionCacheIndex, CacheStatus, CachezeroMode, ShadowDecision,
    classify_would_be_status, record_shadow_decision, store_root_from_cache_path,
};

pub fn observe_action_cache(
    engine: &TokenZeroEngine,
    op: &str,
    args: &Value,
    wall_ns: u64,
    response: &mut ToolResponse,
) {
    observe_with_mode(
        engine,
        op,
        args,
        wall_ns,
        response,
        CachezeroMode::from_env(),
    );
}

pub fn observe_with_mode(
    engine: &TokenZeroEngine,
    op: &str,
    args: &Value,
    wall_ns: u64,
    response: &mut ToolResponse,
    mode: CachezeroMode,
) {
    if !mode.is_shadow() {
        return;
    }
    let store_root = store_root_from_cache_path(&engine.config.cache_path);
    let consistency = ConsistencyClass::parse(
        args.get("consistency_class")
            .and_then(Value::as_str)
            .or_else(|| args.get("consistency").and_then(Value::as_str)),
    );
    let model_id = args.get("model_id").and_then(Value::as_str);
    let key = action_cache_key(ActionCacheKeyInput {
        op,
        args,
        store_root: &store_root,
        model_id,
        consistency_class: Some(consistency),
    });
    let index = ActionCacheIndex::open(&store_root);
    // Tenancy scope (ZS-CACHE-015): every write-through is attributed to the
    // engine's session world, and resolution is world-filtered so an entry
    // written under one world never resolves under another.
    let world_id = Some(engine.session_id.clone());
    let entry = index.resolve(&key, world_id.as_deref()).ok().flatten();
    let result_digest = result_digest(response);
    let in_flight = index.has_in_flight_serve(&key);
    // Without an FSZero journal we cannot prove a bookmark is still live.
    // A present bookmark is treated as intersect so we never invent causal-hit.
    let blast_intersect = entry
        .as_ref()
        .is_some_and(|item| item.fszero_bookmark.is_some());
    // L2-valid / L3-cold entries must refetch before use: never claim a
    // would-have-hit (nor full savings) for a blob that is no longer resident
    // (ZS-CACHE-013). The write-through below restores L3 on identical bytes.
    let entry_for_classification = entry.as_ref().filter(|item| !item.l3_cold);
    let status = classify_would_be_status(
        entry_for_classification,
        &result_digest,
        in_flight,
        blast_intersect,
    );
    let result_tokens = response
        .accounting
        .as_ref()
        .map(|acct| acct.visible_tokens as u64)
        .unwrap_or(0);
    let saved = if status.would_have_hit() {
        result_tokens
    } else {
        0
    };
    response.cache_status = Some(status.as_str().to_string());
    response.saved_tokens_estimate = Some(saved);

    let bookmark = entry.as_ref().and_then(|item| item.fszero_bookmark.clone());
    let decision = ShadowDecision {
        key: key.clone(),
        bookmark,
        blast_intersect,
        result_digest: result_digest.clone(),
        result_tokens,
        wall_ms: wall_ns / 1_000_000,
        would_be_status: status,
        artifact_class: artifact_class(op).to_string(),
        saved_tokens_estimate: saved,
    };
    let _ = record_shadow_decision(&store_root, &decision);
    // Write-through only. The just-computed body is what the caller already has.
    let _ = index.put(ActionCacheEntry {
        key,
        artifact_ref: format!("tz://blob/{result_digest}"),
        fszero_bookmark: None,
        dep_closure_ref: None,
        class: consistency.as_str().to_string(),
        verified: response.status == "ok",
        world_id: world_id.clone(),
        tombstone: false,
        tombstoned_at_unix: None,
        l3_cold: false,
        cold_since_unix: None,
    });
}

fn result_digest(response: &ToolResponse) -> String {
    for record in &response.refs {
        if record.kind != "blob" {
            continue;
        }
        if let Some(hash) = record.ref_id.rsplit('/').next().filter(|part| {
            part.len() == 64
                && part
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        }) {
            return hash.to_string();
        }
    }
    let text = response
        .visible
        .as_ref()
        .map(|visible| visible.text.as_str())
        .unwrap_or("");
    sha256_hex(text)
}

fn artifact_class(op: &str) -> &str {
    op.strip_prefix("tz_")
        .or_else(|| op.strip_prefix("zero.token."))
        .or_else(|| op.strip_prefix("zero."))
        .unwrap_or(op)
}

