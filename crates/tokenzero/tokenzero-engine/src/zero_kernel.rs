//! TokenEngine (`ZeroTokenEngine`) lives in `tokenzero-kernel`.
//!
//! This path used to be compiled by both crates: engine as `pub mod zero_kernel`
//! and kernel via `#[path = "../../tokenzero-engine/src/zero_kernel.rs"]`.
//! That made kernel advertise engine-owned types (and engine advertise kernel
//! types). The implementation moved to `crates/tokenzero/tokenzero-kernel/src/lib.rs`.
//!
//! File retained (RULE 1). Not a module of this crate.
