//! TokenEngine contract tests: every implementation of `zero_abi::TokenEngine`
//! must satisfy these. Engines call `run_all` with their concrete instance.

use zero_abi::{TokenAccounting, TokenEngine};

use crate::SuiteResult;

/// Run the full TokenEngine conformance suite.
pub fn run_all(engine: &dyn TokenEngine, invocation: &zero_abi::EngineInvocation) -> SuiteResult {
    let mut result = SuiteResult::default();
    let sample = b"The quick brown fox jumps over the lazy dog. The quick brown fox jumps over the lazy dog.";

    // measure determinism: same bytes → same accounting
    {
        let name = "measure_determinism";
        let a = engine.measure(invocation, sample);
        let b = engine.measure(invocation, sample);
        match (a, b) {
            (Ok(a), Ok(b)) if a == b => result.record_pass(name),
            (Ok(a), Ok(b)) => result.record_fail(
                name,
                format!("same bytes produced different accounting:\n  {a:?}\n  {b:?}"),
            ),
            (Err(e1), Err(e2)) if e1.to_string() == e2.to_string() => {
                result.record_pass(name) // both fail identically is still deterministic
            }
            (e1, e2) => result.record_fail(name, format!("inconsistent errors: {e1:?} vs {e2:?}")),
        }
    }

    // certify: matches fresh measurement, rejects tampered claim
    {
        let name = "certify_matches_fresh";
        let claimed = engine
            .measure(invocation, sample)
            .unwrap_or(TokenAccounting {
                tokenizer: "unknown".into(),
                billed: 0,
                visible: 0,
                cached: 0,
                certified: false,
            });
        let certify_result = engine.certify(invocation, sample, &claimed);
        match certify_result {
            Ok(cr) if cr.matches => result.record_pass(name),
            Ok(cr) => result.record_fail(
                name,
                format!(
                    "certify says mismatch for identical input: {:?}",
                    cr.recomputed
                ),
            ),
            Err(e) => result.record_fail(name, format!("certify errored: {e}")),
        }
    }

    // certify detects tampered claim
    {
        let name = "certify_detects_tampered_claim";
        let mut forged = engine
            .measure(invocation, sample)
            .unwrap_or(TokenAccounting {
                tokenizer: String::new(),
                billed: 0,
                visible: 0,
                cached: 0,
                certified: false,
            });
        forged.billed = forged.billed.saturating_add(999_999);
        let certify_result = engine.certify(invocation, sample, &forged);
        match certify_result {
            Ok(cr) if !cr.matches => result.record_pass(name),
            Ok(_) => result.record_fail(
                name,
                "certify accepted an inflated billed count — accounting lies possible",
            ),
            Err(e) => result.record_fail(
                name,
                format!("certify should not error on tampered claim: {e}"),
            ),
        }
    }

    result
}
