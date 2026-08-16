//! Snapshot-isolation contract and explicit stale-reader semantics
//! (ZS-OPS-002 / V6-R14).
//!
//! Concurrent reads and branch races are serializable at the root: every
//! commit is a parent-root CAS in `durable_journal` (a second writer from
//! the same parent observes `RootMismatch` -- never a torn or interleaved
//! root). This module adds the reader side of that contract:
//!
//! - [`take_root_snapshot_v1`] captures the current published root and its
//!   generation as an immutable [`SnapshotViewV1`].
//! - [`resolve_snapshot_read_v1`] resolves a reader's snapshot against the
//!   current root. Staleness is *explicit*: a stale reader receives a sealed
//!   [`SnapshotStalenessReceiptV1`] naming both the snapshot and the current
//!   root. The store never silently redirects a stale reader to newer data
//!   and never serves mixed roots: a read under a snapshot is either served
//!   exactly from the snapshot root or refused with the receipt.
//! - [`snapshot_isolation_contract_v1`] freezes the contract manifest:
//!   readers read exactly one root; stale readers are explicit; concurrent
//!   writers are serializable via the parent-root CAS; a branch race leaves
//!   exactly one authoritative root.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use zero_abi::{DigestV1, canonical_json};

use crate::durable_journal::{JournalErrorV1, JournalPathsV1, read_published_root_v1};

/// Schema version of snapshot artifacts.
pub const SNAPSHOT_SCHEMA_VERSION_V1: u16 = 1;
/// Domain tag bound into every staleness receipt digest.
pub const SNAPSHOT_STALENESS_DOMAIN_V1: &[u8] = b"zerostack.snapshot-staleness.v1\0";
/// ABI tag carried by snapshot artifacts.
pub const SNAPSHOT_ABI_VERSION_V1: &str = "v6-r14";

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
pub struct SnapshotViewV1 {
    pub root: DigestV1,
    pub generation: u64,
    pub taken_at_unix_ns: u64,
}

impl SnapshotViewV1 {
    pub fn new(root: DigestV1, generation: u64, taken_at_unix_ns: u64) -> Self {
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
pub struct SnapshotStalenessReceiptV1 {
    pub schema_version: u16,
    pub view_root: DigestV1,
    pub view_generation: u64,
    pub current_root: DigestV1,
    pub current_generation: u64,
    pub stale: bool,
    pub abi_version: String,
}

impl SnapshotStalenessReceiptV1 {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let value =
            serde_json::to_value(self).expect("staleness receipt is JSON-serializable");
        canonical_json(&value).into_bytes()
    }

    pub fn digest(&self) -> DigestV1 {
        let mut tagged = Vec::with_capacity(SNAPSHOT_STALENESS_DOMAIN_V1.len() + 128);
        tagged.extend_from_slice(SNAPSHOT_STALENESS_DOMAIN_V1);
        tagged.extend_from_slice(&self.canonical_bytes());
        DigestV1::from_bytes(zero_abi::sha256(&tagged))
    }
}

/// The result of resolving a reader snapshot against the current root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotReadResolutionV1 {
    pub view: SnapshotViewV1,
    pub current_root: DigestV1,
    pub current_generation: u64,
    pub stale: bool,
    pub receipt: SnapshotStalenessReceiptV1,
}

/// Capture a snapshot of the current published root. The snapshot is
/// immutable: later commits move the root but never mutate the view.
pub fn take_root_snapshot_v1(paths: &JournalPathsV1) -> Result<SnapshotViewV1, JournalErrorV1> {
    let root = read_published_root_v1(paths)?;
    Ok(SnapshotViewV1::new(
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
pub fn resolve_snapshot_read_v1(
    paths: &JournalPathsV1,
    view: SnapshotViewV1,
) -> Result<SnapshotReadResolutionV1, JournalErrorV1> {
    let current = read_published_root_v1(paths)?;
    let stale = current.root_digest != view.root || current.generation != view.generation;
    let receipt = SnapshotStalenessReceiptV1 {
        schema_version: SNAPSHOT_SCHEMA_VERSION_V1,
        view_root: view.root,
        view_generation: view.generation,
        current_root: current.root_digest,
        current_generation: current.generation,
        stale,
        abi_version: SNAPSHOT_ABI_VERSION_V1.to_owned(),
    };
    let _ = receipt.digest();
    Ok(SnapshotReadResolutionV1 {
        view,
        current_root: current.root_digest,
        current_generation: current.generation,
        stale,
        receipt,
    })
}

/// The frozen snapshot-isolation contract manifest (ZS-OPS-002).
pub fn snapshot_isolation_contract_v1() -> serde_json::Value {
    serde_json::json!({
        "schema_version": SNAPSHOT_SCHEMA_VERSION_V1,
        "reader": {
            "serves": "exactly the snapshot root; never a mixed or silently advanced root",
            "stale_reader": "explicit sealed receipt (view root vs current root); never silent",
        },
        "writer": {
            "commit": "parent-root CAS; one winner per parent; losers observe RootMismatch",
            "branch_race": "exactly one authoritative root; losing branches are unreferenced, never partial",
        },
        "abi_version": SNAPSHOT_ABI_VERSION_V1,
    })
}

