//! Session residency report honesty for an empty demand window.

use zsx_core::SessionResidencyGate;

#[test]
fn empty_window_report_is_unavailable_never_numeric() {
    let gate = SessionResidencyGate::new("empty-q99");
    let report = gate.report(0).expect("empty report is a first-class state");
    assert!(report.unavailable, "empty windows must be unavailable");
    assert_eq!(report.demanded_mass, 0);
    assert!(report.eviction_slack_ppm.is_none());
    assert!(
        report
            .reasons
            .iter()
            .any(|reason| reason.contains("no_demand_observations")),
        "reasons={:?}",
        report.reasons
    );
    for tier in &report.tiers {
        assert!(tier.window.unavailable);
        assert_eq!(tier.window.demanded_mass, 0);
        assert_eq!(tier.denominator_label, "q99_demanded_mass:0");
        assert!(
            tier.window
                .reasons
                .iter()
                .any(|reason| reason == "no_demand_observations"),
            "tier={:?} reasons={:?}",
            tier.tier,
            tier.window.reasons
        );
        let dumped = serde_json::to_string(&tier.window).expect("serialize");
        assert!(
            !dumped.contains('%'),
            "empty window must not emit a bare percentage: {dumped}"
        );
    }
}
