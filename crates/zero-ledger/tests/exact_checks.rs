//! Ledger/receipt subset of the RACC exact-check suite, ported to Rust.
//!
//! Mirrors docs/racc/RACC_CONTRACT.rs unit tests and the ledger-relevant cases
//! of docs/racc/RACC_EXACT_CHECKS.py (test_replay_exposure_identity and the
//! phase-target arithmetic).

use proptest::prelude::*;
use zero_ledger::{
    Digest, DominanceReceipt, ExactnessGates, ExposureAccount, ExposureBlock, ExposureSide,
    LedgerConfig, LedgerError, ReceiptError, ReceiptRoots, ResourceGauge, RetainedFractionPpm,
    TokenCharge, TokenLedger, TokenizerIdentity, PPM_ONE,
};

fn d(x: u8) -> Digest {
    Digest([x; 32])
}

fn tokenizer() -> TokenizerIdentity {
    TokenizerIdentity::new("cl100k_base", d(7))
}

fn other_tokenizer() -> TokenizerIdentity {
    TokenizerIdentity::new("o200k_base", d(8))
}

fn gauge() -> ResourceGauge {
    ResourceGauge::new(LedgerConfig::new(tokenizer()))
}

fn all_gates() -> ExactnessGates {
    ExactnessGates {
        byte_exact: true,
        policy_exact_or_fallback: true,
        task_verified: true,
    }
}

fn roots() -> ReceiptRoots {
    ReceiptRoots {
        archive_root: d(1),
        certificate_root: d(2),
    }
}

fn receipt_with(raw: u64, racc: u64, target_ppm: u32) -> DominanceReceipt {
    let mut ledger = TokenLedger::empty(tokenizer());
    ledger.raw_input_tokens = raw;
    ledger.racc_input_tokens = racc;
    ledger.model_calls = 1;
    DominanceReceipt {
        ledger,
        target_retained_ppm: RetainedFractionPpm(target_ppm),
        archive_root: d(1),
        certificate_root: d(2),
        byte_exact: true,
        policy_exact_or_fallback: true,
        task_verified: true,
    }
}

// --- RACC_CONTRACT.rs ported unit tests -----------------------------------

#[test]
fn checks_97_percent_saving() {
    let receipt = receipt_with(1_000_000, 30_000, 30_000);
    assert!(receipt.exact_phase_valid());
}

#[test]
fn rejects_receipt_over_target() {
    let receipt = receipt_with(1_000_000, 30_001, 30_000);
    assert!(!receipt.meets_token_target());
    assert!(!receipt.exact_phase_valid());
}

#[test]
fn exactness_gates_are_conjunctive() {
    for i in 0..3 {
        let mut receipt = receipt_with(1_000_000, 30_000, 30_000);
        match i {
            0 => receipt.byte_exact = false,
            1 => receipt.policy_exact_or_fallback = false,
            _ => receipt.task_verified = false,
        }
        assert!(
            receipt.meets_token_target(),
            "arithmetic still holds for gate {i}"
        );
        assert!(
            !receipt.exact_phase_valid(),
            "gate {i} must veto the phase certificate"
        );
    }
}

// --- ppm boundary exactness -----------------------------------------------

#[test]
fn ppm_boundaries_are_exact() {
    // 0 ppm: only a zero RACC cost can pass.
    assert!(receipt_with(1_000_000, 0, 0).meets_token_target());
    assert!(!receipt_with(1_000_000, 1, 0).meets_token_target());

    // 1 ppm against a 1e6 baseline: exactly one token is allowed.
    assert!(receipt_with(1_000_000, 1, 1).meets_token_target());
    assert!(!receipt_with(1_000_000, 2, 1).meets_token_target());

    // 999_999 ppm: one token short of the baseline passes, the baseline does not.
    assert!(receipt_with(1_000_000, 999_999, 999_999).meets_token_target());
    assert!(!receipt_with(1_000_000, 1_000_000, 999_999).meets_token_target());

    // 1_000_000 ppm is the identity target: parity passes, one over fails.
    assert!(receipt_with(1_000_000, 1_000_000, PPM_ONE).meets_token_target());
    assert!(!receipt_with(1_000_000, 1_000_001, PPM_ONE).meets_token_target());
}

#[test]
fn zero_raw_baseline_only_passes_at_zero_cost() {
    assert!(receipt_with(0, 0, PPM_ONE).meets_token_target());
    assert!(!receipt_with(0, 1, PPM_ONE).meets_token_target());
    assert_eq!(
        receipt_with(0, 0, PPM_ONE).achieved_retained_ppm_ceil(),
        None
    );
}

