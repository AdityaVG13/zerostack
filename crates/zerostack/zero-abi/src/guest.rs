//! Retired pre-ZeroKernel guest catalog.
//!
//! The only model-facing method table is `zero_kernel::GUEST_METHODS`, which
//! contains exactly `read`, `find`, `edit`, `apply`, `run`, and `state`.
//! Domain capabilities remain behind typed engine traits.
