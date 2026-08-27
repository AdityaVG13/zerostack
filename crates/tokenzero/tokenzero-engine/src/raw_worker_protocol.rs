//! Compatibility module for the canonical ZeroStack raw-worker v2 authority.
//!
//! TokenZero owns no wire structs, codecs, validators, limits, or protocol
//! digest. Consumers import these re-exports from the pinned `zero-abi` crate.

pub use zero_abi::raw_worker::*;
