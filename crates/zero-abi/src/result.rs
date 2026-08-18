//! Canonical public result envelope for the aggregate zero surface.
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::error::Error;
use std::fmt;

pub const ZERO_RESULT: &str = "zero-result";
pub const MAX_ACK_CHARS: usize = 64;
pub const MAX_PREVIEW_CHARS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ZeroResult {
    ack: String,
    content: ZeroResultContent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ZeroResultContent {
    Inline {
        value: Value,
    },
    Ref {
        #[serde(rename = "ref")]
        reference: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        preview: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ZeroResultWire {
    ack: String,
    content: ZeroResultContent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ZeroResultBuildError {
    InvalidAck,
    InvalidRef,
    PreviewTooLong,
}
impl fmt::Display for ZeroResultBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidAck => "ack must contain 1 to 64 characters",
            Self::InvalidRef => "ref must be a canonical fz://, gz://, or tz:// reference",
            Self::PreviewTooLong => "preview exceeds the 1024-character bound",
        })
    }
}
impl Error for ZeroResultBuildError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZeroResultAccessError {
    ExpectedInline { actual: &'static str },
    ExpectedRef { actual: &'static str },
}
impl fmt::Display for ZeroResultAccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExpectedInline { actual } => write!(f, "expected inline content, found {actual}"),
            Self::ExpectedRef { actual } => write!(f, "expected ref content, found {actual}"),
        }
    }
}
impl Error for ZeroResultAccessError {}

impl ZeroResult {
    pub fn inline(ack: impl Into<String>, value: Value) -> Result<Self, ZeroResultBuildError> {
        let ack = ack.into();
        validate_ack(&ack)?;
        Ok(Self {
            ack,
            content: ZeroResultContent::Inline { value },
        })
    }
    pub fn reference(
        ack: impl Into<String>,
        reference: impl Into<String>,
        preview: Option<String>,
    ) -> Result<Self, ZeroResultBuildError> {
        let ack = ack.into();
        let reference = reference.into();
        validate_ack(&ack)?;
        validate_ref(&reference)?;
        validate_preview(preview.as_deref())?;
        Ok(Self {
            ack,
            content: ZeroResultContent::Ref { reference, preview },
        })
    }
    pub fn ack(&self) -> &str {
        &self.ack
    }
    pub fn kind(&self) -> &'static str {
        match self.content {
            ZeroResultContent::Inline { .. } => "inline",
            ZeroResultContent::Ref { .. } => "ref",
        }
    }
    pub fn inline_value(&self) -> Result<&Value, ZeroResultAccessError> {
        match &self.content {
            ZeroResultContent::Inline { value } => Ok(value),
            ZeroResultContent::Ref { .. } => {
                Err(ZeroResultAccessError::ExpectedInline { actual: "ref" })
            }
        }
    }
    pub fn reference_value(&self) -> Result<&str, ZeroResultAccessError> {
        match &self.content {
            ZeroResultContent::Ref { reference, .. } => Ok(reference),
            ZeroResultContent::Inline { .. } => {
                Err(ZeroResultAccessError::ExpectedRef { actual: "inline" })
            }
        }
    }
    pub fn preview(&self) -> Result<Option<&str>, ZeroResultAccessError> {
        match &self.content {
            ZeroResultContent::Ref { preview, .. } => Ok(preview.as_deref()),
            ZeroResultContent::Inline { .. } => {
                Err(ZeroResultAccessError::ExpectedRef { actual: "inline" })
            }
        }
    }
}
impl<'de> Deserialize<'de> for ZeroResult {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ZeroResultWire::deserialize(deserializer)?;
        match wire.content {
            ZeroResultContent::Inline { value } => Self::inline(wire.ack, value),
            ZeroResultContent::Ref { reference, preview } => {
                Self::reference(wire.ack, reference, preview)
            }
        }
        .map_err(serde::de::Error::custom)
    }
}
fn validate_ack(ack: &str) -> Result<(), ZeroResultBuildError> {
    if (1..=MAX_ACK_CHARS).contains(&ack.chars().count()) {
        Ok(())
    } else {
        Err(ZeroResultBuildError::InvalidAck)
    }
}
fn validate_ref(reference: &str) -> Result<(), ZeroResultBuildError> {
    zero_ref::ZeroRef::parse(reference)
        .map(|_| ())
        .map_err(|_| ZeroResultBuildError::InvalidRef)
}
fn validate_preview(preview: Option<&str>) -> Result<(), ZeroResultBuildError> {
    if preview.is_some_and(|value| value.chars().count() > MAX_PREVIEW_CHARS) {
        Err(ZeroResultBuildError::PreviewTooLong)
    } else {
        Ok(())
    }
}

/// Build a `zero-result` envelope from one engine step.
///
/// Extracted from the engine-local CodeMode hosts. Canonical `fz://` /
/// `gz://` / `tz://` recovery keys become `content.kind=ref`. Non-canonical
/// aliases stay inline. Failures are always inline `{ok:false, detail, method}`.
pub fn from_step(
    ack: &str,
    ok: bool,
    method: &str,
    recovery_key: &str,
    payload_wire: &Value,
    detail: Option<&str>,
) -> ZeroResult {
    let ack = normalize_ack(ack, ok);
    if !ok {
        return inline_or_x0(
            &ack,
            serde_json::json!({
                "ok": false,
                "method": method,
                "detail": detail.unwrap_or("failed"),
            }),
        );
    }

    if let Some(reference) = canonical_zeroref(recovery_key).or_else(|| {
        payload_wire
            .get("ref")
            .and_then(Value::as_str)
            .and_then(canonical_zeroref)
    }) {
        let preview = payload_wire
            .get("preview")
            .and_then(Value::as_str)
            .map(|s| truncate_chars(s, MAX_PREVIEW_CHARS))
            .or_else(|| detail.map(|d| truncate_chars(d, MAX_PREVIEW_CHARS)));
        if let Ok(result) = ZeroResult::reference(ack.clone(), reference, preview) {
            return result;
        }
    }

    let value = if let Some(text) = payload_wire.as_str() {
        serde_json::json!(text)
    } else if payload_wire.is_null() {
        serde_json::json!({
            "ok": true,
            "method": method,
            "detail": detail,
        })
    } else {
        payload_wire.clone()
    };
    inline_or_x0(&ack, value)
}

/// Serialize a result for the CodeMode wire (canonical tagged shape only).
pub fn to_wire(result: &ZeroResult) -> Value {
    serde_json::to_value(result).expect("ZeroResult always serializes")
}

fn normalize_ack(ack: &str, ok: bool) -> String {
    let trimmed = ack.trim();
    if (1..=MAX_ACK_CHARS).contains(&trimmed.chars().count()) {
        trimmed.to_string()
    } else if ok {
        "ok".into()
    } else {
        "X0".into()
    }
}

fn inline_or_x0(ack: &str, value: Value) -> ZeroResult {
    ZeroResult::inline(ack, value).unwrap_or_else(|_| {
        ZeroResult::inline(
            "X0",
            serde_json::json!({"ok": false, "detail": "invalid zero-result ack"}),
        )
        .expect("X0 inline always valid")
    })
}

fn canonical_zeroref(reference: &str) -> Option<&str> {
    let valid_scheme = ["fz://", "gz://", "tz://"]
        .iter()
        .any(|prefix| reference.starts_with(prefix));
    let suffix = reference
        .split_once("://")
        .map(|(_, suffix)| suffix)
        .unwrap_or_default();
    if valid_scheme && !suffix.is_empty() && !reference.chars().any(char::is_whitespace) {
        Some(reference)
    } else {
        None
    }
}

fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}
