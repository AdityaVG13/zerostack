//! Keep-gate schema contract: harness evidence must be
//! `fszero.surface_wire_ratchet.v1` or `scripts/apply_bench_ratchet.py` rejects it.

use fs_zero::{
    WIRE_RATCHET_MIN_SAMPLES, WIRE_RATCHET_MULTIPLIER, WIRE_RATCHET_SCHEMA, wire_evidence_document,
};

#[test]
fn wire_evidence_schema_matches_keep_gate_v1() {
    let walls = vec![100_000u64; WIRE_RATCHET_MIN_SAMPLES];
    let sha = "ab".repeat(32);
    let doc = wire_evidence_document(
        "deadbeef",
        3,
        "one validated warmup discarded",
        &sha,
        &sha,
        &walls,
        &walls,
        serde_json::json!({"responses_validated": true}),
    )
    .expect("wire evidence");
    assert_eq!(doc["schema"], WIRE_RATCHET_SCHEMA);
    assert_eq!(doc["schema"], "fszero.surface_wire_ratchet.v1");
    assert_eq!(doc["scope"], "persistent_stdio_json_rpc");
    assert_eq!(
        doc["ratchet"]["threshold_multiplier"],
        WIRE_RATCHET_MULTIPLIER
    );
    assert_eq!(sha.len(), 64);
}
