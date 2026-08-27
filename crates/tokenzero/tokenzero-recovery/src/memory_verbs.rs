//! RACC actions-v2 memory verbs as the TokenZero working-set policy surface.
//!
//! The substrate stays deterministic (`WorkingSet` admit/evict/touch). These
//! verbs are the named interface a hub policy may drive. Policy does not live
//! here. `describe_memory_verb` names a target without mutating it;
//! `apply_memory_verb` runs the mapped primitive and reports `applied: true`
//! only after a visible mutation.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::working_set::{SpanAnchor, WorkingSet, WorkingSetResponse};
use crate::{RecoveryError, RecoveryStore};
use zero_store::{FileIdentity, SessionWal, SessionWalConfig};

/// Six RACC actions-v2 memory-management verbs (tokenzero-fmeo).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryVerb {
    Store,
    CommitSession,
    UpdateCapsule,
    ForgetVisible,
    PromoteAnchor,
    LinkRefs,
}

impl MemoryVerb {
    pub const ALL: [Self; 6] = [
        Self::Store,
        Self::CommitSession,
        Self::UpdateCapsule,
        Self::ForgetVisible,
        Self::PromoteAnchor,
        Self::LinkRefs,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Store => "store",
            Self::CommitSession => "commit_session",
            Self::UpdateCapsule => "update_capsule",
            Self::ForgetVisible => "forget_visible",
            Self::PromoteAnchor => "promote_anchor",
            Self::LinkRefs => "link_refs",
        }
    }

    /// Existing TokenZero working-set / recovery primitive this verb names.
    /// Policy does not live here.
    pub const fn substrate_target(self) -> &'static str {
        match self {
            Self::Store => "working_set.admit",
            Self::CommitSession => "recovery_store.persist",
            Self::UpdateCapsule => "working_set.rewrite_render",
            Self::ForgetVisible => "working_set.evict",
            Self::PromoteAnchor => "working_set.touch",
            Self::LinkRefs => "working_set.evicted_refs",
        }
    }

    /// Parse a product verb name. Unknown names fail loud.
    pub fn from_name(name: &str) -> Result<Self, MemoryVerbError> {
        match name {
            "store" => Ok(Self::Store),
            "commit_session" => Ok(Self::CommitSession),
            "update_capsule" => Ok(Self::UpdateCapsule),
            "forget_visible" => Ok(Self::ForgetVisible),
            "promote_anchor" => Ok(Self::PromoteAnchor),
            "link_refs" => Ok(Self::LinkRefs),
            other => Err(MemoryVerbError::UnknownVerb(other.to_string())),
        }
    }
}

impl FromStr for MemoryVerb {
    type Err = MemoryVerbError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::from_name(name)
    }
}

