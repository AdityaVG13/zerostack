//! One-way differential: `ZeroRef::parse` Ok => `classify_ref` == BlobSpan
//! plus crash-oracle for `EditPlan::parse`.
//!
//! Contract: classifier is syntactic-only and intentionally more permissive
//! than the strict parser (e.g. `gz://blob/deadbeef` is BlobSpan). The only
//! enforced direction is: a strictly valid portable blob ref MUST be
//! classified as BlobSpan. The converse is NOT asserted.
//!
//! Uses only public APIs from `zero-codemode` and `zero-ref`.

use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence};
use zero_codemode::{classify_ref, EditPlan, RefKind};
use zero_ref::ZeroRef;

fn config() -> Config {
    Config {
        cases: if cfg!(miri) { 8 } else { 256 },
        failure_persistence: if cfg!(miri) {
            None
        } else {
            Some(Box::new(FileFailurePersistence::WithSource(
                "proptest-regressions",
            )))
        },
        ..Config::default()
    }
}

fn hex64() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[0-9a-f]{64}").unwrap()
}

fn scheme() -> impl Strategy<Value = &'static str> {
    prop_oneof![Just("fz"), Just("gz"), Just("tz")]
}

fn valid_zeroref_strategy() -> impl Strategy<Value = String> {
    (scheme(), hex64(), 0u8..3, 0u64..4096, 0u64..4096).prop_map(
        |(scheme, hash, kind, start, span)| match kind {
            0 => format!("{scheme}://blob/{hash}"),
            1 => format!("{scheme}://blob/{hash}#B{start}-{}", start + span),
            _ => {
                let ls = if start == 0 { 1 } else { start % 64 + 1 };
                format!("{scheme}://blob/{hash}#L{ls}-{}", ls + (span % 64))
            }
        },
    )
}

/// One-way differential: any string that the strict parser accepts must be
/// classified as BlobSpan.
proptest! {
    #![proptest_config(config())]

    #[test]
    fn zeroref_ok_implies_blobspan_on_random_bytes(s in prop::string::string_regex(".*").unwrap()) {
        if let Ok(parsed) = ZeroRef::parse(&s) {
            let canonical = parsed.to_string();
            let kind = classify_ref(&s).unwrap_or_else(|e| panic!("classify_ref rejected valid ZeroRef {s:?}: {e}"));
            prop_assert_eq!(kind, RefKind::BlobSpan, "valid ZeroRef not classified as BlobSpan: {s:?} -> {parsed:?}");
            let kind2 = classify_ref(&canonical).unwrap();
            prop_assert_eq!(kind2, RefKind::BlobSpan);
        }
    }

    #[test]
    fn zeroref_ok_implies_blobspan_on_structured_valid_refs(s in valid_zeroref_strategy()) {
        let parsed = ZeroRef::parse(&s).expect("structured valid ref must parse");
        let kind = classify_ref(&s).unwrap_or_else(|e| panic!("classify_ref rejected {s:?}: {e}"));
        prop_assert_eq!(kind, RefKind::BlobSpan, "structured valid ref not BlobSpan: {s:?} {parsed:?}");
    }

    #[test]
    fn edit_plan_parse_never_panics_on_random_input(s in prop::string::string_regex(".{0,4096}").unwrap()) {
        // Size guard: bound corpus to 4 KiB to keep oracle cheap.
        // The oracle is that the parser never panics, only returns Ok or Err.
        let result = std::panic::catch_unwind(|| EditPlan::parse(&s));
        prop_assert!(result.is_ok(), "EditPlan::parse panicked on input of len {}", s.len());
    }

    #[test]
    fn edit_plan_parse_never_panics_on_arbitrary_bytes(bytes in prop::collection::vec(0u8..255u8, 0..2048)) {
        let s = String::from_utf8_lossy(&bytes).to_string();
        if s.len() > 8 * 1024 {
            return Ok(());
        }
        let result = std::panic::catch_unwind(|| EditPlan::parse(&s));
        prop_assert!(result.is_ok(), "EditPlan::parse panicked on arbitrary bytes len {}", s.len());
    }
}

#[test]
fn smoke_known_good_and_garbage_agree_disagree_table() {
    // Known-good refs: every Ok parse must be BlobSpan.
    let good: Vec<String> = {
        let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        vec![
            format!("fz://blob/{hash}"),
            format!("gz://blob/{hash}#B0-10"),
            format!("tz://blob/{hash}#L1-5"),
            format!("fz://blob/{hash}#B100-200"),
        ]
    };
    let mut agree = 0usize;
    let mut disagree = 0usize;
    for s in &good {
        let parsed = ZeroRef::parse(s).expect("known-good must parse");
        let kind = classify_ref(s).expect("known-good must classify");
        if kind == RefKind::BlobSpan {
            agree += 1;
        } else {
            disagree += 1;
            eprintln!("DISAGREE good: {s:?} parsed {parsed:?} but classify {kind:?}");
        }
        assert_eq!(kind, RefKind::BlobSpan, "known-good {s:?}");
    }

    // Garbage buffer: classifier may be permissive, parser should reject.
    // We only log the table; we do NOT assert converse.
    let garbage = vec![
        "not a ref",
        "gz://blob/deadbeef",
        "fz://blob/ZZZZ",
        "gz://node/symbol",
        "file.txt#L1-L2",
        "fz://blob/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde",
        "",
        "://blob/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "fz://blob/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef#garbage",
    ];
    let mut garbage_blobspan = 0usize;
    let mut garbage_other = 0usize;
    for s in &garbage {
        let p = ZeroRef::parse(s);
        let c = classify_ref(s);
        eprintln!(
            "garbage: {s:?} parse_ok={} classify={:?}",
            p.is_ok(),
            c
        );
        if let Ok(k) = c {
            if k == RefKind::BlobSpan {
                garbage_blobspan += 1;
            } else {
                garbage_other += 1;
            }
        }
        // Ensure EditPlan::parse doesn't panic on garbage as JSON.
        let _ = std::panic::catch_unwind(|| EditPlan::parse(s)).expect("EditPlan::parse panicked on garbage");
    }

    eprintln!(
        "agree/disagree table: good agree={agree} disagree={disagree} garbage_blobspan={garbage_blobspan} garbage_other={garbage_other} total_good={}",
        good.len()
    );
    assert_eq!(disagree, 0, "differential violated: valid ZeroRef not BlobSpan");
}
