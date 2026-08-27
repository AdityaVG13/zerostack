//! ZeroRef v1: the portable cross-engine blob ref subset.
//!
//! The contract implementation lives in the shared zero-ref foundation crate
//! (canonical annex mirrored at docs/design/zeroref-v1-annex.md; golden
//! vectors tests/fixtures/zeroref_v1_vectors.json, executed live by
//! tests/core/zeroref_golden.rs against parse + engine expand). This module
//! re-exports the grammar, selection algebra, and error classes.
//!
//! FSZero divergence (fszero-00vq): engine *read* surfaces clamp line-span
//! ends past EOF instead of hard-erroring (a default L1-200 window on a
//! shorter file must succeed), while starts past EOF still error. That
//! policy lives in this module's [select_fragment] wrapper.
//! Portable expand uses [ZeroRef::verify_and_select_with_policy] with
//! [LineEndPolicy::Strict]: line-span ends never clamp (golden
//! `lines_end_past_count`, annex §3). Hub [ZeroRef::verify_and_select]
//! defaults to ClampEnd; FSZero expand must not.
//!
//! The engine-internal recovery-store key grammar (fz://seq/..., view_N/...
//! aliases, opaque payload keys, the lenient normalize_ref_scheme rewrite in
//! core/recovery.rs) is wider and stays engine-owned.

pub use zero_ref::{
    BYTE_FRAGMENT_SEMANTICS, HASH_ALGORITHM, HASH_CASE, HASH_HEX_LEN, LEGACY_BYTE_FRAGMENT_ALIAS,
    LINE_FRAGMENT_SEMANTICS, LineEndPolicy, PORTABLE_KINDS, ZEROREF_MAJOR, ZEROREF_MINOR,
    ZEROREF_VERSION, ZeroFragment, ZeroRef, ZeroRefError, ZeroRefErrorClass, ZeroScheme,
};

/// The scheme this engine writes when emitting refs. Parsing accepts every
/// [ZeroScheme]; emission is always fz.
pub const EMITTED_SCHEME: ZeroScheme = ZeroScheme::Fz;

/// FSZero fragment selector for engine read surfaces: byte spans are exact,
/// line-span ends clamp to EOF (fszero-00vq). Callers must digest-verify the
/// complete object bytes first (annex verification-order rule).
pub fn select_fragment<'a>(
    bytes: &'a [u8],
    fragment: &ZeroFragment,
    context: &str,
) -> Result<&'a [u8], ZeroRefError> {
    zero_ref::select_fragment_with_policy(bytes, fragment, context, LineEndPolicy::ClampEnd)
}
