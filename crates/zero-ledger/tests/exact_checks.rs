//! Ledger/receipt subset of the RACC exact-check suite, ported to Rust.
//!
//! Mirrors docs/racc/RACC_CONTRACT.rs unit tests and the ledger-relevant cases
//! of docs/racc/RACC_EXACT_CHECKS.py (test_replay_exposure_identity and the
//! phase-target arithmetic), plus the accounting-completeness and anti-forgery
//! hardening required by the spec review.

use proptest::prelude::*;
use zero_ledger::{
    ArchiveAttestation, ChargeClass, Digest, DominanceReceipt, EvidenceError, ExactnessGates,
    ExposureAccount, ExposureBlock, ExposureSide, LedgerConfig, LedgerError, PolicyDecision,
    PolicyEvidence, ReceiptError, ReceiptRoots, ResourceGauge, RetainedFractionPpm,
    TaskAcceptanceReceipt, TaskOutcome, TokenCharge, TokenLedger, TokenizerIdentity, PPM_ONE,
    RECEIPT_SCHEMA_VERSION,
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

fn spans() -> Vec<Digest> {
    vec![d(0x11), d(0x12), d(0x13)]
}

fn decisions() -> Vec<PolicyDecision> {
    vec![
        PolicyDecision::SufficiencyProven {
            witness_digest: d(0x21),
        },
        PolicyDecision::RawFallbackServed {
            view_digest: d(0x22),
        },
    ]
}

fn outcomes() -> Vec<TaskOutcome> {
    vec![TaskOutcome::Accepted {
        task_digest: d(0x31),
    }]
}

fn archive() -> ArchiveAttestation {
    ArchiveAttestation::verify(ArchiveAttestation::root_of(&spans()), &spans()).unwrap()
}

fn policy() -> PolicyEvidence {
    PolicyEvidence::verify(PolicyEvidence::root_of(&decisions()), &decisions()).unwrap()
}

fn task() -> TaskAcceptanceReceipt {
    TaskAcceptanceReceipt::verify(TaskAcceptanceReceipt::root_of(&outcomes()), &outcomes()).unwrap()
}

fn all_gates() -> ExactnessGates {
    ExactnessGates::default()
        .with_byte_exact(&archive())
        .with_policy_exact_or_fallback(&policy())
        .with_task_verified(&task())
}

fn roots() -> ReceiptRoots {
    ReceiptRoots {
        archive_root: archive().archive_root(),
        certificate_root: policy().certificate_root(),
    }
}

fn ppm(value: u32) -> RetainedFractionPpm {
    RetainedFractionPpm::new(value).unwrap()
}

/// A ledger whose whole RACC exposure sits in one charge class.
fn ledger_in_class(raw: u64, racc: u64, class: ChargeClass) -> TokenLedger {
    let mut ledger = TokenLedger::empty(tokenizer());
    ledger.raw_input_tokens = raw;
    ledger.declared_input_tokens = racc;
    ledger.model_calls = 1;
    match class {
        ChargeClass::Billed => ledger.billed_tokens = racc,
        ChargeClass::FailedTrial => ledger.failed_trial_tokens = racc,
        ChargeClass::Retry => ledger.retry_tokens = racc,
        ChargeClass::Recovery => ledger.recovery_tokens = racc,
        ChargeClass::Reexpansion => ledger.reexpansion_tokens = racc,
        ChargeClass::Fallback => ledger.fallback_tokens = racc,
    }
    ledger
}

/// Zeroes one charge class, simulating a cost hidden from the receipt sum.
fn hide_class(ledger: &mut TokenLedger, class: ChargeClass) {
    match class {
        ChargeClass::Billed => ledger.billed_tokens = 0,
        ChargeClass::FailedTrial => ledger.failed_trial_tokens = 0,
        ChargeClass::Retry => ledger.retry_tokens = 0,
        ChargeClass::Recovery => ledger.recovery_tokens = 0,
        ChargeClass::Reexpansion => ledger.reexpansion_tokens = 0,
        ChargeClass::Fallback => ledger.fallback_tokens = 0,
    }
}

fn receipt_with(raw: u64, racc: u64, target_ppm: u32) -> DominanceReceipt {
    DominanceReceipt::seal(
        ledger_in_class(raw, racc, ChargeClass::Billed),
        ppm(target_ppm),
        roots(),
        all_gates(),
    )
    .expect("sealable receipt")
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
    let ledger = ledger_in_class(1_000_000, 30_000, ChargeClass::Billed);
    let partial = [
        ExactnessGates::default()
            .with_policy_exact_or_fallback(&policy())
            .with_task_verified(&task()),
        ExactnessGates::default()
            .with_byte_exact(&archive())
            .with_task_verified(&task()),
        ExactnessGates::default()
            .with_byte_exact(&archive())
            .with_policy_exact_or_fallback(&policy()),
    ];
    for (i, gates) in partial.into_iter().enumerate() {
        let receipt = DominanceReceipt::seal(ledger.clone(), ppm(30_000), roots(), gates).unwrap();
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
fn zero_raw_baseline_cannot_certify_a_phase() {
    // R = 0 is not a phase: sealing is refused outright.
    assert_eq!(
        DominanceReceipt::seal(
            ledger_in_class(0, 0, ChargeClass::Billed),
            ppm(PPM_ONE),
            roots(),
            all_gates(),
        ),
        Err(ReceiptError::IncompleteLedger)
    );

    // And a receipt mutated to R = 0 on the wire fails both predicates.
    let json = receipt_with(1_000_000, 0, PPM_ONE)
        .to_canonical_json()
        .unwrap();
    let forged: DominanceReceipt = serde_json::from_str(
        &json.replace("\"raw_input_tokens\":1000000", "\"raw_input_tokens\":0"),
    )
    .unwrap();
    assert_eq!(forged.ledger.raw_input_tokens, 0);
    assert!(!forged.meets_token_target());
    assert!(!forged.exact_phase_valid());
    assert_eq!(forged.achieved_retained_ppm_ceil(), None);
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

// --- retained fraction range validation -----------------------------------

#[test]
fn retained_fraction_rejects_out_of_range_at_construction() {
    assert_eq!(RetainedFractionPpm::new(0).unwrap().ppm(), 0);
    assert_eq!(RetainedFractionPpm::new(PPM_ONE).unwrap().ppm(), PPM_ONE);
    assert_eq!(
        RetainedFractionPpm::new(PPM_ONE + 1),
        Err(LedgerError::PpmOutOfRange { ppm: PPM_ONE + 1 })
    );
    assert_eq!(
        RetainedFractionPpm::new(u32::MAX),
        Err(LedgerError::PpmOutOfRange { ppm: u32::MAX })
    );
}

#[test]
fn retained_fraction_rejects_out_of_range_on_the_wire() {
    assert_eq!(
        serde_json::from_str::<RetainedFractionPpm>("1000000")
            .unwrap()
            .ppm(),
        PPM_ONE
    );
    assert!(serde_json::from_str::<RetainedFractionPpm>("1000001").is_err());
    assert!(serde_json::from_str::<RetainedFractionPpm>("4294967295").is_err());
}

#[test]
fn receipt_deserialization_rejects_out_of_range_target() {
    let json = receipt_with(1_000_000, 30_000, 30_000)
        .to_canonical_json()
        .unwrap();
    let forged = json.replace(
        "\"target_retained_ppm\":30000",
        "\"target_retained_ppm\":1000001",
    );
    assert_ne!(forged, json);
    assert!(serde_json::from_str::<DominanceReceipt>(&forged).is_err());
}

// --- accounting completeness ----------------------------------------------

#[test]
fn every_charge_class_enters_the_receipt_sum() {
    for class in ChargeClass::ALL {
        let ledger = ledger_in_class(1_000_000, 30_000, class);
        assert_eq!(
            ledger.racc_input_tokens().unwrap(),
            30_000,
            "{class} must be summed into the RACC exposure"
        );
        let receipt = DominanceReceipt::seal(ledger, ppm(30_000), roots(), all_gates()).unwrap();
        assert_eq!(receipt.racc_input_tokens, 30_000);
        assert!(receipt.exact_phase_valid());
    }
}

/// One omission mutation per charge class: hiding the class must fail the seal.
#[test]
fn hiding_any_charge_class_fails_finalization() {
    for class in ChargeClass::ALL {
        // The exposure blows the target, so hiding it would otherwise buy a pass.
        let mut ledger = ledger_in_class(1_000_000, 900_000, class);
        let honest =
            DominanceReceipt::seal(ledger.clone(), ppm(30_000), roots(), all_gates()).unwrap();
        assert!(
            !honest.meets_token_target(),
            "{class}: honest accounting must blow the target"
        );

        hide_class(&mut ledger, class);
        assert_eq!(
            ledger.racc_input_tokens().unwrap(),
            0,
            "{class}: the mutation really hides the cost"
        );
        assert_eq!(
            DominanceReceipt::seal(ledger.clone(), ppm(30_000), roots(), all_gates()),
            Err(ReceiptError::Accounting(LedgerError::UnclassifiedInput {
                declared: 900_000,
                classified: 0,
            })),
            "{class}: hiding the class must fail finalization"
        );
    }
}

/// The same omission must also break the T8 replay identity, not just the seal.
#[test]
fn hiding_any_charge_class_fails_the_replay_identity() {
    for class in ChargeClass::ALL {
        let acct = account(10, 40, &[10, 20, 30], &[5, 2, 1], &[1, 0, 1]);
        let racc = u64::try_from(acct.racc_cost().unwrap()).unwrap();
        let raw = u64::try_from(acct.raw_cost().unwrap()).unwrap();
        let mut ledger = ledger_in_class(raw, racc, class);
        acct.check_replay_identity(&ledger)
            .unwrap_or_else(|e| panic!("{class}: honest ledger should reconcile: {e}"));

        hide_class(&mut ledger, class);
        assert_eq!(
            acct.check_replay_identity(&ledger),
            Err(LedgerError::UnclassifiedInput {
                declared: racc,
                classified: 0,
            }),
            "{class}: hidden cost must break eq (5.7)"
        );
    }
}

#[test]
fn double_counted_input_is_rejected() {
    let mut ledger = ledger_in_class(1_000_000, 30_000, ChargeClass::Billed);
    // The same call is charged again as a retry.
    ledger.retry_tokens = 30_000;
    assert_eq!(
        ledger.check_accounting_complete(),
        Err(LedgerError::DoubleCountedInput {
            declared: 30_000,
            classified: 60_000,
        })
    );
    assert_eq!(
        DominanceReceipt::seal(ledger, ppm(30_000), roots(), all_gates()),
        Err(ReceiptError::Accounting(LedgerError::DoubleCountedInput {
            declared: 30_000,
            classified: 60_000,
        }))
    );
}

#[test]
fn charge_rejects_unclassified_and_double_counted_input() {
    let mut g = gauge();
    let err = g
        .charge(
            &tokenizer(),
            &TokenCharge {
                raw_input_tokens: 100,
                input_tokens: 10,
                billed_tokens: 4,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(
        err,
        LedgerError::UnclassifiedInput {
            declared: 10,
            classified: 4
        }
    );
    assert_eq!(g.charge_count(), 0, "a rejected charge leaves no trace");
    assert_eq!(g.ledger().raw_input_tokens, 0);

    let err = g
        .charge(
            &tokenizer(),
            &TokenCharge {
                input_tokens: 10,
                billed_tokens: 10,
                retry_tokens: 1,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(
        err,
        LedgerError::DoubleCountedInput {
            declared: 10,
            classified: 11
        }
    );
    assert_eq!(g.charge_count(), 0);
}

#[test]
fn gauge_accumulates_every_class_into_the_receipt_total() {
    let mut g = gauge();
    g.charge(
        &tokenizer(),
        &TokenCharge {
            raw_input_tokens: 1_000_000,
            input_tokens: 60,
            billed_tokens: 10,
            failed_trial_tokens: 10,
            retry_tokens: 10,
            recovery_tokens: 10,
            reexpansion_tokens: 10,
            fallback_tokens: 10,
            model_calls: 1,
            retries: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let receipt = g
        .finalize_receipt(ppm(30_000), roots(), all_gates())
        .unwrap();
    assert_eq!(receipt.racc_input_tokens, 60);
    assert_eq!(receipt.ledger.declared_input_tokens, 60);
    assert!(receipt.exact_phase_valid());
}

// --- anti-forgery of the exactness gates ----------------------------------

#[test]
fn gates_default_to_false_without_evidence() {
    let gates = ExactnessGates::default();
    assert!(!gates.byte_exact());
    assert!(!gates.policy_exact_or_fallback());
    assert!(!gates.task_verified());

    let receipt = DominanceReceipt::seal(
        ledger_in_class(1_000_000, 30_000, ChargeClass::Billed),
        ppm(30_000),
        ReceiptRoots::default(),
        gates,
    )
    .unwrap();
    assert!(receipt.meets_token_target());
    assert!(!receipt.exact_phase_valid());
}

#[test]
fn evidence_handles_reject_wrong_or_missing_evidence() {
    assert_eq!(
        ArchiveAttestation::verify(d(0), &[]),
        Err(EvidenceError::NoEvidence { kind: "archive" })
    );
    assert!(matches!(
        ArchiveAttestation::verify(d(0), &spans()),
        Err(EvidenceError::RootMismatch {
            kind: "archive",
            ..
        })
    ));
    // Dropping one retained span changes the root, so it cannot be attested.
    assert!(matches!(
        ArchiveAttestation::verify(ArchiveAttestation::root_of(&spans()), &spans()[..2]),
        Err(EvidenceError::RootMismatch { .. })
    ));

    assert_eq!(
        PolicyEvidence::verify(d(0), &[]),
        Err(EvidenceError::NoEvidence { kind: "policy" })
    );
    assert!(matches!(
        PolicyEvidence::verify(d(0), &decisions()),
        Err(EvidenceError::RootMismatch { kind: "policy", .. })
    ));

    assert_eq!(
        TaskAcceptanceReceipt::verify(d(0), &[]),
        Err(EvidenceError::NoEvidence { kind: "task" })
    );
    let regressed = vec![TaskOutcome::Regressed {
        task_digest: d(0x31),
    }];
    assert_eq!(
        TaskAcceptanceReceipt::verify(TaskAcceptanceReceipt::root_of(&regressed), &regressed),
        Err(EvidenceError::TaskRegressed)
    );
}

#[test]
fn policy_and_task_evidence_are_not_interchangeable() {
    // Policy-sufficiency receipts (8.2) and T13 task-no-regret receipts (8.3)
    // commit under different domain tags, so one can never stand in for the other.
    let policy_root = PolicyEvidence::root_of(&[PolicyDecision::SufficiencyProven {
        witness_digest: d(0x41),
    }]);
    let task_root = TaskAcceptanceReceipt::root_of(&[TaskOutcome::Accepted {
        task_digest: d(0x41),
    }]);
    assert_ne!(policy_root, task_root);
    assert!(TaskAcceptanceReceipt::verify(
        policy_root,
        &[TaskOutcome::Accepted {
            task_digest: d(0x41)
        }]
    )
    .is_err());
}

#[test]
fn gate_evidence_must_match_the_receipt_roots() {
    let mismatched = ReceiptRoots {
        archive_root: d(0xee),
        certificate_root: policy().certificate_root(),
    };
    assert!(matches!(
        DominanceReceipt::seal(
            ledger_in_class(1_000_000, 30_000, ChargeClass::Billed),
            ppm(30_000),
            mismatched,
            all_gates(),
        ),
        Err(ReceiptError::EvidenceRootMismatch {
            kind: "archive",
            ..
        })
    ));

    let mismatched = ReceiptRoots {
        archive_root: archive().archive_root(),
        certificate_root: d(0xee),
    };
    assert!(matches!(
        DominanceReceipt::seal(
            ledger_in_class(1_000_000, 30_000, ChargeClass::Billed),
            ppm(30_000),
            mismatched,
            all_gates(),
        ),
        Err(ReceiptError::EvidenceRootMismatch { kind: "policy", .. })
    ));
}

// --- locked tokenizer gauge -----------------------------------------------

#[test]
fn mixed_tokenizer_identity_is_rejected() {
    let mut g = gauge();
    let charge = TokenCharge {
        raw_input_tokens: 100,
        input_tokens: 3,
        billed_tokens: 3,
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
fn same_tokenizer_name_with_a_different_digest_is_rejected() {
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
fn a_failed_charge_applies_no_field_at_all() {
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

#[test]
fn finalize_rejects_an_empty_gauge() {
    let g = gauge();
    assert_eq!(
        g.finalize_receipt(ppm(30_000), roots(), all_gates()),
        Err(ReceiptError::IncompleteLedger)
    );
}

#[test]
fn finalize_rejects_a_zero_raw_baseline() {
    let mut g = gauge();
    g.charge(
        &tokenizer(),
        &TokenCharge {
            input_tokens: 10,
            billed_tokens: 10,
            model_calls: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        g.finalize_receipt(ppm(30_000), roots(), all_gates()),
        Err(ReceiptError::IncompleteLedger)
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
                input_tokens: 2_000,
                billed_tokens: 2_000,
                model_calls: 1,
                ..Default::default()
            },
        )
        .unwrap();
    }
    let receipt = g
        .finalize_receipt(ppm(30_000), roots(), all_gates())
        .unwrap();
    assert_eq!(receipt.ledger.raw_input_tokens, 1_000_000);
    assert_eq!(receipt.racc_input_tokens, 20_000);
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
        // Split the RACC side across every charge class to prove the identity
        // sums every exposure channel.
        let racc = u64::try_from(acct.racc_cost().unwrap()).unwrap();
        ledger.declared_input_tokens = racc;
        let share = racc / 6;
        ledger.billed_tokens = share;
        ledger.failed_trial_tokens = share;
        ledger.retry_tokens = share;
        ledger.recovery_tokens = share;
        ledger.reexpansion_tokens = share;
        ledger.fallback_tokens = racc - 5 * share;
        acct.check_replay_identity(&ledger)
            .unwrap_or_else(|e| panic!("case {j}: {e}"));
    }
}

#[test]
fn replay_identity_detects_raw_side_drift() {
    let acct = account(10, 3, &[10, 20, 30], &[5, 2, 1], &[1, 0, 1]);
    let racc = u64::try_from(acct.racc_cost().unwrap()).unwrap();
    let raw = u64::try_from(acct.raw_cost().unwrap()).unwrap();
    let ledger = ledger_in_class(raw + 1, racc, ChargeClass::Billed);
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
    let racc = u64::try_from(acct.racc_cost().unwrap()).unwrap();
    let raw = u64::try_from(acct.raw_cost().unwrap()).unwrap();
    // Consistently declaring one token less understates C_tz and must be caught.
    let ledger = ledger_in_class(raw, racc - 1, ChargeClass::Billed);
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
fn saving_ppm_is_floor_rounded_and_never_negative() {
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
    assert!(json.contains(&format!("\"schema_version\":{RECEIPT_SCHEMA_VERSION}")));
    assert!(json.contains("\"racc_input_tokens\":30000"));
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
            (0u64..1_000_000, 0u64..1_000, 0u64..1_000, 0u64..1_000, 0u64..1_000),
            0..64usize,
        ),
    ) {
        let mut g = gauge();
        let mut previous = g.ledger().clone();
        for (raw, billed, recovery, reexpansion, fallback) in charges {
            g.charge(&tokenizer(), &TokenCharge {
                raw_input_tokens: raw,
                input_tokens: billed + recovery + reexpansion + fallback,
                billed_tokens: billed,
                recovery_tokens: recovery,
                reexpansion_tokens: reexpansion,
                fallback_tokens: fallback,
                model_calls: 1,
                ..Default::default()
            }).unwrap();
            let now = g.ledger();
            prop_assert!(now.raw_input_tokens >= previous.raw_input_tokens);
            prop_assert!(now.declared_input_tokens >= previous.declared_input_tokens);
            prop_assert!(now.billed_tokens >= previous.billed_tokens);
            prop_assert!(now.recovery_tokens >= previous.recovery_tokens);
            prop_assert!(now.reexpansion_tokens >= previous.reexpansion_tokens);
            prop_assert!(now.fallback_tokens >= previous.fallback_tokens);
            prop_assert!(now.model_calls >= previous.model_calls);
            prop_assert_eq!(now.racc_input_tokens().unwrap(), now.declared_input_tokens);
            previous = now.clone();
        }
    }

    #[test]
    fn charge_order_does_not_change_totals(
        charges in prop::collection::vec((0u64..1_000_000, 0u64..1_000_000), 0..32usize),
    ) {
        let build = |items: &[(u64, u64)]| {
            let mut g = gauge();
            for &(raw, billed) in items {
                g.charge(&tokenizer(), &TokenCharge {
                    raw_input_tokens: raw,
                    input_tokens: billed,
                    billed_tokens: billed,
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
        raw in 1u64..=u64::MAX,
        racc in any::<u64>(),
        target in 0u32..=PPM_ONE,
    ) {
        let receipt = receipt_with(raw, racc, target);
        let expected = u128::from(racc) * u128::from(PPM_ONE) <= u128::from(raw) * u128::from(target);
        prop_assert_eq!(receipt.meets_token_target(), expected);
    }

    #[test]
    fn achieved_ppm_is_the_tightest_passing_target(
        raw in 1u64..1_000_000_000,
        racc in 0u64..1_000_000_000,
    ) {
        let achieved = receipt_with(raw, racc, PPM_ONE).achieved_retained_ppm_ceil().unwrap();
        prop_assume!(achieved <= u128::from(PPM_ONE));
        let target = u32::try_from(achieved).unwrap();
        prop_assert!(receipt_with(raw, racc, target).meets_token_target());
        if target > 0 {
            let tighter = receipt_with(raw, racc, target - 1);
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
        let raw = u64::try_from(acct.raw_cost().unwrap()).unwrap();
        let racc = u64::try_from(acct.racc_cost().unwrap()).unwrap();
        let mut ledger = ledger_in_class(raw, racc, ChargeClass::Billed);
        prop_assert!(acct.check_replay_identity(&ledger).is_ok());

        // An extra recovery token that was never declared is double counting.
        ledger.recovery_tokens += 1;
        prop_assert!(acct.check_replay_identity(&ledger).is_err());
    }

    #[test]
    fn canonical_json_round_trips_for_any_receipt(
        raw in 1u64..=u64::MAX,
        racc in any::<u64>(),
        target in 0u32..=PPM_ONE,
    ) {
        let receipt = receipt_with(raw, racc, target);
        let json = receipt.to_canonical_json().unwrap();
        let decoded: DominanceReceipt = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(&decoded, &receipt);
        prop_assert_eq!(decoded.canonical_digest_hex().unwrap(), receipt.canonical_digest_hex().unwrap());
    }
}
