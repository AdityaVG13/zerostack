//! K0 session-state persistence over the published CAS (zerostack-7inx).
//!
//! The K0 guest surface's small serializable state map survives fresh
//! executors through the existing content-addressed store. The committed
//! session state is one canonical JSON object in the shared CAS under the
//! session state root; the session root is its SHA-256 identity; and the
//! supervisor keeps the current committed root in a per-session pointer
//! file. A successful, quiescent call may CAS one successor; every other
//! terminal (syntax error, exception, deadline, cancellation, worker
//! crash, output limit, stale root, conflict) leaves the committed roots
//! unchanged and performs no write. No live JS heap is authoritative —
//! every fresh executor hydrates from the committed object.
//!
//! # Layout
//!
//! - Objects: `<state_root>/blobs/sha256/<hh>/<hash>` (the published
//!   [`SharedCas`] layout, unchanged).
//! - Pointer: `<state_root>/session/<session_id>.root`, a plain lowercase
//!   64-hex SHA-256 identity, atomically replaced.
//!
//! # Commit law
//!
//! The exact compare-and-swap runs entirely under the store's single
//! exclusive coordination lock ([`SharedCas::lock_for_sweep`]): read the
//! current pointer, compare it with the request's expected root, publish
//! the successor object ([`SharedCas::put_in_lock`], one collector
//! boundary with the pointer replacement), and atomically replace the
//! pointer. A mismatch is a typed conflict that performs **no write at
//! all** — no object publish, no pointer change. A call whose final state
//! equals its hydrated base also performs no write and keeps the committed
//! root. A request without an expected root commits unconditionally
//! (last-writer-wins, no precondition).
//!
//! # Bounds
//!
//! The state map is bounded by the K0 guest budgets (64 keys, 128-byte
//! keys, 4 KiB values, 16 KiB total), so the successor object is small;
//! reads and writes are refused past a documented ceiling with slack.
//! Values are JSON only — opaque or large runtime values never enter the
//! state map (`z.state.set` rejects them) — so the persisted state never
//! carries a live object; handles stay opaque strings.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;

use zero_abi::guest::{
    K0_STATE_MAX_KEY_BYTES, K0_STATE_MAX_KEYS, K0_STATE_MAX_TOTAL_BYTES,
    K0_STATE_MAX_VALUE_BYTES,
};
use zero_abi::schema::canonical_json;
use zero_store::{
    CasError, SharedCas, SyncPolicy, atomic_write_file_with_sync,
};

/// Directory under the session state root that hosts the per-session
/// committed-root pointers.
pub const SESSION_ROOTS_DIR: &str = "session";

/// Suffix of the per-session committed-root pointer file.
pub const SESSION_ROOT_SUFFIX: &str = ".root";

/// Ceiling for reading and writing session-state objects. The guest state
/// budget caps the map at 16 KiB of values plus 64 keys of up to 128
/// bytes; 64 KiB covers the canonical encoding with slack and keeps the
/// store policy bounded.
pub const SESSION_STATE_MAX_OBJECT_BYTES: u64 = 64 * 1024;

/// Whether `value` is a full lowercase 64-hex SHA-256 CAS identity — the
/// only session-root shape the K0 supervisor accepts.
pub fn is_session_root_identity(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Path of the per-session committed-root pointer.
pub fn session_root_pointer(state_root: &Path, session_id: &str) -> PathBuf {
    state_root
        .join(SESSION_ROOTS_DIR)
        .join(format!("{session_id}{SESSION_ROOT_SUFFIX}"))
}

/// The committed session root of `session_id`, if one was ever committed.
/// A missing pointer is the fresh-session state; a malformed pointer is a
/// fail-closed error (the supervisor never silently resets).
pub fn current_session_root(
    state_root: &Path,
    session_id: &str,
) -> Result<Option<String>, String> {
    let path = session_root_pointer(state_root, session_id);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let root = text.trim().to_owned();
            if !is_session_root_identity(&root) {
                return Err(format!(
                    "session root pointer {} is malformed (expected a lowercase 64-hex SHA-256 identity)",
                    path.display()
                ));
            }
            Ok(Some(root))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "cannot read session root pointer {}: {error}",
            path.display()
        )),
    }
}

