use super::*;

#[test]
fn amortized_cold_allocation_is_an_exact_fraction() {
    // 10 units of cold build over 3 uses: 10/3 per use, unreduced to floats.
    let campaign = ReuseCampaign::new(10, 5, 20, 3).unwrap();
    assert_eq!(campaign.amortized_cold_per_use(), (10, 3));
    // All-in per use: (10 + 3 * 5) / 3 = 25/3.
    assert_eq!(campaign.allocated_per_use(), (25, 3));
    assert_eq!(campaign.per_use_savings(), 15);
    // The total always includes the cold build.
    assert_eq!(campaign.total_campaign_cost(), 25);
    assert_eq!(campaign.alternative_total_cost(), 60);
    assert_eq!(campaign.campaign_surplus(), 35);
}

#[test]
fn strict_break_even_boundary_is_exact_on_both_sides() {
    // C = 10, s = 3 -> ceil(10/3) = 4 uses.
    let campaign = ReuseCampaign::new(10, 5, 8, 4).unwrap();
    assert_eq!(campaign.strict_break_even_uses(), 4);
    // At 3 uses the campaign has not broken even: 10 > 3 * 3.
    assert_eq!(ReuseCampaign::new(10, 5, 8, 3).unwrap().campaign_surplus(), -1);
    // At 4 uses it has: 10 <= 4 * 3.
    assert_eq!(campaign.campaign_surplus(), 2);
    // Exact division boundary: C = 12, s = 3 -> 4 uses exactly.
    let campaign = ReuseCampaign::new(12, 5, 8, 4).unwrap();
    assert_eq!(campaign.strict_break_even_uses(), 4);
    assert_eq!(campaign.campaign_surplus(), 0);
    // Zero cold build breaks even immediately.
    assert_eq!(ReuseCampaign::new(0, 5, 8, 1).unwrap().strict_break_even_uses(), 0);
}

#[test]
fn q99_break_even_uses_the_actual_sample() {
    // Savings sample of 100 values, 99th percentile is the 99th smallest.
    let mut sample: Vec<u64> = (1..=100).collect();
    sample.sort_unstable();
    let campaign = ReuseCampaign::new(990, 0, 100, 100).unwrap();
    // q99 = 99 -> ceil(990 / 99) = 10 uses.
    assert_eq!(campaign.q99_break_even_uses(&sample).unwrap(), 10);
    // At 9 uses it has not broken even at the Q99 savings; at 10 it has.
    let q99 = 99u64;
    assert!(990 > 9 * q99);
    assert!(990 <= 10 * q99);
    // Smaller samples: single observation uses that observation.
    assert_eq!(
        ReuseCampaign::new(100, 0, 10, 1)
            .unwrap()
            .q99_break_even_uses(&[7])
            .unwrap(),
        15 // ceil(100 / 7)
    );
}

#[test]
fn impossible_denominators_are_refused() {
    // No uses: denominator nonpositive.
    assert_eq!(
        ReuseCampaign::new(10, 5, 20, 0).unwrap_err(),
        CampaignError::Impossible("reuse campaign has no uses: denominator nonpositive")
    );
    // Alternative not strictly more expensive: no savings.
    assert_eq!(
        ReuseCampaign::new(10, 5, 5, 1).unwrap_err(),
        CampaignError::Impossible(
            "per-use savings nonpositive: the alternative is not strictly more expensive than the campaign"
        )
    );
    assert!(ReuseCampaign::new(10, 5, 4, 1).is_err());
    // Empty Q99 sample and zero Q99 savings.
    let campaign = ReuseCampaign::new(10, 5, 20, 1).unwrap();
    assert!(campaign.q99_break_even_uses(&[]).is_err());
    assert_eq!(
        campaign.q99_break_even_uses(&[0]).unwrap_err(),
        CampaignError::Impossible("zero Q99 savings: no finite break-even horizon")
    );
    // Wire decode refuses a zero reuse count.
    let wire = r#"{"cold_build_cost":10,"per_use_cost":5,"alternative_per_use_cost":20,"reuse_count":0}"#;
    assert!(serde_json::from_str::<ReuseCampaign>(wire).is_err());
}

#[test]
fn warm_run_claim_omitting_cold_build_is_refused() {
    let campaign = ReuseCampaign::new(100, 5, 20, 10).unwrap();
    assert_eq!(campaign.total_campaign_cost(), 150);
    // A claim below the cold build is a warm-run claim that omits it.
    assert_eq!(
        campaign.check_claim_includes_cold_build(99).unwrap_err(),
        CampaignError::WarmRunClaimOmitsColdBuild {
            claimed: 99,
            cold_build: 100,
        }
    );
    // A claim that covers the cold build passes.
    campaign.check_claim_includes_cold_build(100).unwrap();
    campaign.check_claim_includes_cold_build(150).unwrap();
}
