//! ZeroRef: the portable cross-domain blob ref subset. The contract implementation lives in the
//! shared zero-ref foundation crate (canonical annex: docs/fszero/design/zeroref-annex.md, golden
//! vectors contracts/zeroref-fixtures.json, asserted verbatim inside the shared crate).

pub use zero_ref::{
    BYTE_FRAGMENT_SEMANTICS, HASH_ALGORITHM, HASH_CASE, HASH_HEX_LEN, LINE_FRAGMENT_SEMANTICS,
    LineEndPolicy, PORTABLE_KINDS, ZeroFragment, ZeroRef, ZeroRefError, ZeroRefErrorClass,
    ZeroScheme, content_hash_hex, is_full_lower_hex, select_fragment, select_fragment_with_policy,
};
