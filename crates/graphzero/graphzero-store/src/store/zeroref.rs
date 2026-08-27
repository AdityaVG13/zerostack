//! ZeroRef v1: the portable cross-engine blob ref subset.
//!
//! The contract implementation lives in the shared zero-ref foundation crate
//! (canonical annex: docs/adr/002-zeroref-v1.md, golden vectors:
//! docs/contracts/zeroref-v1-fixtures.json, asserted verbatim inside the
//! shared crate). This module re-exports it so GraphZero, FSZero, and
//! TokenZero can never drift on the grammar, selection algebra, or error
//! classes. The engine-internal [super::refs::GzRef] grammar (nodes,
//! queries, snaps, prefixes, compact g:/q: forms) is wider and stays
//! engine-owned.

pub use zero_ref::{
    BYTE_FRAGMENT_SEMANTICS, HASH_ALGORITHM, HASH_CASE, HASH_HEX_LEN, LEGACY_BYTE_FRAGMENT_ALIAS,
    LINE_FRAGMENT_SEMANTICS, LineEndPolicy, PORTABLE_KINDS, ZEROREF_MAJOR, ZEROREF_MINOR,
    ZEROREF_VERSION, ZeroFragment, ZeroRef, ZeroRefError, ZeroRefErrorClass, ZeroScheme,
    content_hash_hex, is_full_lower_hex, select_fragment, select_fragment_with_policy,
};
