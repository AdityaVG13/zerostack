//! Error helpers for the sync control-plane surface.
//!
//! Async executions resolve the canonical envelope instead of throwing, so
//! typed zsx-core errors reach JS as `{ error: { code, detail } }`; the
//! helpers here only shape synchronous throws (constructor, admission,
//! control-plane tasks that reject).

use zsx_core::ZsxSessionError;

/// Synchronous throw for a typed zsx-core error, prefixed with the calling
/// phase so the failure is attributable.
pub fn zsx_error(phase: &str, err: &ZsxSessionError) -> napi::Error {
    napi::Error::new(
        napi::Status::GenericFailure,
        format!("[zsx-node:{phase}] {}: {}", err.code.as_str(), err.detail),
    )
}

/// Synchronous throw for an addon-level failure.
pub fn message(phase: &str, detail: impl Into<String>) -> napi::Error {
    napi::Error::new(
        napi::Status::GenericFailure,
        format!("[zsx-node:{phase}] {}", detail.into()),
    )
}

/// Synchronous throw for a panic that escaped the `catch_unwind` boundary.
pub fn panic_error(phase: &str) -> napi::Error {
    message(
        phase,
        "internal panic contained by catch_unwind; session state is preserved",
    )
}
