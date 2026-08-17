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

/// Redacted variant of [`zsx_error`] (ZS-SEC-004): the message is scrubbed
/// for every configured secret before it crosses the FFI boundary. Fails
/// closed -- if a secret survives redaction the throw carries a leak-guard
/// message instead of the original detail.
pub fn zsx_error_redacted(
    phase: &str,
    err: &ZsxSessionError,
    redactor: &zero_abi::Redactor,
) -> napi::Error {
    let rendered = format!("[zsx-node:{phase}] {}: {}", err.code.as_str(), err.detail);
    match redactor.redact_text_checked(&rendered) {
        Ok(redacted) => napi::Error::new(napi::Status::GenericFailure, redacted),
        Err(leak) => napi::Error::new(
            napi::Status::GenericFailure,
            format!("[zsx-node:{phase}] redaction failure: {leak}"),
        ),
    }
}

/// Synchronous throw for an addon-level failure.
pub fn message(phase: &str, detail: impl Into<String>) -> napi::Error {
    napi::Error::new(
        napi::Status::GenericFailure,
        format!("[zsx-node:{phase}] {}", detail.into()),
    )
}

/// Redacted variant of [`message`] (ZS-SEC-004), for error strings that may
/// carry secrets or environment values.
pub fn message_redacted(
    phase: &str,
    detail: impl Into<String>,
    redactor: &zero_abi::Redactor,
) -> napi::Error {
    let rendered = format!("[zsx-node:{phase}] {}", detail.into());
    match redactor.redact_text_checked(&rendered) {
        Ok(redacted) => napi::Error::new(napi::Status::GenericFailure, redacted),
        Err(leak) => napi::Error::new(
            napi::Status::GenericFailure,
            format!("[zsx-node:{phase}] redaction failure: {leak}"),
        ),
    }
}

/// Synchronous throw for a panic that escaped the `catch_unwind` boundary.
pub fn panic_error(phase: &str) -> napi::Error {
    message(
        phase,
        "internal panic contained by catch_unwind; session state is preserved",
    )
}
