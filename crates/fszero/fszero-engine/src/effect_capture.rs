//! V6-F1 (ZS-STORE-004): uniform effect capture across every mutating op.
//!
//! Each mutating op (fs.write, fs.edit, fs.undo, fs.transact, verifiedEdit,
//! world commit) seals ONE structured effect record into the recovery store
//! and binds it into the op receipt (`effects=<content-ref>` token in the op
//! detail; the ref expands to the record's JSON). The record reuses the
//! mutation-journal vocabulary (op names, repo-relative paths, pre/post
//! content refs, seq ordinals, session window, agent) so it cross-checks
//! against fs.history / fs.undo, and it is deterministic (paths sorted by
//! path). Root-guard escape attempts that fail loud are receipted too:
//! `paths` is empty and `refused` lists every attempted path — nothing
//! outside the root is ever written, and the attempt is never silent.

use serde::{Deserialize, Serialize};

use crate::session::FSZeroSession;

/// One path's effect in a mutating op.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectPath {
    /// Repo-relative path (mutation-journal `path` vocabulary).
    pub path: String,
    /// write = overwrote existing content; create = file did not exist
    /// before the op; delete = file removed by the op.
    pub action: EffectAction,
    /// Mutation-journal seq of the row this path's mutation produced
    /// (cross-checkable against fs.history). Parent-directory creations
    /// made by fs.write share the file row's seq (no journal rows exist
    /// for directories).
    pub seq: i64,
    /// Content ref of the pre-image ("" when the file did not exist).
    pub pre_ref: String,
    /// Content ref of the post-image ("" when the file was deleted).
    pub post_ref: String,
}

/// Effect classification per path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectAction {
    Write,
    Create,
    Delete,
}

/// Where an op's effects landed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectScope {
    /// Direct session-workspace mutation (write/edit/undo/transact/verifiedEdit).
    Session,
    /// Mutation published by a world commit.
    World { wid: String },
}

/// One mutating op's complete uniform effect record (V6-F1 / ZS-STORE-004).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectRecord {
    /// Mutation-journal op vocabulary: write | edit | undo | verifiedEdit |
    /// world | transact.
    pub op: String,
    pub scope: EffectScope,
    /// UNIX epoch seconds (journal ts vocabulary).
    pub ts: u64,
    /// FSZERO_AGENT_ID (journal agent vocabulary; "" when unset).
    pub agent: String,
    /// Session access-window ordinal (journal window vocabulary).
    pub window: i64,
    /// Lexical session root ("" when the session had no root).
    pub root: String,
    /// Deterministic: sorted by `path`.
    pub paths: Vec<EffectPath>,
    /// Root-guard refusals observed by the op, in attempt order. A refusal
    /// record has an empty `paths`: nothing was written.
    pub refused: Vec<String>,
}

impl FSZeroSession {
    /// Seal one op's effect record and return the `effects=<ref>` receipt
    /// token to bind into the op receipt. Degraded (non-durable) sessions
    /// return an empty token — the op detail then carries no effects claim
    /// (same hole policy as the mutation journal: never claim a hole).
    pub fn seal_effect_record(
        &mut self,
        op: &str,
        scope: EffectScope,
        mut paths: Vec<EffectPath>,
        refused: Vec<String>,
    ) -> String {
        if self.durable_degraded {
            return String::new();
        }
        paths.sort_by(|a, b| a.path.cmp(&b.path));
        let record = EffectRecord {
            op: op.to_string(),
            scope,
            ts: crate::recovery::unix_epoch_secs().max(0) as u64,
            agent: std::env::var("FSZERO_AGENT_ID").unwrap_or_default(),
            window: self.access_session_window,
            root: self
                .root
                .as_ref()
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_default(),
            paths,
            refused,
        };
        let payload = serde_json::to_vec(&record).unwrap_or_default();
        // Content-addressed named key: deterministic per distinct record
        // (re-sealing an identical record is idempotent) and expandable by
        // name or by the returned content ref.
        let digest = crate::access_log::content_hash_bytes(&payload);
        let key = format!("effect/{op}/{}", &digest[..16.min(digest.len())]);
        let content_ref = self.recovery.put_named_payload(&key, &payload);
        format!("effects={content_ref}")
    }

    /// Append an `effects=<ref>` token to an op detail string (no-op when
    /// the seal produced nothing, e.g. degraded sessions).
    pub(crate) fn append_effect_token(detail: &mut String, effects: &str) {
        if !effects.is_empty() {
            detail.push(' ');
            detail.push_str(effects);
        }
    }
}
