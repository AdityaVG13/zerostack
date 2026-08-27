//! vz89.10 session exposure ledger (mirror of hub
//! zerostack-racc-caching-output-vz89.10): track which evidence objects have
//! already crossed into the model for a session scope, so a second reference
//! sends the short ref instead of re-inlining bytes.
//!
//! Scope identity matches session_persist (TOKENZERO_SESSION_SCOPE or the
//! cache-path-derived scope), so the per-call engines CodeMode builds share
//! one ledger inside a server process. The ledger is deliberately
//! memory-resident: losing it (process restart) only causes a re-inline,
//! never wrong bytes, and re-expansion is always available and accounted as
//! recovery.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

/// One exposed evidence object: (session scope, object digest/ref, span).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExposureRow {
    /// Content-addressed ref of the exposed object (digest-bearing).
    pub object_ref: String,
    /// Span within the object; None = whole object.
    pub span: Option<String>,
    /// Session turn (codemode execution index) of first exposure.
    pub first_exposure_turn: u64,
    pub byte_len: u64,
    /// Expands after first exposure; each is accounted as recovery.
    pub reexpansions: u64,
}

/// Declared dynamic-envelope segments of a provider-visible history:
/// 0-based message indexes (e.g. per-request headers or system tail) that
/// are allowed to differ between successive histories. Everything outside
/// this declaration is append-only frozen content.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DynamicEnvelopeExclusion {
    pub message_indexes: Vec<usize>,
}

impl DynamicEnvelopeExclusion {
    /// Strictest posture: no segment may differ between histories.
    pub fn none() -> Self {
        Self {
            message_indexes: Vec::new(),
        }
    }

    /// Declare the given 0-based message indexes as dynamic envelope
    /// segments (allowed to differ).
    pub fn message_indexes(message_indexes: Vec<usize>) -> Self {
        Self { message_indexes }
    }

    fn contains(&self, index: usize) -> bool {
        self.message_indexes.contains(&index)
    }
}

/// Fail-loud violation: a successive provider-visible history rewrote
/// earlier content outside the declared dynamic-envelope exclusion
/// (ZS-VIEW-005). Providers cache against the earlier history; rewriting
/// earlier messages would silently invalidate prefix caching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderHistoryRewrite {
    /// The successive history is shorter than the earlier one: earlier
    /// content was dropped instead of extended.
    Truncated {
        previous_len: usize,
        next_len: usize,
    },
    /// A non-excluded earlier message changed at `index`; `lcp` is the
    /// message-sequence LCP with the previous history.
    RewroteMessage {
        index: usize,
        lcp: usize,
        previous: String,
        rewritten: String,
    },
}

/// Append-only provider-visible message-history policy (ZS-VIEW-005;
/// transcript-policy surface is hub ZS-CONTRACT-003).
///
/// Successive histories sent to the provider must extend the previous one:
/// the longest common prefix of two consecutive histories is the earlier
/// history itself (every earlier message unchanged, in order, and nothing
/// dropped). Only segments declared dynamic via [DynamicEnvelopeExclusion]
/// (e.g. per-request headers, system tail) may differ.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppendOnlyHistoryPolicy {
    dynamic_envelope: DynamicEnvelopeExclusion,
}

impl AppendOnlyHistoryPolicy {
    /// Build a policy with the given dynamic-envelope declaration.
    pub fn new(dynamic_envelope: DynamicEnvelopeExclusion) -> Self {
        Self { dynamic_envelope }
    }

    /// Enforce append-only extension of `previous` by `next`. Returns the
    /// message-sequence LCP length on success; returns a fail-loud
    /// [ProviderHistoryRewrite] when `next` drops or rewrites earlier
    /// content outside the declared dynamic-envelope exclusion.
    pub fn check(
        &self,
        previous: &[String],
        next: &[String],
    ) -> Result<usize, ProviderHistoryRewrite> {
        if next.len() < previous.len() {
            return Err(ProviderHistoryRewrite::Truncated {
                previous_len: previous.len(),
                next_len: next.len(),
            });
        }
        let lcp = crate::engine_common::common_prefix_len(previous, next);
        for index in lcp..previous.len() {
            if !self.dynamic_envelope.contains(index) && previous[index] != next[index] {
                return Err(ProviderHistoryRewrite::RewroteMessage {
                    index,
                    lcp,
                    previous: previous[index].clone(),
                    rewritten: next[index].clone(),
                });
            }
        }
        Ok(lcp)
    }
}

#[derive(Debug, Default)]
pub struct SessionExposureLedger {
    rows: HashMap<(String, Option<String>), ExposureRow>,
    turn: u64,
    /// Last provider-visible message history accepted for this session.
    provider_history: Vec<String>,
    /// Append-only policy over successive provider-visible histories.
    history_policy: AppendOnlyHistoryPolicy,
}

impl SessionExposureLedger {
    /// Advance the session turn; called once per codemode execution.
    pub fn next_turn(&mut self) -> u64 {
        self.turn = self.turn.saturating_add(1);
        self.turn
    }

    /// The recorded exposure for (object_ref, span), if the session already
    /// holds those bytes.
    pub fn exposure(&self, object_ref: &str, span: Option<&str>) -> Option<&ExposureRow> {
        self.rows
            .get(&(object_ref.to_string(), span.map(str::to_string)))
    }

    /// Record first exposure of byte_len bytes. Returns true when newly
    /// recorded, false when the session already held the object.
    pub fn record(&mut self, object_ref: &str, span: Option<String>, byte_len: u64) -> bool {
        let key = (object_ref.to_string(), span.clone());
        if self.rows.contains_key(&key) {
            return false;
        }
        self.rows.insert(
            key,
            ExposureRow {
                object_ref: object_ref.to_string(),
                span,
                first_exposure_turn: self.turn,
                byte_len,
                reexpansions: 0,
            },
        );
        true
    }

    /// Record a re-expansion of a session-known object; returns the running
    /// re-expansion count, or None when the object was never exposed (an
    /// expand of foreign bytes is ordinary recovery, not a session replay).
    pub fn record_reexpansion(&mut self, object_ref: &str, span: Option<&str>) -> Option<u64> {
        let key = (object_ref.to_string(), span.map(str::to_string));
        let row = self.rows.get_mut(&key)?;
        row.reexpansions = row.reexpansions.saturating_add(1);
        Some(row.reexpansions)
    }

    /// Declare the append-only policy for provider-visible histories (e.g.
    /// to allow dynamic envelope segments) before recording histories.
    pub fn set_history_policy(&mut self, policy: AppendOnlyHistoryPolicy) {
        self.history_policy = policy;
    }

    /// Record the provider-visible message history for this turn, enforcing
    /// append-only extension of the previously recorded history (ZS-VIEW-005).
    /// On success returns the message-sequence LCP with the previous history
    /// and stores `history`; on violation the ledger keeps the previous
    /// history unchanged (fail loud, no partial state).
    pub fn record_provider_history(
        &mut self,
        history: Vec<String>,
    ) -> Result<usize, ProviderHistoryRewrite> {
        let lcp = self
            .history_policy
            .check(&self.provider_history, &history)?;
        self.provider_history = history;
        Ok(lcp)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

static REGISTRY: LazyLock<Mutex<HashMap<String, Arc<Mutex<SessionExposureLedger>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The process-wide ledger for a session scope. Engines built per call under
/// the same scope (CodeMode) share it; different scopes are isolated.
pub fn session_exposure_ledger(scope_id: &str) -> Arc<Mutex<SessionExposureLedger>> {
    let mut registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(
        registry
            .entry(scope_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(SessionExposureLedger::default()))),
    )
}

