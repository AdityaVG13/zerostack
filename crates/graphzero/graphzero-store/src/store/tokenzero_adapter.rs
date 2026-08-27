//! Optional TokenZero seen-set + `#B` fragment adapter (ref-contract §8, P4.3 G-006).
//!
//! Composes [`EntityAwareSeenProvider`] over [`LocalSeenProvider`] so TokenZero
//! batch lookups share the same know-this-fact novelty as the default snap path.
//! When `ZEROSTACK_STORE_ROOT` (or peers) is set, mark/is_seen also hydrate and
//! flush the shared `zerostack.entity-novelty` pointer (`.5` fusion).

use std::path::Path;

use super::entity_novelty_fusion::{
    flush_entity_novelty, fusion_store_root_from_env, hydrate_entity_novelty, scope_key_for,
};
use super::expand::apply_fragment;
use super::refs::{Fragment, GzRef};
use super::session::{
    EntityAwareSeenProvider, LocalSeenProvider, SeenKey, SeenProvider, SeenScope, SeenStatus,
    SessionLedger,
};

/// TokenZero recovery cache directory + scope for batch seen lookup.
pub struct TokenZeroSeenAdapter {
    pub warning: Option<String>,
}

impl TokenZeroSeenAdapter {
    pub fn from_env() -> Option<Self> {
        let dir = std::env::var("GRAPHZERO_TOKENZERO_CACHE").ok()?;
        if !Path::new(&dir).is_dir() {
            return None;
        }
        Some(Self { warning: None })
    }

    fn entity_aware() -> EntityAwareSeenProvider<LocalSeenProvider> {
        EntityAwareSeenProvider::local()
    }

    fn hydrate_scope(scope: &SeenScope) {
        let Some(root) = fusion_store_root_from_env() else {
            return;
        };
        let key = scope_key_for(scope);
        let mut novelty = super::entity::EntityNovelty::new();
        if hydrate_entity_novelty(&root, &key, &mut novelty).is_ok() {
            let _ = SessionLedger::merge_shared_entity_ids(scope, novelty.known_ids());
        }
    }

    fn flush_scope(scope: &SeenScope) {
        let Some(root) = fusion_store_root_from_env() else {
            return;
        };
        let key = scope_key_for(scope);
        let mut novelty = super::entity::EntityNovelty::new();
        let _ = novelty.merge_ids(SessionLedger::known_entity_ids(scope));
        let _ = flush_entity_novelty(&root, &key, &novelty);
    }
}

impl SeenProvider for TokenZeroSeenAdapter {
    fn batch_seen(&self, keys: &[SeenKey]) -> Vec<SeenStatus> {
        // Preserve byte SeenKey; novelty via EntityAwareSeenProvider + shared store.
        let provider = Self::entity_aware();
        keys.iter()
            .map(|k| {
                Self::hydrate_scope(&k.scope);
                let dest = if k.end > k.start && !k.blob_sha256.is_empty() {
                    format!("gz://blob/{}#B{}-{}", k.blob_sha256, k.start, k.end)
                } else {
                    String::new()
                };
                let seen = if dest.is_empty() {
                    false
                } else {
                    provider.is_seen(&k.scope, &dest)
                };
                SeenStatus {
                    seen,
                    scope: k.scope.clone(),
                    source: "tokenzero",
                }
            })
            .collect()
    }

    fn is_seen(&self, scope: &SeenScope, destination_ref: &str) -> bool {
        Self::hydrate_scope(scope);
        Self::entity_aware().is_seen(scope, destination_ref)
    }

    fn mark_seen(&self, scope: &SeenScope, destination_ref: &str) {
        Self::entity_aware().mark_seen(scope, destination_ref);
        Self::flush_scope(scope);
    }
}

/// Verify `#B<start>-<end>` bytes match between GraphZero expand input and a blob slice.
pub fn b_fragment_matches(blob: &[u8], start: u64, end: u64) -> bool {
    let frag = Fragment::Bytes { start, end };
    apply_fragment(blob, &frag).is_ok()
}

/// Round-trip: parse `gz://blob/...#B` and slice bytes.
pub fn gz_b_fragment_bytes(reference: &str, blob: &[u8]) -> Result<Vec<u8>, String> {
    let gz = GzRef::parse(reference).map_err(|e| e.to_string())?;
    match &gz {
        GzRef::Blob { fragment, .. } => apply_fragment(blob, fragment),
        _ => Err("not a blob ref".into()),
    }
}

/// When TokenZero is unavailable, batch lookup returns all-unseen + warning.
pub fn batch_seen_with_fallback(
    provider: Option<&TokenZeroSeenAdapter>,
    keys: &[SeenKey],
) -> (Vec<SeenStatus>, Option<String>) {
    match provider {
        Some(p) => (p.batch_seen(keys), p.warning.clone()),
        None => (
            keys.iter()
                .map(|k| SeenStatus {
                    seen: false,
                    scope: k.scope.clone(),
                    source: "unavailable",
                })
                .collect(),
            Some("tokenzero adapter not configured".to_string()),
        ),
    }
}
