#![no_main]

//! Differential fuzz target (tokenzero-uymp / 3kmx.2, CONF-NEG-002 EXP-008
//! F-009): TokenZeroStore (embedded) and RecoveryStore must agree on
//! arbitrary payload + fragment inputs. This is the fuzz complement to
//! tests/dual_store_fragment_proptest.rs (fixed seeds, 256 cases): libFuzzer
//! mutates raw bytes across both payload and fragment text, exploring
//! grammar corners the proptest strategies did not enumerate.

use arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;
use tokenzero_core::ContentType;
use tokenzero_recovery::embedded_store::TokenZeroStore;
use tokenzero_recovery::RecoveryStore;

#[derive(Debug)]
struct FuzzInput {
    payload: String,
    fragment: Option<String>,
}

impl<'a> Arbitrary<'a> for FuzzInput {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        // Cap sizes: fragment grammar is index-based; huge inputs add no
        // grammar coverage but cost memory per iteration.
        let payload_len = u.int_in_range(0usize..=256)?;
        let payload_bytes: Vec<u8> = (0..payload_len)
            .map(|_| u8::arbitrary(u))
            .collect::<arbitrary::Result<_>>()?;
        let payload = String::from_utf8_lossy(&payload_bytes).into_owned();
        let fragment = if bool::arbitrary(u)? {
            let frag_len = u.int_in_range(0usize..=32)?;
            let frag_bytes: Vec<u8> = (0..frag_len)
                .map(|_| u8::arbitrary(u))
                .collect::<arbitrary::Result<_>>()?;
            Some(String::from_utf8_lossy(&frag_bytes).into_owned())
        } else {
            None
        };
        Ok(FuzzInput { payload, fragment })
    }
}

enum Outcome {
    Bytes(Vec<u8>),
    Reason(String),
}

fn embedded_expand(payload: &str, fragment: Option<&str>) -> Outcome {
    let mut store = TokenZeroStore::in_memory();
    let ref_id = store.put(payload.as_bytes(), None).unwrap();
    let full = match fragment {
        Some(f) => format!("{ref_id}#{f}"),
        None => ref_id,
    };
    match store.expand(&full) {
        Ok(bytes) => Outcome::Bytes(bytes),
        Err(err) => Outcome::Reason(format!("{err:?}")),
    }
}

fn recovery_expand(payload: &str, fragment: Option<&str>) -> Outcome {
    let mut store = RecoveryStore::new(None);
    let ref_id = store.store_blob(payload, ContentType::Unknown).unwrap();
    let full = match fragment {
        Some(f) => format!("{ref_id}#{f}"),
        None => ref_id,
    };
    let result = store.expand(&full, None, None, None, None, None);
    if result.found {
        Outcome::Bytes(result.content.into_bytes())
    } else {
        Outcome::Reason(result.reason)
    }
}

/// Reason classes must match across stores. Mirrors the comparator in
/// tests/dual_store_fragment_proptest.rs exactly: TokenZeroStore error
/// variants embed the shared fragment taxonomy verbatim inside a
/// `Fragment("...")` Debug wrapper; RecoveryStore reports #L window
/// failures under its pinned `window-out-of-range` string, the same
/// out-of-range class as embedded `fragment-out-of-range`.
fn reason_class_matches(embedded: &str, recovery: &str) -> bool {
    // Keep in lockstep with tokenzero_test_support::fragment_reason_class_matches.
    // Fuzz stays free of the test-support crate.
    const CLASSES: &[&str] = &[
        "fragment-out-of-range",
        "fragment-not-utf8-boundary",
        "fragment-unknown-kind",
        "fragment-duplicate",
        "fragment-malformed",
        "fragment-reversed",
        "non_utf8_line_fragment",
        "non-utf8 line fragment",
        "NonUtf8Line",
    ];
    let Some(class) = CLASSES.iter().copied().find(|c| embedded.contains(c)) else {
        return false;
    };
    match class {
        "fragment-out-of-range" => {
            recovery.starts_with("fragment-out-of-range")
                || recovery.starts_with("window-out-of-range")
        }
        "non_utf8_line_fragment" | "non-utf8 line fragment" | "NonUtf8Line" => {
            recovery.starts_with("non_utf8_line_fragment")
                || recovery.contains("NonUtf8Line")
                || recovery.contains("non-utf8 line fragment")
        }
        class => recovery.starts_with(class),
    }
}

fuzz_target!(|data: &[u8]| {
    let input = match FuzzInput::arbitrary(&mut Unstructured::new(data)) {
        Ok(i) => i,
        Err(_) => return,
    };
    let embedded = embedded_expand(&input.payload, input.fragment.as_deref());
    let recovery = recovery_expand(&input.payload, input.fragment.as_deref());
    match (embedded, recovery) {
        (Outcome::Bytes(a), Outcome::Bytes(b)) => {
            assert_eq!(
                a, b,
                "expanded bytes diverge for payload {:?} fragment {:?}",
                input.payload, input.fragment
            );
        }
        (Outcome::Reason(a), Outcome::Reason(b)) => {
            assert!(
                reason_class_matches(&a, &b),
                "error reason class diverges for payload {:?} fragment {:?}: embedded={} recovery={}",
                input.payload, input.fragment, a, b
            );
        }
        (Outcome::Bytes(bytes), Outcome::Reason(r))
            if r.starts_with("fragment-not-utf8-boundary") =>
        {
            // Structural capability difference: TokenZeroStore expands raw
            // bytes, RecoveryStore returns String content and must fail
            // loudly when a #B range splits a UTF-8 char boundary. The
            // embedded bytes must indeed be invalid UTF-8.
            assert!(
                std::str::from_utf8(&bytes).is_err(),
                "recovery refused non-UTF8-boundary slice but embedded bytes were valid UTF-8: payload {:?} fragment {:?}",
                input.payload, input.fragment
            );
        }
        (a, b) => {
            // Line-fragment EOF clamp is shared, so found/missing must agree.
            let a = match a {
                Outcome::Bytes(_) => "ok".to_string(),
                Outcome::Reason(r) => r,
            };
            let b = match b {
                Outcome::Bytes(_) => "ok".to_string(),
                Outcome::Reason(r) => r,
            };
            assert!(
                false,
                "ok/err divergence for payload {:?} fragment {:?}: embedded={} recovery={}",
                input.payload, input.fragment, a, b
            );
        }
    }
});
