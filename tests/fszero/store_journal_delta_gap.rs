//! `integrate_journal_deltas` rejects gapped or truncated pages.

use std::collections::BTreeMap;

use fszero_store::journal_delta::{JournalDelta, integrate_journal_deltas};

#[test]
fn consecutive_seq_page_integrates_expected_bytes() {
    let mut state = BTreeMap::new();
    let d1 = JournalDelta::upsert(1, "a.txt", b"", b"hello");
    let d2 = JournalDelta::upsert(2, "a.txt", b"hello", b"hello world");
    let d3 = JournalDelta::upsert(3, "b.txt", b"", b"other");

    integrate_journal_deltas(&mut state, 0, &[d1, d2, d3]).expect("consecutive page");
    assert_eq!(
        state.get("a.txt").map(Vec::as_slice),
        Some(b"hello world".as_slice())
    );
    assert_eq!(
        state.get("b.txt").map(Vec::as_slice),
        Some(b"other".as_slice())
    );
}

#[test]
fn seq_gap_fails_closed_and_leaves_state_unchanged() {
    let base = BTreeMap::from([("keep.txt".to_string(), b"keep".to_vec())]);
    let mut state = base.clone();
    let prefix = JournalDelta::upsert(1, "new.txt", b"", b"applied?");
    let gap = JournalDelta::upsert(3, "later.txt", b"", b"skipped");

    let err = integrate_journal_deltas(&mut state, 0, &[prefix, gap])
        .expect_err("gapped page must fail closed");
    assert!(
        err.contains("sequence gap") && err.contains("expected 2, got 3"),
        "{err}"
    );
    assert_eq!(state, base, "failed page must not publish a prefix");
}

#[test]
fn truncated_replacement_after_hash_mismatch_fails_closed() {
    let base = BTreeMap::from([("keep.txt".to_string(), b"keep".to_vec())]);
    let mut state = base.clone();
    let prefix = JournalDelta::upsert(1, "new.txt", b"", b"applied?");
    let mut truncated = JournalDelta::upsert(2, "a.txt", b"", b"abcdef");
    truncated.replacement.truncate(3);
    truncated.byte_range.after_end = 3;

    let err = integrate_journal_deltas(&mut state, 0, &[prefix, truncated])
        .expect_err("truncated replacement must fail closed");
    assert!(err.contains("after_hash"), "{err}");
    assert_eq!(state, base, "failed page must not publish a prefix");
}
