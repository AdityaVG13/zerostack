//! Canonical zsx result envelope.
//!
//! Mirrors the JSON envelope `zsx exec` resolves (`zerostack.zsx.v1`):
//! `{ protocol, ok, generation, request_id, result, metrics }`, extended with
//! a typed `error` object `{ code, detail, retry_after_ms? }` for failures.
//! Every execution resolves this envelope; rejections are reserved for
//! napi-level failures (e.g. an abort before the async work started).

use serde_json::Value;
use zsx_core::{ZsxExecutionMetrics, ZsxExecutionResult, ZsxSessionError};

use crate::core::{CODE_CANCELLED, CODE_COMMIT_RACE, CODE_PANIC};

/// Protocol label for the in-process zsx result envelope.
pub const ZSX_PROTOCOL: &str = "zerostack.zsx.v1";

/// Typed failure detail inside a non-ok envelope.
#[derive(Debug, Clone)]
pub struct EnvelopeError {
    pub code: String,
    pub detail: String,
    pub retry_after_ms: Option<u64>,
}

/// The canonical execution envelope resolved by `execute`.
#[derive(Debug, Clone)]
pub struct Envelope {
    pub ok: bool,
    pub generation: u64,
    pub request_id: u64,
    pub result: Option<Value>,
    pub metrics: Option<ZsxExecutionMetrics>,
    pub error: Option<EnvelopeError>,
}

impl Envelope {
    pub fn ok(generation: u64, request_id: u64, result: ZsxExecutionResult) -> Self {
        Self {
            ok: true,
            generation,
            request_id,
            result: Some(result.value),
            metrics: Some(result.metrics),
            error: None,
        }
    }

    /// Execute finished after AbortSignal. Keep the result so a committed
    /// mutation is not silently reported as cancelled.
    pub fn commit_race(generation: u64, request_id: u64, result: ZsxExecutionResult) -> Self {
        Self {
            ok: false,
            generation,
            request_id,
            result: Some(result.value),
            metrics: Some(result.metrics),
            error: Some(EnvelopeError {
                code: CODE_COMMIT_RACE.to_string(),
                detail: "execute completed after abort; mutation may have committed".to_string(),
                retry_after_ms: None,
            }),
        }
    }

    pub fn cancelled(generation: u64, request_id: u64) -> Self {
        Self {
            ok: false,
            generation,
            request_id,
            result: None,
            metrics: None,
            error: Some(EnvelopeError {
                code: CODE_CANCELLED.to_string(),
                detail: "request cancelled by AbortSignal".to_string(),
                retry_after_ms: None,
            }),
        }
    }

    /// Map a typed zsx-core error into the envelope, preserving its failure
    /// code and retry hint.
    pub fn from_zsx_error(generation: u64, request_id: u64, err: &ZsxSessionError) -> Self {
        Self {
            ok: false,
            generation,
            request_id,
            result: None,
            metrics: None,
            error: Some(EnvelopeError {
                code: err.code.as_str().to_string(),
                detail: err.detail.clone(),
                retry_after_ms: err.retry_after_ms,
            }),
        }
    }

    /// Envelope for a panic contained by `catch_unwind(AssertUnwindSafe)`.
    pub fn panic(generation: u64, request_id: u64) -> Self {
        Self {
            ok: false,
            generation,
            request_id,
            result: None,
            metrics: None,
            error: Some(EnvelopeError {
                code: CODE_PANIC.to_string(),
                detail: "backend panic contained by catch_unwind; session state is preserved"
                    .to_string(),
                retry_after_ms: None,
            }),
        }
    }

    /// Convert the envelope into canonical serde_json (resolved on the main
    /// thread via the `JsEnvelope` task value).
    pub fn to_value(&self) -> serde_json::Value {
        let mut v = serde_json::Map::new();
        v.insert("protocol".into(), serde_json::json!(ZSX_PROTOCOL));
        v.insert("ok".into(), serde_json::json!(self.ok));
        v.insert("generation".into(), serde_json::json!(self.generation));
        v.insert("request_id".into(), serde_json::json!(self.request_id));
        if let Some(result) = &self.result {
            v.insert("result".into(), result.clone());
            if let Some(metrics) = &self.metrics {
                v.insert("metrics".into(), serde_json::json!(metrics));
            }
        } else if let Some(err) = &self.error {
            let mut e = serde_json::Map::new();
            e.insert("code".into(), serde_json::json!(err.code));
            e.insert("detail".into(), serde_json::json!(err.detail));
            if let Some(ms) = err.retry_after_ms {
                e.insert("retry_after_ms".into(), serde_json::json!(ms));
            }
            v.insert("error".into(), serde_json::Value::Object(e));
        }
        serde_json::Value::Object(v)
    }
}
