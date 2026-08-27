#![forbid(unsafe_code)]

//! FSZero implementation consumed directly by ZeroKernel.

#[path = "../../fszero-engine/src/zero_kernel.rs"]
mod implementation;

pub use implementation::{ZeroFileEngine, ZeroFileLease};
