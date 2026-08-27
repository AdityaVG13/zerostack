//! SPEC-TZ-ELIG-001 / ELIG-002: eligibility vs hit are declared, never from LCP.

use tokenzero_engine::{replay_prefix_probe, ProbeArm, ProbeFixture, QualitySlot};
use tokenzero_test_support::{GauntletIdentityPair, GauntletOracle};

#[test]
fn raw_retained_full_lcp_is_not_a_declared_hit() {
    GauntletIdentityPair::new(GauntletOracle::Spec).assert_distinct();
    let fixture: ProbeFixture = serde_json::from_str(include_str!(
        "../../engine/fixtures/prefix-probe-replay.json"
    ))
    .expect("prefix-probe-replay.json");
    assert_eq!(fixture.schema, "tokenzero.prefix-probe.v1");

    let reports = replay_prefix_probe(&fixture);
    let raw = reports
        .iter()
        .find(|report| report.arm == ProbeArm::RawRetained)
        .expect("raw_retained arm");

    assert!(
        raw.lcp_tokens > 0,
        "fixture documents full prefix retention (lcp_tokens={})",
        raw.lcp_tokens
    );
    assert_eq!(
        raw.hit_declared_by_provider,
        Some(false),
        "LCP must not be rewritten into a provider hit"
    );
    assert_eq!(raw.eligibility_declared, Some(true));
    assert_ne!(raw.quality_slot, QualitySlot::NoReuse);
    assert_ne!(
        raw.eligibility_declared.map(|_| true),
        raw.hit_declared_by_provider,
        "eligibility and hit stay distinct declared facts"
    );
}
