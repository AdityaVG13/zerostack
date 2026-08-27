//! Certify primitive: re-measurement must be deterministic, catch tampered
//! bytes, and distinguish the BPE path from the lexical estimate path.

use tokenzero_kernel::ZeroTokenEngine;
use zero_abi::{CompressionRequest, EngineInvocation, ExpandOptions, TokenAccounting, TokenEngine};
use zerostack_conformance::token_engine;
use zerostack_test_support::{TempWorkspace, test_invocation};

fn workspace() -> TempWorkspace {
    TempWorkspace::new("tz-certify").unwrap()
}

/// Engine bound to the shared hub scaffolding; store lives in the hermetic
/// workspace so tests stay isolated without hand-built invocations.
fn engine(ws: &TempWorkspace) -> ZeroTokenEngine {
    ZeroTokenEngine::open(ws.store(), None)
}

fn invocation_for(ws: &TempWorkspace) -> EngineInvocation {
    test_invocation(ws.root(), "certify-test", "cell-1")
}

#[test]
fn certify_is_deterministic_and_matches_fresh_measurement() {
    let ws = workspace();
    let invocation = invocation_for(&ws);
    let engine = engine(&ws);
    let bytes = b"the quick brown fox jumps over the lazy dog";
    let claimed = engine.measure(&invocation, bytes).unwrap();
    let result = engine.certify(&invocation, bytes, &claimed).unwrap();
    assert!(result.matches, "identical bytes must match the claim");
    assert_eq!(result.recomputed, claimed);
}

#[test]
fn certify_detects_tampered_bytes() {
    let ws = workspace();
    let invocation = invocation_for(&ws);
    let engine = engine(&ws);
    let original = b"alpha beta gamma delta epsilon zeta eta theta";
    let claimed = engine.measure(&invocation, original).unwrap();

    // Appending tokens must change the lexical count, so the claim for the
    // shorter text cannot match the tampered bytes.
    let tampered = b"alpha beta gamma delta epsilon zeta eta theta plus five more words";
    let result = engine.certify(&invocation, tampered, &claimed).unwrap();
    assert!(!result.matches, "tampered bytes must not match the claim");
}

#[test]
fn certify_rejects_mismatched_tokenizer_claim() {
    let ws = workspace();
    let invocation = invocation_for(&ws);
    let engine = engine(&ws);
    let bytes = b"determinism probe for tokenizer identity";
    let mut forged = engine.measure(&invocation, bytes).unwrap();
    forged.tokenizer = "forged-tokenizer".into();
    let result = engine.certify(&invocation, bytes, &forged).unwrap();
    assert!(!result.matches, "tokenizer identity is part of the claim");
    assert_ne!(result.recomputed.tokenizer, "forged-tokenizer");
}

#[test]
fn tokenzero_engine_conforms_to_shared_contract() {
    let ws = workspace();
    let invocation = invocation_for(&ws);
    let engine = engine(&ws);
    let result = token_engine::run_all(&engine, &invocation);
    result.require_clean("TokenZero");
}

#[test]
fn compression_hard_caps_visible_tokens_and_round_trips_exactly() {
    let ws = workspace();
    let invocation = invocation_for(&ws);
    let engine = engine(&ws);
    let input = "repeated exact line for compression\n".repeat(50);
    for budget in [8, 16, 30, 64] {
        let result = engine
            .compress(
                &invocation,
                CompressionRequest {
                    bytes: input.as_bytes().to_vec(),
                    max_tokens: budget,
                    mode: String::new(),
                    label: None,
                    media_type: "text/plain; charset=utf-8".into(),
                },
            )
            .unwrap();
        assert!(
            result.accounting.visible <= u64::from(budget),
            "visible={} budget={budget}",
            result.accounting.visible
        );
        if budget == 8 {
            assert!(
                result.truncated,
                "an eight-token envelope must require truncation"
            );
        }
        assert!(result.omitted_tokens > 0);
        let expanded = engine
            .expand(&invocation, &result.exact, ExpandOptions::default())
            .unwrap();
        assert_eq!(expanded, input.as_bytes());
    }
}

#[test]
fn repeated_input_saves_more_than_half_the_visible_tokens() {
    let ws = workspace();
    let invocation = invocation_for(&ws);
    let engine = engine(&ws);
    let input = "same line same line same line\n".repeat(50);
    let measured = engine.measure(&invocation, input.as_bytes()).unwrap();
    let result = engine
        .compress(
            &invocation,
            CompressionRequest {
                bytes: input.as_bytes().to_vec(),
                max_tokens: 64,
                mode: String::new(),
                label: None,
                media_type: "text/plain; charset=utf-8".into(),
            },
        )
        .unwrap();
    assert!(result.accounting.visible * 2 < measured.billed);
}

fn compress(
    engine: &ZeroTokenEngine,
    invocation: &EngineInvocation,
    input: &str,
    budget: u32,
    mode: &str,
) -> zero_abi::CompressionResult {
    engine
        .compress(
            invocation,
            CompressionRequest {
                bytes: input.as_bytes().to_vec(),
                max_tokens: budget,
                mode: mode.to_string(),
                label: None,
                media_type: "text/plain; charset=utf-8".into(),
            },
        )
        .unwrap()
}

#[test]
fn auto_oversized_visible_is_budget_monotonic_and_round_trips() {
    let ws = workspace();
    let invocation = invocation_for(&ws);
    let engine = engine(&ws);
    let input = "repeated exact line for compression\n".repeat(50);
    let mut previous_visible = 0u64;
    let mut previous_budget = 0u32;
    for budget in [8, 16, 32, 64, 128, 256, 512, 1024] {
        let result = compress(&engine, &invocation, &input, budget, "");
        assert!(
            result.accounting.visible <= u64::from(budget),
            "visible={} budget={budget}",
            result.accounting.visible
        );
        let expanded = engine
            .expand(&invocation, &result.exact, ExpandOptions::default())
            .unwrap();
        assert_eq!(expanded, input.as_bytes(), "budget={budget}");
        if previous_budget > 0 {
            assert!(
                result.accounting.visible >= previous_visible,
                "tighter budget {previous_budget} visible={previous_visible} vs looser {budget} visible={}",
                result.accounting.visible
            );
        }
        previous_visible = result.accounting.visible;
        previous_budget = budget;
    }
}

#[test]
fn compress_expand_round_trips_unicode_and_trailing_whitespace() {
    let ws = workspace();
    let invocation = invocation_for(&ws);
    let engine = engine(&ws);
    for input in ["café 🦀  \n", "hello world\n\n", "a\u{0301}e\n"] {
        let result = compress(&engine, &invocation, input, 64, "exact");
        let expanded = engine
            .expand(&invocation, &result.exact, ExpandOptions::default())
            .unwrap();
        assert_eq!(expanded, input.as_bytes(), "payload={input:?}");
    }
}