/// Policy-facing request. Apply fails loud when a verb's required fields are missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryVerbRequest {
    pub verb: MemoryVerb,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ref_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Effect of a describe or apply. `applied` is true only after a visible mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryVerbEffect {
    pub verb: MemoryVerb,
    pub substrate: String,
    pub applied: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum MemoryVerbError {
    #[error("unknown memory verb: {0}")]
    UnknownVerb(String),
    #[error("memory verb {verb} did not mutate: {reason}")]
    NotApplied { verb: &'static str, reason: String },
    #[error(transparent)]
    Recovery(#[from] RecoveryError),
}

/// Map a request onto the deterministic substrate. Does not mutate state.
pub fn describe_memory_verb(request: &MemoryVerbRequest) -> MemoryVerbEffect {
    MemoryVerbEffect {
        verb: request.verb,
        substrate: request.verb.substrate_target().to_string(),
        applied: false,
    }
}

/// Run the mapped substrate primitive. `applied` is true only after a visible
/// mutation. Unknown names must go through `MemoryVerb::from_name` (fail loud).
/// Missing fields, missing spans, and no-op primitives return `Err`, never
/// `Ok` with `applied: false`.
pub fn apply_memory_verb(
    working_set: &mut WorkingSet,
    store: &mut RecoveryStore,
    request: &MemoryVerbRequest,
) -> Result<MemoryVerbEffect, MemoryVerbError> {
    match request.verb {
        MemoryVerb::Store => {
            let (text, anchor) = payload_anchor(request)?;
            require_visible_admit(request.verb, working_set.admit(store, text, anchor)?)?;
        }
        MemoryVerb::CommitSession => {
            let path = store.persistence_path.clone().ok_or_else(|| {
                not_applied(
                    request.verb,
                    "commit_session requires a recovery persist path",
                )
            })?;
            // Applied iff snapshot or WAL FileIdentity changed (len/mtime/ino).
            let before = persist_fingerprint(&path);
            store.persist_pending()?;
            let after = persist_fingerprint(&path);
            if before == after {
                return Err(not_applied(
                    request.verb,
                    "commit_session persist did not mutate the recovery store",
                ));
            }
        }
        MemoryVerb::UpdateCapsule => {
            let text = request
                .payload
                .clone()
                .ok_or_else(|| not_applied(request.verb, "payload is required"))?;
            if text.is_empty() {
                return Err(not_applied(request.verb, "payload must be non-empty"));
            }
            let label = request
                .label
                .as_deref()
                .ok_or_else(|| not_applied(request.verb, "label (span path) is required"))?;
            if label.is_empty() {
                return Err(not_applied(
                    request.verb,
                    "label (span path) must be non-empty",
                ));
            }
            let Some(anchor) = working_set.anchor_for_path(Path::new(label)) else {
                return Err(not_applied(request.verb, format!("no capsule at {label}")));
            };
            require_visible_admit(
                request.verb,
                working_set.rewrite_render(store, text, anchor)?,
            )?;
        }
        MemoryVerb::ForgetVisible => {
            let id = span_id(request)?;
            if working_set.evict(store, id)?.is_none() {
                return Err(not_applied(
                    request.verb,
                    format!("forget_visible did not page out span {id}"),
                ));
            }
        }
        MemoryVerb::PromoteAnchor => {
            let id = span_id(request)?;
            if !working_set.touch(id) {
                return Err(not_applied(
                    request.verb,
                    format!("promote_anchor did not touch span {id}"),
                ));
            }
        }
        MemoryVerb::LinkRefs => {
            let (source, alias) = link_pair(request)?;
            if !working_set.link_refs(store, source, alias)? {
                return Err(not_applied(
                    request.verb,
                    format!("link_refs did not record {source} -> {alias}"),
                ));
            }
        }
    }
    Ok(MemoryVerbEffect {
        verb: request.verb,
        substrate: request.verb.substrate_target().to_string(),
        applied: true,
    })
}

fn require_visible_admit(
    verb: MemoryVerb,
    admission: crate::working_set::Admission,
) -> Result<(), MemoryVerbError> {
    if matches!(admission.response, WorkingSetResponse::AlreadyResident) {
        return Err(not_applied(
            verb,
            "already resident; no visible working-set mutation",
        ));
    }
    Ok(())
}

fn not_applied(verb: MemoryVerb, reason: impl Into<String>) -> MemoryVerbError {
    MemoryVerbError::NotApplied {
        verb: verb.as_str(),
        reason: reason.into(),
    }
}

fn payload_anchor(request: &MemoryVerbRequest) -> Result<(String, SpanAnchor), MemoryVerbError> {
    let text = request
        .payload
        .clone()
        .ok_or_else(|| not_applied(request.verb, "payload is required"))?;
    if text.is_empty() {
        return Err(not_applied(request.verb, "payload must be non-empty"));
    }
    let path = request
        .label
        .as_deref()
        .ok_or_else(|| not_applied(request.verb, "label (span path) is required"))?;
    if path.is_empty() {
        return Err(not_applied(
            request.verb,
            "label (span path) must be non-empty",
        ));
    }
    let end_line = text.lines().count().max(1);
    Ok((
        text,
        SpanAnchor {
            path: PathBuf::from(path),
            symbol: None,
            start_line: 1,
            end_line,
        },
    ))
}

fn span_id(request: &MemoryVerbRequest) -> Result<u64, MemoryVerbError> {
    let raw = request
        .ref_ids
        .first()
        .ok_or_else(|| not_applied(request.verb, "ref_ids[0] span id is required"))?;
    raw.parse::<u64>()
        .map_err(|_| not_applied(request.verb, format!("ref_ids[0] is not a span id: {raw}")))
}

fn link_pair(request: &MemoryVerbRequest) -> Result<(&str, &str), MemoryVerbError> {
    match request.ref_ids.as_slice() {
        [source, alias, ..] if !source.is_empty() && !alias.is_empty() => {
            Ok((source.as_str(), alias.as_str()))
        }
        _ => Err(not_applied(
            request.verb,
            "link_refs requires non-empty ref_ids[0] source and ref_ids[1] alias",
        )),
    }
}

/// Snapshot + WAL identity, same pairing `RecoveryStore` uses in `cache_identities`.
/// After the first snapshot, later persists append the session WAL and leave
/// the snapshot file's len+mtime unchanged; WAL identity is the applied signal.
fn persist_fingerprint(path: &Path) -> (Option<FileIdentity>, Option<FileIdentity>) {
    match SessionWal::new(path, SessionWalConfig::default()) {
        Ok(wal) => (wal.snapshot_identity(), wal.wal_identity()),
        Err(_) => (FileIdentity::capture(path), None),
    }
}