/// Validate one parsed state map against the K0 guest budgets — the same
/// key/value/total bounds `z.state.set` and hydration enforce. A committed
/// or expected root that violates them is typed store corruption, never a
/// silent reset.
pub fn validate_state_map(state: &BTreeMap<String, JsonValue>) -> Result<(), String> {
    if state.len() > K0_STATE_MAX_KEYS {
        return Err(format!(
            "state holds {} keys, above the {K0_STATE_MAX_KEYS}-key bound",
            state.len()
        ));
    }
    let mut total = 0usize;
    for (key, value) in state {
        if key.is_empty() || key.len() > K0_STATE_MAX_KEY_BYTES {
            return Err(format!(
                "state key '{}' is outside the 1..{K0_STATE_MAX_KEY_BYTES}-byte bound",
                key.chars().take(16).collect::<String>()
            ));
        }
        let encoded = serde_json::to_string(value)
            .map_err(|error| format!("state value is not serializable: {error}"))?;
        if encoded.len() > K0_STATE_MAX_VALUE_BYTES {
            return Err(format!(
                "state value for '{key}' is {} bytes, above the {K0_STATE_MAX_VALUE_BYTES}-byte per-value bound",
                encoded.len()
            ));
        }
        total = total.saturating_add(encoded.len());
        if total > K0_STATE_MAX_TOTAL_BYTES {
            return Err(format!(
                "state total would reach {total} bytes, above the {K0_STATE_MAX_TOTAL_BYTES}-byte budget"
            ));
        }
    }
    Ok(())
}

/// Read and verify one committed session-state object: a canonical JSON
/// object of the bounded state map. Read-only; the object must exist,
/// verify against its identity, and satisfy the K0 state budgets.
pub fn read_state_map(
    cas: &SharedCas,
    root: &str,
) -> Result<BTreeMap<String, JsonValue>, String> {
    let bytes = cas
        .get_verified_limited(root, SESSION_STATE_MAX_OBJECT_BYTES)
        .map_err(|error| match error {
            CasError::NotFound => {
                format!("session state root {root} is not present in the store")
            }
            other => format!("session state root {root} is unreadable: {other}"),
        })?;
    let value: JsonValue = serde_json::from_slice(&bytes)
        .map_err(|error| format!("session state root {root} is not valid JSON: {error}"))?;
    let state = serde_json::from_value::<BTreeMap<String, JsonValue>>(value).map_err(|error| {
        format!(
            "session state root {root} is not a JSON object of state entries: {error}"
        )
    })?;
    validate_state_map(&state)
        .map_err(|detail| format!("session state root {root} violates the state budgets: {detail}"))?;
    Ok(state)
}

/// Canonical bytes of one state map (deterministic object content).
pub fn state_object_bytes(
    state: &BTreeMap<String, JsonValue>,
) -> Result<Vec<u8>, String> {
    let value = serde_json::to_value(state)
        .map_err(|error| format!("session state is not serializable: {error}"))?;
    Ok(canonical_json(&value).into_bytes())
}

/// One commit attempt outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommitOutcome {
    /// A successor object was published and the pointer CAS won.
    Committed { successor: String },
    /// The call left the state map equal to its hydrated base: no write,
    /// the committed root stays.
    Unchanged,
    /// The request's expected root does not equal the committed root: no
    /// write at all (the caller reports the typed conflict).
    Conflict { current: Option<String> },
}

/// Exact compare-and-swap of one session-state successor.
///
/// Runs entirely under the store's exclusive coordination lock, so the
/// pointer read, the successor publish, and the pointer replacement are
/// one collector/commit boundary and concurrent successors serialize.
/// `expected` is the request's expected session root (`None` commits
/// unconditionally); `baseline` is the state map the call hydrated from;
/// `final_state` is the per-call state after the plan settled. When
/// `final_state` equals `baseline` no write happens even on a matching
/// root.
pub fn commit_successor(
    state_root: &Path,
    session_id: &str,
    expected: Option<&str>,
    baseline: &BTreeMap<String, JsonValue>,
    final_state: &BTreeMap<String, JsonValue>,
) -> Result<CommitOutcome, String> {
    let cas = SharedCas::open(state_root.to_path_buf());
    let guard = cas
        .lock_for_sweep()
        .map_err(|error| format!("cannot take the session store commit lock: {error}"))?;
    let current = current_session_root(state_root, session_id)?;
    if let Some(expected) = expected {
        if current.as_deref() != Some(expected) {
            return Ok(CommitOutcome::Conflict { current });
        }
    }
    if final_state == baseline {
        return Ok(CommitOutcome::Unchanged);
    }
    let bytes = state_object_bytes(final_state)?;
    let successor = cas
        .put_in_lock(&bytes, SESSION_STATE_MAX_OBJECT_BYTES, &guard)
        .map_err(|error| format!("cannot publish session state successor: {error}"))?
        .hash;
    write_pointer(state_root, session_id, &successor)?;
    Ok(CommitOutcome::Committed { successor })
}

/// Atomically replace the per-session pointer (the caller holds the
/// exclusive coordination lock).
fn write_pointer(state_root: &Path, session_id: &str, root: &str) -> Result<(), String> {
    let path = session_root_pointer(state_root, session_id);
    let mut bytes = root.as_bytes().to_vec();
    bytes.push(b'\n');
    atomic_write_file_with_sync(&path, &bytes, SyncPolicy::Required).map_err(|error| {
        format!(
            "cannot commit session root pointer {}: {error}",
            path.display()
        )
    })
}