#[test]
fn no_overflow_at_u64_max_tokens() {
    let receipt = receipt_with(u64::MAX, u64::MAX, PPM_ONE);
    assert!(receipt.meets_token_target());
    assert_eq!(
        receipt.achieved_retained_ppm_ceil(),
        Some(u128::from(PPM_ONE))
    );

    let receipt = receipt_with(u64::MAX, u64::MAX, PPM_ONE - 1);
    assert!(!receipt.meets_token_target());
}

#[test]
fn achieved_retained_ppm_rounds_up() {
    // 1/3 of 1e6 is 333_333.33..; the ceiling must not understate the cost.
    assert_eq!(
        receipt_with(3, 1, PPM_ONE).achieved_retained_ppm_ceil(),
        Some(333_334)
    );
    assert_eq!(
        receipt_with(4, 1, PPM_ONE).achieved_retained_ppm_ceil(),
        Some(250_000)
    );
}

#[test]
fn retained_fraction_rejects_out_of_range() {
    assert_eq!(RetainedFractionPpm::new(PPM_ONE).unwrap().0, PPM_ONE);
    assert_eq!(
        RetainedFractionPpm::new(PPM_ONE + 1),
        Err(LedgerError::PpmOutOfRange { ppm: PPM_ONE + 1 })
    );
}

// --- locked tokenizer gauge -----------------------------------------------

#[test]
fn mixed_tokenizer_identity_is_rejected() {
    let mut g = gauge();
    let charge = TokenCharge {
        raw_input_tokens: 100,
        racc_input_tokens: 3,
        model_calls: 1,
        ..Default::default()
    };
    g.charge(&tokenizer(), &charge).unwrap();

    let err = g.charge(&other_tokenizer(), &charge).unwrap_err();
    assert!(matches!(err, LedgerError::TokenizerIdentityMismatch { .. }));
    // The rejected charge left no trace.
    assert_eq!(g.charge_count(), 1);
    assert_eq!(g.ledger().raw_input_tokens, 100);
}

#[test]
fn same_id_different_digest_is_rejected() {
    let mut g = gauge();
    let impostor = TokenizerIdentity::new("cl100k_base", d(9));
    let err = g.charge(&impostor, &TokenCharge::default()).unwrap_err();
    assert!(matches!(err, LedgerError::TokenizerIdentityMismatch { .. }));
}

#[test]
fn ledger_is_tagged_with_the_locked_gauge() {
    let g = gauge();
    assert_eq!(g.ledger().tokenizer, tokenizer());
    assert_eq!(g.config().tokenizer, tokenizer());
}

// --- monotonicity and overflow --------------------------------------------

