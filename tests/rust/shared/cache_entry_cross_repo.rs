//! W-hygiene: the cross-repo cache-entry fixture validates against the hub's
//! zero-abi CacheEntryV1 wire contract. The SAME fixture is deserialized by
//! GraphZero's witness_cache types in the GraphZero repo
//! (tests/engine/cache_entry_cross_repo.rs), proving wire compatibility of
//! the shared common subset and pinning the divergence (hub Hit values may
//! carry an optional verifier_receipt; GraphZero Hit values never do).

use zero_abi::{
    CACHE_ENTRY_SCHEMA_V1, CacheEntryV1, CacheValueV1,
};

const FIXTURE: &str = include_str!("../../../conformance/models/cache-entry-v1-cross-repo.json");

#[test]
fn cross_repo_fixture_deserializes_with_the_hub_cache_entry_contract() {
    let entry: CacheEntryV1 =
        serde_json::from_str(FIXTURE).expect("fixture must deserialize as hub CacheEntryV1");
    // Schema pinned to the shared wire constant.
    assert_eq!(
        entry.schema(),
        CACHE_ENTRY_SCHEMA_V1,
        "fixture must use the shared schema constant"
    );
    // Deserialization itself is the hub's fail-closed validation gate
    // (CacheEntryV1's Deserialize rebuilds through validate and rejects
    // unwitnessed roots, wrong schema, and malformed fields).

    // The common-subset value is a plain hit (no verifier receipt): this is
    // exactly the shape GraphZero can also deserialize.
    assert!(matches!(
        entry.value(),
        CacheValueV1::Hit {
            verifier_receipt: None,
            ..
        }
    ));

    // The hub can re-deserialize its own serialization to the identical
    // struct (wire-compatible; the hub normalizes root ordering on build,
    // which is a semantic no-op).
    let round_value = serde_json::to_value(&entry).unwrap();
    let round: CacheEntryV1 = serde_json::from_value(round_value).unwrap();
    assert_eq!(round, entry, "hub round trip must preserve the entry");
}

#[test]
fn cross_repo_fixture_accepts_a_hub_verifier_receipt_extension() {
    // Hub extension: a Hit may carry an optional verifier receipt. This is
    // the documented divergence from GraphZero (which rejects it); the
    // common subset stays interoperable.
    let mut value: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    value["value"]["verifier_receipt"] = serde_json::json!({
        "verifier": "verifier-a",
        "receipt_root": "fz://blob/verifier-receipt-0001"
    });
    let entry: CacheEntryV1 = serde_json::from_value(value).expect("hub accepts its extension");
    assert!(matches!(
        entry.value(),
        CacheValueV1::Hit {
            verifier_receipt: Some(_),
            ..
        }
    ));
}
