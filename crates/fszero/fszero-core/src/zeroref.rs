//! ZeroRef: the portable cross-domain blob ref subset.

pub use zero_ref::{
    BYTE_FRAGMENT_SEMANTICS, HASH_ALGORITHM, HASH_CASE, HASH_HEX_LEN, LINE_FRAGMENT_SEMANTICS,
    LineEndPolicy, PORTABLE_KINDS, ZeroFragment, ZeroRef, ZeroRefError, ZeroRefErrorClass,
    ZeroScheme,
};

/// The scheme this engine writes when emitting refs. One ZeroStack family.
pub const EMITTED_SCHEME: ZeroScheme = ZeroScheme::Z;

/// FSZero fragment selector for engine read surfaces: byte spans are exact, line-span ends clamp to
/// EOF. Callers must digest-verify the complete object bytes first (annex verification-order rule).
pub fn select_fragment<'a>(
    bytes: &'a [u8],
    fragment: &ZeroFragment,
    context: &str,
) -> Result<&'a [u8], ZeroRefError> {
    zero_ref::select_fragment_with_policy(bytes, fragment, context, LineEndPolicy::ClampEnd)
}