#[test]
fn counter_overflow_is_typed_and_leaves_ledger_intact() {
    let mut g = gauge();
    g.charge(
        &tokenizer(),
        &TokenCharge {
            raw_input_tokens: u64::MAX,
            ..Default::default()
        },
    )
    .unwrap();
    let err = g
        .charge(
            &tokenizer(),
            &TokenCharge {
                raw_input_tokens: 1,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(
        err,
        LedgerError::CounterOverflow {
            counter: "raw_input_tokens"
        }
    );
    assert_eq!(g.ledger().raw_input_tokens, u64::MAX);
    assert_eq!(g.charge_count(), 1);
}

#[test]
fn partial_overflow_does_not_apply_earlier_fields() {
    let mut g = gauge();
    g.charge(
        &tokenizer(),
        &TokenCharge {
            retries: u64::MAX,
            ..Default::default()
        },
    )
    .unwrap();
    let err = g
        .charge(
            &tokenizer(),
            &TokenCharge {
                raw_input_tokens: 5,
                retries: 1,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(err, LedgerError::CounterOverflow { counter: "retries" });
    assert_eq!(
        g.ledger().raw_input_tokens,
        0,
        "no field may be applied when a later one overflows"
    );
}

// --- receipt sealing ------------------------------------------------------

#[test]
fn finalize_requires_a_nonempty_ledger() {
    let g = gauge();
    assert_eq!(
        g.finalize_receipt(RetainedFractionPpm(30_000), roots(), all_gates()),
        Err(ReceiptError::IncompleteLedger)
    );
}

#[test]
fn finalize_rejects_out_of_range_target() {
    let mut g = gauge();
    g.charge(
        &tokenizer(),
        &TokenCharge {
            raw_input_tokens: 10,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        g.finalize_receipt(RetainedFractionPpm(PPM_ONE + 1), roots(), all_gates()),
        Err(ReceiptError::TargetOutOfRange { ppm: PPM_ONE + 1 })
    );
}

#[test]
fn finalize_carries_the_accumulated_ledger() {
    let mut g = gauge();
    for _ in 0..10 {
        g.charge(
            &tokenizer(),
            &TokenCharge {
                raw_input_tokens: 100_000,
                racc_input_tokens: 2_000,
                model_calls: 1,
                ..Default::default()
            },
        )
        .unwrap();
    }
    let receipt = g
        .finalize_receipt(RetainedFractionPpm(30_000), roots(), all_gates())
        .unwrap();
    assert_eq!(receipt.ledger.raw_input_tokens, 1_000_000);
    assert_eq!(receipt.ledger.racc_input_tokens, 20_000);
    assert_eq!(receipt.ledger.model_calls, 10);
    assert!(receipt.exact_phase_valid());
}

// --- T8 exposure / replay identity ----------------------------------------

/// One case from RACC_EXACT_CHECKS.py::test_replay_exposure_identity.
struct ExposureCase {
    h_raw: u64,
    h_tz: u64,
    b: Vec<u64>,
    r: Vec<u64>,
    d: Vec<u64>,
}

fn exposure_cases() -> Vec<ExposureCase> {
    vec![
        ExposureCase {
            h_raw: 10,
            h_tz: 3,
            b: vec![10, 20, 30],
            r: vec![5, 2, 1],
            d: vec![1, 0, 1],
        },
        ExposureCase {
            h_raw: 0,
            h_tz: 7,
            b: vec![1, 1, 1, 1],
            r: vec![100, 100, 100, 100],
            d: vec![1, 1, 0, 0],
        },
        ExposureCase {
            h_raw: 17,
            h_tz: 19,
            b: vec![7, 13],
            r: vec![4, 9],
            d: vec![2, 3],
        },
    ]
}

fn account(h_raw: u64, h_tz: u64, b: &[u64], r: &[u64], d: &[u64]) -> ExposureAccount {
    ExposureAccount {
        raw_fixed_overhead: h_raw,
        racc_fixed_overhead: h_tz,
        blocks: (0..b.len())
            .map(|i| ExposureBlock {
                block_tokens: b[i],
                raw_exposures: r[i],
                racc_exposures: d[i],
            })
            .collect(),
    }
}

#[test]
fn exposure_costs_match_the_paper_formulas() {
    for (j, case) in exposure_cases().into_iter().enumerate() {
        let ExposureCase {
            h_raw,
            h_tz,
            b,
            r,
            d,
        } = case;
        let acct = account(h_raw, h_tz, &b, &r, &d);
        let expected_raw: u128 = u128::from(h_raw)
            + (0..b.len())
                .map(|i| u128::from(b[i]) * u128::from(r[i]))
                .sum::<u128>();
        let expected_racc: u128 = u128::from(h_tz)
            + (0..b.len())
                .map(|i| u128::from(b[i]) * u128::from(d[i]))
                .sum::<u128>();
        assert_eq!(acct.raw_cost().unwrap(), expected_raw, "case {j} raw");
        assert_eq!(acct.racc_cost().unwrap(), expected_racc, "case {j} racc");

        // exposure-general-{j}: saving == 1 - C_tz / C_raw, checked exactly.
        let (num, den) = acct.exact_saving_ratio().unwrap();
        assert_eq!(den, expected_raw, "case {j} denominator");
        assert_eq!(
            num,
            expected_raw.saturating_sub(expected_racc),
            "case {j} numerator"
        );
    }
}

#[test]
fn replay_identity_holds_for_a_consistent_ledger() {
    for (j, case) in exposure_cases().into_iter().enumerate() {
        let ExposureCase {
            h_raw,
            h_tz,
            b,
            r,
            d,
        } = case;
        let acct = account(h_raw, h_tz, &b, &r, &d);
        let mut ledger = TokenLedger::empty(tokenizer());
        ledger.raw_input_tokens = u64::try_from(acct.raw_cost().unwrap()).unwrap();
        // Split the RACC side across billed / recovery / re-expansion / fallback
        // to prove the identity sums every exposure channel.
        let racc = u64::try_from(acct.racc_cost().unwrap()).unwrap();
        ledger.racc_input_tokens = racc / 4;
        ledger.recovery_tokens = racc / 4;
        ledger.reexpansion_tokens = racc / 4;
        ledger.fallback_tokens = racc - 3 * (racc / 4);
        acct.check_replay_identity(&ledger)
            .unwrap_or_else(|e| panic!("case {j}: {e}"));
    }
}

#[test]
fn replay_identity_detects_raw_side_drift() {
    let acct = account(10, 3, &[10, 20, 30], &[5, 2, 1], &[1, 0, 1]);
    let mut ledger = TokenLedger::empty(tokenizer());
    ledger.raw_input_tokens = u64::try_from(acct.raw_cost().unwrap()).unwrap() + 1;
    ledger.racc_input_tokens = u64::try_from(acct.racc_cost().unwrap()).unwrap();
    let err = acct.check_replay_identity(&ledger).unwrap_err();
    assert!(matches!(
        err,
        LedgerError::ReplayIdentityMismatch {
            side: ExposureSide::Raw,
            ..
        }
    ));
}

#[test]
fn replay_identity_detects_unbilled_recovery() {
    let acct = account(10, 3, &[10, 20, 30], &[5, 2, 1], &[1, 0, 1]);
    let mut ledger = TokenLedger::empty(tokenizer());
    ledger.raw_input_tokens = u64::try_from(acct.raw_cost().unwrap()).unwrap();
    // Hiding the re-expansion surcharge understates C_tz and must be caught.
    ledger.racc_input_tokens = u64::try_from(acct.racc_cost().unwrap()).unwrap() - 1;
    let err = acct.check_replay_identity(&ledger).unwrap_err();
    assert!(matches!(
        err,
        LedgerError::ReplayIdentityMismatch {
            side: ExposureSide::Racc,
            ..
        }
    ));
}

#[test]
fn empty_exposure_account_is_rejected() {
    let acct = ExposureAccount::default();
    assert_eq!(acct.raw_cost(), Err(LedgerError::EmptyExposureAccount));
    assert_eq!(
        acct.saving_ppm_floor(),
        Err(LedgerError::EmptyExposureAccount)
    );
}

#[test]
fn saving_ppm_floor_never_overstates() {
    // C_raw = 1000, C_tz = 30 -> 97% saving exactly.
    let acct = account(0, 0, &[1000], &[1], &[0]);
    assert_eq!(acct.saving_ppm_floor().unwrap(), PPM_ONE);
    let acct = account(0, 30, &[1000], &[1], &[0]);
    assert_eq!(acct.saving_ppm_floor().unwrap(), 970_000);
    // A RACC path that costs more than raw reports zero saving, never negative.
    let acct = account(0, 0, &[1000], &[1], &[3]);
    assert_eq!(acct.saving_ppm_floor().unwrap(), 0);
}

#[test]
fn weighted_exposures_exclude_fixed_overhead() {
    let acct = account(10, 3, &[10, 20, 30], &[5, 2, 1], &[1, 0, 1]);
    assert_eq!(acct.weighted_raw_exposure(), 10 * 5 + 20 * 2 + 30);
    assert_eq!(acct.weighted_racc_exposure(), 10 + 30);
    assert_eq!(acct.block_tokens_total(), 60);
}

// --- canonical JSON round trip --------------------------------------------

#[test]
fn receipt_round_trips_through_canonical_json() {
    let receipt = receipt_with(1_000_000, 30_000, 30_000);
    let json = receipt.to_canonical_json().unwrap();
    let decoded: DominanceReceipt = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, receipt);
    assert_eq!(decoded.to_canonical_json().unwrap(), json);
}

#[test]
fn canonical_json_has_deterministic_key_order() {
    let receipt = receipt_with(1_000_000, 30_000, 30_000);
    let json = receipt.to_canonical_json().unwrap();
    assert!(
        json.starts_with("{\"archive_root\":"),
        "unexpected encoding: {json}"
    );
    assert!(json.contains("\"target_retained_ppm\":30000"));
    assert!(json.contains("\"tokenizer_id\":\"cl100k_base\""));
    assert!(
        !json.contains('%'),
        "no percentage strings in the wire schema"
    );
}

#[test]
fn canonical_digest_is_stable_and_content_sensitive() {
    let receipt = receipt_with(1_000_000, 30_000, 30_000);
    let first = receipt.canonical_digest_hex().unwrap();
    assert_eq!(first, receipt.clone().canonical_digest_hex().unwrap());
    assert_eq!(first.len(), 64);

    let other = receipt_with(1_000_000, 30_001, 30_000);
    assert_ne!(first, other.canonical_digest_hex().unwrap());
}

#[test]
fn digest_hex_round_trips() {
    let digest = d(0xab);
    assert_eq!(digest.to_hex().len(), 64);
    assert_eq!(Digest::from_hex(&digest.to_hex()), Some(digest));
    assert_eq!(Digest::from_hex("nope"), None);
    assert_eq!(Digest::from_hex(&"g".repeat(64)), None);
}

// --- properties -----------------------------------------------------------

proptest! {
    #[test]
    fn counters_are_monotone_under_arbitrary_interleavings(
        charges in prop::collection::vec(
            (0u64..1_000_000, 0u64..1_000_000, 0u64..1_000, 0u64..1_000, 0u64..1_000),
            0..64usize,
        ),
    ) {
        let mut g = gauge();
        let mut previous = g.ledger().clone();
        for (raw, racc, recovery, reexpansion, fallback) in charges {
            g.charge(&tokenizer(), &TokenCharge {
                raw_input_tokens: raw,
                racc_input_tokens: racc,
                recovery_tokens: recovery,
                reexpansion_tokens: reexpansion,
                fallback_tokens: fallback,
                model_calls: 1,
                ..Default::default()
            }).unwrap();
            let now = g.ledger();
            prop_assert!(now.raw_input_tokens >= previous.raw_input_tokens);
            prop_assert!(now.racc_input_tokens >= previous.racc_input_tokens);
            prop_assert!(now.recovery_tokens >= previous.recovery_tokens);
            prop_assert!(now.reexpansion_tokens >= previous.reexpansion_tokens);
            prop_assert!(now.fallback_tokens >= previous.fallback_tokens);
            prop_assert!(now.model_calls >= previous.model_calls);
            previous = now.clone();
        }
    }

    #[test]
    fn charge_order_does_not_change_totals(
        charges in prop::collection::vec((0u64..1_000_000, 0u64..1_000_000), 0..32usize),
    ) {
        let build = |items: &[(u64, u64)]| {
            let mut g = gauge();
            for &(raw, racc) in items {
                g.charge(&tokenizer(), &TokenCharge {
                    raw_input_tokens: raw,
                    racc_input_tokens: racc,
                    ..Default::default()
                }).unwrap();
            }
            g.ledger().clone()
        };
        let forward = build(&charges);
        let mut reversed = charges.clone();
        reversed.reverse();
        prop_assert_eq!(forward, build(&reversed));
    }

    #[test]
    fn meets_token_target_never_overflows(
        raw in any::<u64>(),
        racc in any::<u64>(),
        ppm in 0u32..=PPM_ONE,
    ) {
        let receipt = receipt_with(raw, racc, ppm);
        let expected = u128::from(racc) * u128::from(PPM_ONE) <= u128::from(raw) * u128::from(ppm);
        prop_assert_eq!(receipt.meets_token_target(), expected);
    }

    #[test]
    fn achieved_ppm_ceiling_certifies_its_own_target(
        raw in 1u64..=u64::MAX,
        racc in any::<u64>(),
    ) {
        let achieved = receipt_with(raw, racc, 0).achieved_retained_ppm_ceil().unwrap();
        prop_assume!(achieved <= u128::from(PPM_ONE));
        let ppm = u32::try_from(achieved).unwrap();
        prop_assert!(receipt_with(raw, racc, ppm).meets_token_target());
        if ppm > 0 {
            let tighter = receipt_with(raw, racc, ppm - 1);
            prop_assert!(!tighter.meets_token_target() || racc == 0);
        }
    }

    #[test]
    fn replay_identity_round_trips_for_any_account(
        blocks in prop::collection::vec((1u64..10_000, 0u64..64, 0u64..64), 1..16usize),
        h_raw in 0u64..10_000,
        h_tz in 0u64..10_000,
    ) {
        let acct = ExposureAccount {
            raw_fixed_overhead: h_raw,
            racc_fixed_overhead: h_tz,
            blocks: blocks
                .iter()
                .map(|&(block_tokens, raw_exposures, racc_exposures)| ExposureBlock {
                    block_tokens,
                    raw_exposures,
                    racc_exposures,
                })
                .collect(),
        };
        let mut ledger = TokenLedger::empty(tokenizer());
        ledger.raw_input_tokens = u64::try_from(acct.raw_cost().unwrap()).unwrap();
        ledger.racc_input_tokens = u64::try_from(acct.racc_cost().unwrap()).unwrap();
        prop_assert!(acct.check_replay_identity(&ledger).is_ok());

        ledger.recovery_tokens += 1;
        prop_assert!(acct.check_replay_identity(&ledger).is_err());
    }

    #[test]
    fn canonical_json_round_trips_for_any_receipt(
        raw in any::<u64>(),
        racc in any::<u64>(),
        ppm in 0u32..=PPM_ONE,
    ) {
        let receipt = receipt_with(raw, racc, ppm);
        let json = receipt.to_canonical_json().unwrap();
        let decoded: DominanceReceipt = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(&decoded, &receipt);
        prop_assert_eq!(decoded.canonical_digest_hex().unwrap(), receipt.canonical_digest_hex().unwrap());
    }
}
