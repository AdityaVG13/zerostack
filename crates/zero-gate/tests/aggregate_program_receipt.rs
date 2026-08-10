//! Exact proof fixtures for the truthful aggregate Program receipt.
//!
//! The valid fixture locks the canonical bytes of a sealed
//! `AggregateProgramReceiptV1` (receipt_head recomputed over the canonical
//! body). Every mutant fixture must fail `verify` with the documented typed
//! failure code — a missing engine or surface can never report Program
//! success.

use zero_gate::{
    AggregateProgramFailureCodeV1, AggregateProgramReceiptV1, aggregate_program_contract_digest_v1,
};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{FIXTURES}/{name}"))
        .unwrap_or_else(|error| panic!("missing fixture {name}: {error}"))
}

#[test]
fn valid_fixture_verifies_and_round_trips() {
    let bytes = fixture("aggregate_program_receipt_v1_valid.json");
    let receipt = AggregateProgramReceiptV1::from_canonical_bytes(&bytes).unwrap();
    receipt.verify().unwrap();
    // Exact proof: the sealed canonical bytes are stable.
    assert_eq!(receipt.canonical_bytes().unwrap(), bytes);
}

#[test]
fn missing_engine_fixture_can_never_report_program_success() {
    let bytes = fixture("aggregate_program_receipt_v1_missing_engine.json");
    let receipt = AggregateProgramReceiptV1::from_canonical_bytes(&bytes).unwrap();
    assert_eq!(
        receipt.verify().unwrap_err().failure_code(),
        AggregateProgramFailureCodeV1::MissingEngine
    );
}

#[test]
fn missing_surface_fixture_can_never_report_program_success() {
    let bytes = fixture("aggregate_program_receipt_v1_missing_surface.json");
    let receipt = AggregateProgramReceiptV1::from_canonical_bytes(&bytes).unwrap();
    assert_eq!(
        receipt.verify().unwrap_err().failure_code(),
        AggregateProgramFailureCodeV1::MissingEvidenceClass
    );
}

#[test]
fn duplicate_slot_fixture_can_never_report_program_success() {
    let bytes = fixture("aggregate_program_receipt_v1_duplicate_slot.json");
    let receipt = AggregateProgramReceiptV1::from_canonical_bytes(&bytes).unwrap();
    assert_eq!(
        receipt.verify().unwrap_err().failure_code(),
        AggregateProgramFailureCodeV1::DuplicateEvidenceSlot
    );
}

#[test]
fn unknown_engine_fixture_can_never_report_program_success() {
    let bytes = fixture("aggregate_program_receipt_v1_unknown_engine.json");
    let receipt = AggregateProgramReceiptV1::from_canonical_bytes(&bytes).unwrap();
    assert_eq!(
        receipt.verify().unwrap_err().failure_code(),
        AggregateProgramFailureCodeV1::UnknownEngine
    );
}

#[test]
fn forged_head_fixture_can_never_report_program_success() {
    let bytes = fixture("aggregate_program_receipt_v1_forged_head.json");
    let receipt = AggregateProgramReceiptV1::from_canonical_bytes(&bytes).unwrap();
    assert_eq!(
        receipt.verify().unwrap_err().failure_code(),
        AggregateProgramFailureCodeV1::ReceiptHeadMismatch
    );
}

#[test]
fn contract_digest_is_nonzero() {
    let digest = aggregate_program_contract_digest_v1();
    assert_ne!(digest, zero_abi::DigestV1::ZERO);
}
