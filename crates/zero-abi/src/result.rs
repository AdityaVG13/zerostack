//! Canonical public result envelope for the aggregate zero surface.
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::error::Error;
use std::fmt;

pub const ZERO_RESULT_V1: &str = "zero-result/v1";
pub const MAX_ACK_CHARS: usize = 64;
pub const MAX_PREVIEW_CHARS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ZeroResultV1 {
    ack: String,
    content: ZeroResultContentV1,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ZeroResultContentV1 {
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
struct ZeroResultWireV1 {
    ack: String,
    content: ZeroResultContentV1,
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
            Self::PreviewTooLong => "preview exceeds the 1024-character v1 bound",
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

impl ZeroResultV1 {
    pub fn inline(ack: impl Into<String>, value: Value) -> Result<Self, ZeroResultBuildError> {
        let ack = ack.into();
        validate_ack(&ack)?;
        Ok(Self {
            ack,
            content: ZeroResultContentV1::Inline { value },
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
            content: ZeroResultContentV1::Ref { reference, preview },
        })
    }
    pub fn ack(&self) -> &str {
        &self.ack
    }
    pub fn kind(&self) -> &'static str {
        match self.content {
            ZeroResultContentV1::Inline { .. } => "inline",
            ZeroResultContentV1::Ref { .. } => "ref",
        }
    }
    pub fn inline_value(&self) -> Result<&Value, ZeroResultAccessError> {
        match &self.content {
            ZeroResultContentV1::Inline { value } => Ok(value),
            ZeroResultContentV1::Ref { .. } => {
                Err(ZeroResultAccessError::ExpectedInline { actual: "ref" })
            }
        }
    }
    pub fn reference_value(&self) -> Result<&str, ZeroResultAccessError> {
        match &self.content {
            ZeroResultContentV1::Ref { reference, .. } => Ok(reference),
            ZeroResultContentV1::Inline { .. } => {
                Err(ZeroResultAccessError::ExpectedRef { actual: "inline" })
            }
        }
    }
    pub fn preview(&self) -> Result<Option<&str>, ZeroResultAccessError> {
        match &self.content {
            ZeroResultContentV1::Ref { preview, .. } => Ok(preview.as_deref()),
            ZeroResultContentV1::Inline { .. } => {
                Err(ZeroResultAccessError::ExpectedRef { actual: "inline" })
            }
        }
    }
}
impl<'de> Deserialize<'de> for ZeroResultV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ZeroResultWireV1::deserialize(deserializer)?;
        match wire.content {
            ZeroResultContentV1::Inline { value } => Self::inline(wire.ack, value),
            ZeroResultContentV1::Ref { reference, preview } => {
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
    let valid_scheme = ["fz://", "gz://", "tz://"]
        .iter()
        .any(|prefix| reference.starts_with(prefix));
    let suffix = reference
        .split_once("://")
        .map(|(_, suffix)| suffix)
        .unwrap_or_default();
    if valid_scheme && !suffix.is_empty() && !reference.chars().any(char::is_whitespace) {
        Ok(())
    } else {
        Err(ZeroResultBuildError::InvalidRef)
    }
}
fn validate_preview(preview: Option<&str>) -> Result<(), ZeroResultBuildError> {
    if preview.is_some_and(|value| value.chars().count() > MAX_PREVIEW_CHARS) {
        Err(ZeroResultBuildError::PreviewTooLong)
    } else {
        Ok(())
    }
}
