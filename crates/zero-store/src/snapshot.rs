//! Snapshot-isolation contract and explicit stale-reader semantics
//! (ZS-OPS-002 / V6-R14).
//!
//! Concurrent reads and branch races are serializable at the root: every
//! commit is a parent-root CAS in `durable_journal` (a second writer from
//! the same parent observes `RootMismatch` -- never a torn or interleaved
//! root). This module adds the reader side of that contract:
//!
//! - [`take_root_snapshot`] captures the current published root and its
//!   generation as an immutable [`SnapshotView`].
//! - [`resolve_snapshot_read`] resolves a reader's snapshot against the
//!   current root. Staleness is *explicit*: a stale reader receives a sealed
//!   [`SnapshotStalenessReceipt`] naming both the snapshot and the current
//!   root. The store never silently redirects a stale reader to newer data
//!   and never serves mixed roots: a read under a snapshot is either served
//!   exactly from the snapshot root or refused with the receipt.
//! - [`snapshot_isolation_contract`] freezes the contract manifest:
//!   readers read exactly one root; stale readers are explicit; concurrent
//!   writers are serializable via the parent-root CAS; a branch race leaves
//!   exactly one authoritative root.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use zero_abi::{Sha256Digest, canonical_json};

use crate::durable_journal::{JournalError, JournalPaths, read_published_root};

/// Schema version of snapshot artifacts.
pub const SNAPSHOT_SCHEMA_VERSION: u16 = 1;
/// Domain tag bound into every staleness receipt digest.
pub const SNAPSHOT_STALENESS_DOMAIN: &[u8] = b"zerostack.snapshot-staleness\0";
/// ABI tag carried by snapshot artifacts.
pub const SNAPSHOT_ABI_VERSION: &str = "v6-r14";

fn now_unix_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
}

/// An immutable read snapshot of the published root: the root digest plus
/// the generation of the root record that published it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotView {
    pub root: Sha256Digest,
    pub generation: u64,
    pub taken_at_unix_ns: u64,
}

impl SnapshotView {
    pub fn new(root: Sha256Digest, generation: u64, taken_at_unix_ns: u64) -> Self {
        Self {
            root,
            generation,
            taken_at_unix_ns,
        }
    }
}

/// Sealed receipt for one snapshot resolution. `stale == false` means the
/// snapshot root is still the current root; `stale == true` is the explicit
/// stale-reader artifact: the read must either re-snapshot or be refused --
/// the store never silently serves newer data to a stale view.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotStalenessReceipt {
    pub schema_version: u16,
    pub view_root: Sha256Digest,
    pub view_generation: u64,
    pub current_root: Sha256Digest,
    pub current_generation: u64,
    pub stale: bool,
    pub abi_version: String,
}

impl SnapshotStalenessReceipt {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let value =
            serde_json::to_value(self).expect("staleness receipt is JSON-serializable");
        canonical_json(&value).into_bytes()
    }

    pub fn digest(&self) -> Sha256Digest {
        let mut tagged = Vec::with_capacity(SNAPSHOT_STALENESS_DOMAIN.len() + 128);
        tagged.extend_from_slice(SNAPSHOT_STALENESS_DOMAIN);
        tagged.extend_from_slice(&self.canonical_bytes());
        Sha256Digest::from_bytes(zero_abi::sha256(&tagged))
    }
}

/// The result of resolving a reader snapshot against the current root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotReadResolution {
    pub view: SnapshotView,
    pub current_root: Sha256Digest,
    pub current_generation: u64,
    pub stale: bool,
    pub receipt: SnapshotStalenessReceipt,
}

/// Capture a snapshot of the current published root. The snapshot is
/// immutable: later commits move the root but never mutate the view.
pub fn take_root_snapshot(paths: &JournalPaths) -> Result<SnapshotView, JournalError> {
    let root = read_published_root(paths)?;
    Ok(SnapshotView::new(
        root.root_digest,
        root.generation,
        now_unix_ns(),
    ))
}

/// Resolve a reader snapshot against the current published root.
///
/// Explicit stale-reader semantics: when the snapshot root differs from the
/// current root the resolution is `stale == true` with a sealed receipt
/// naming both roots and generations. The caller must re-snapshot or refuse
/// the read; a stale view is never silently advanced.
pub fn resolve_snapshot_read(
    paths: &JournalPaths,
    view: SnapshotView,
) -> Result<SnapshotReadResolution, JournalError> {
    let current = read_published_root(paths)?;
    let stale = current.root_digest != view.root || current.generation != view.generation;
    let receipt = SnapshotStalenessReceipt {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        view_root: view.root,
        view_generation: view.generation,
        current_root: current.root_digest,
        current_generation: current.generation,
        stale,
        abi_version: SNAPSHOT_ABI_VERSION.to_owned(),
    };
    let _ = receipt.digest();
    Ok(SnapshotReadResolution {
        view,
        current_root: current.root_digest,
        current_generation: current.generation,
        stale,
        receipt,
    })
}

/// The frozen snapshot-isolation contract manifest (ZS-OPS-002).
pub fn snapshot_isolation_contract() -> serde_json::Value {
    serde_json::json!({
        "schema_version": SNAPSHOT_SCHEMA_VERSION,
        "reader": {
            "serves": "exactly the snapshot root; never a mixed or silently advanced root",
            "stale_reader": "explicit sealed receipt (view root vs current root); never silent",
        },
        "writer": {
            "commit": "parent-root CAS; one winner per parent; losers observe RootMismatch",
            "branch_race": "exactly one authoritative root; losing branches are unreferenced, never partial",
        },
        "abi_version": SNAPSHOT_ABI_VERSION,
    })
}

