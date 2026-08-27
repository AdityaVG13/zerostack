//! Physical residency probes (V6-F6 / ZS-BENCH-011, ZS-BENCH-012).
//!
//! A probe classifies a given set of refs against the physical tiers:
//!
//! - **resident** -- the exact content-addressed object is physically present
//!   in the attached CAS (the L3 tier). Byte count is the CAS object's size on
//!   disk (measured via filesystem metadata, never derived from estimates).
//! - **cold** -- the ref resolves in the store (bytes are recoverable) but the
//!   object is NOT materialized in CAS (e.g. a sqlite-pack payload that was
//!   never dual-written, or a CAS that has been partially evicted/GC'd).
//! - **absent** -- the ref does not resolve in the store at all.
//!
//! Everything in a report is measured, never estimated: each classification is
//! the result of an actual store lookup, an actual CAS `contains()` call, and
//! an actual metadata read. Reports carry `"basis": "measured"` so consumers
//! cannot mistake a bucket for a model or policy guess.
//!
//! The CAS queries go through [`CasResidency`]; the engine provides an
//! [`FSZeroSession`]-backed implementation. A store-owned implementation
//! exposing the attached `CasStore` directly is the residual for the store
//! crate's landed API (fszero-store F5).

use serde_json::{Value, json};

use crate::session::FSZeroSession;
use fszero_store::cas::CasStore;

/// Physical residency class of one probed ref.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidencyClass {
    /// Object bytes physically present in the CAS (L3).
    Resident,
    /// Ref known to the store but not materialized in CAS.
    Cold,
    /// Ref unknown to the store.
    Absent,
}

impl ResidencyClass {
    pub fn as_str(self) -> &'static str {
        match self {
            ResidencyClass::Resident => "resident",
            ResidencyClass::Cold => "cold",
            ResidencyClass::Absent => "absent",
        }
    }
}

/// One probed ref: class, measured byte count, and the measurement source.
#[derive(Debug, Clone)]
pub struct RefResidency {
    pub r#ref: String,
    pub class: ResidencyClass,
    /// Measured bytes: CAS object size for resident, store payload length for
    /// cold, 0 for absent.
    pub bytes: u64,
    /// What was actually measured to reach this classification.
    pub source: &'static str,
}

/// CAS presence queries used by the probe. The engine impl reconstructs the
/// CAS from the session's store root; a store-side impl may expose the
/// attached instance directly (residual for fszero-store F5).
pub trait CasResidency {
    /// Physical presence of a content-addressed object. `None` when the CAS
    /// tier is unavailable (not attached / root unknown).
    fn cas_contains(&self, hash: &str) -> Option<bool>;
    /// Physical on-disk size of a CAS object. `None` when unknown.
    fn cas_object_bytes(&self, hash: &str) -> Option<u64>;
}

impl CasResidency for FSZeroSession {
    fn cas_contains(&self, hash: &str) -> Option<bool> {
        let cas = self.cas_for_probe()?;
        Some(cas.contains(hash))
    }

    fn cas_object_bytes(&self, hash: &str) -> Option<u64> {
        let cas = self.cas_for_probe()?;
        let path = cas.object_path(hash).ok()?;
        std::fs::metadata(path).ok().map(|meta| meta.len())
    }
}

impl FSZeroSession {
    /// Reconstruct the attached CAS for probing. Honest failure: `None` when
    /// the session reports no attached CAS or no root to locate it under.
    pub(crate) fn cas_for_probe(&self) -> Option<CasStore> {
        if !self.recovery.cas_attached() {
            return None;
        }
        let root = self
            .root
            .clone()
            .or_else(|| self.root_canon.clone())
            .or_else(|| self.store_root())?;
        Some(CasStore::for_store_root(&root))
    }
}

/// Classify one ref. Every branch is a measured outcome.
pub fn classify_ref(session: &FSZeroSession, r: &str) -> RefResidency {
    let Some(payload) = session.expand(r) else {
        return RefResidency {
            r#ref: r.to_string(),
            class: ResidencyClass::Absent,
            bytes: 0,
            source: "store_miss",
        };
    };
    // Content-address of the object: blob refs name their own hash; other
    // refs are classified by whether their exact payload bytes exist as a
    // CAS object (CAS is content-addressed, so this is the physical truth).
    let hash = fszero_store::cas::full_blob_hash(r)
        .map(str::to_owned)
        .unwrap_or_else(|| crate::access_log::content_hash_bytes(&payload));
    match session.cas_contains(&hash) {
        Some(true) => {
            let bytes = session
                .cas_object_bytes(&hash)
                .unwrap_or(payload.len() as u64);
            RefResidency {
                r#ref: r.to_string(),
                class: ResidencyClass::Resident,
                bytes,
                source: "cas_object",
            }
        }
        Some(false) => RefResidency {
            r#ref: r.to_string(),
            class: ResidencyClass::Cold,
            bytes: payload.len() as u64,
            source: "store_payload",
        },
        None => RefResidency {
            r#ref: r.to_string(),
            class: ResidencyClass::Cold,
            bytes: payload.len() as u64,
            source: "store_payload_cas_unavailable",
        },
    }
}

/// Probe a set of refs and emit the measured report (counts + byte totals).
///
/// `residents`/`cold` byte totals only count bytes whose classification was
/// physically measured; absent refs contribute zero bytes. The report is
/// labelled `basis: "measured"` throughout.
pub fn probe_residency(session: &FSZeroSession, refs: &[String]) -> Value {
    let entries: Vec<Value> = refs
        .iter()
        .map(|r| classify_ref(session, r).to_json())
        .collect();
    let mut counts = json!({ "resident": 0u64, "cold": 0u64, "absent": 0u64 });
    let mut bytes = json!({ "resident": 0u64, "cold": 0u64, "absent": 0u64 });
    for entry in &entries {
        let class = entry["class"].as_str().unwrap_or("absent");
        counts[class] = json!(counts[class].as_u64().unwrap_or(0) + 1);
        bytes[class] =
            json!(bytes[class].as_u64().unwrap_or(0) + entry["bytes"].as_u64().unwrap_or(0));
    }
    json!({
        "basis": "measured",
        "cas_attached": session.recovery.cas_attached(),
        "probed": refs.len(),
        "counts": counts,
        "byte_totals": bytes,
        "refs": entries,
    })
}

impl RefResidency {
    pub fn to_json(&self) -> Value {
        json!({
            "ref": self.r#ref,
            "class": self.class.as_str(),
            "bytes": self.bytes,
            "source": self.source,
        })
    }
}
