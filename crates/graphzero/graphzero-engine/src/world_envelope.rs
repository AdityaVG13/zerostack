//! Validates FSZero world-ref enumeration envelopes.
//! GraphZero validates schema; FSZero owns payload resolution and bytes.

use serde_json::{Map, Value};

/// Supported world-envelope major version (FSZero world-ref).
pub const WORLD_ENVELOPE_VERSION: u64 = 1;

/// Canonical world ref prefix (`world/<wid>`, `<wid>` = `W[0-9]+`).
pub const WORLD_REF_PREFIX: &str = "world/";

/// Parsed and validated FSZero world-ref enumeration envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldEnvelope {
    /// Canonical ref, e.g. `world/W3`.
    pub world_ref: String,
    /// World id, e.g. `W3`.
    pub world: String,
    /// Per-file enumeration entries (additive subset consumed by GraphZero).
    pub files: Vec<WorldFileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldFileEntry {
    /// Root-relative path (same keying as `fs.history` / the access ledger).
    pub file: String,
    /// `clean` | `conflict` | `unreadable`; unknown statuses remain additive.
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldEnvelopeError {
    /// Missing, type-broken, or otherwise malformed envelope.
    Malformed(String),
    /// Version field is an integer but not the supported major.
    UnsupportedMajor { found: u64 },
    /// `world_ref` does not match the canonical `world/W<digits>` shape.
    InvalidWorldRef(String),
    /// Envelope `world_ref` disagrees with the caller's request `world_ref`.
    MismatchedWorldRef { envelope: String, requested: String },
}

impl std::fmt::Display for WorldEnvelopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(msg) => write!(f, "malformed world envelope: {msg}"),
            Self::UnsupportedMajor { found } => write!(
                f,
                "unsupported world-envelope major version {found}; supported major is {WORLD_ENVELOPE_VERSION}"
            ),
            Self::InvalidWorldRef(ref_text) => write!(
                f,
                "invalid world_ref {ref_text:?}; expected {WORLD_REF_PREFIX}W<digits>"
            ),
            Self::MismatchedWorldRef {
                envelope,
                requested,
            } => write!(
                f,
                "world_ref mismatch: envelope declares {envelope:?} but request used {requested:?}"
            ),
        }
    }
}

impl std::error::Error for WorldEnvelopeError {}

/// Parse and strictly validate an FSZero world-ref enumeration envelope. Accepts
/// additive unknown fields; rejects unknown major versions, missing or malformed
/// contract fields, and non-canonical world refs before any graph work is attempted.
pub fn parse_world_envelope(input: &str) -> Result<WorldEnvelope, WorldEnvelopeError> {
    let value: Value = serde_json::from_str(input)
        .map_err(|e| WorldEnvelopeError::Malformed(format!("invalid JSON: {e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| WorldEnvelopeError::Malformed("envelope must be a JSON object".into()))?;
    parse_world_envelope_value(obj)
}

/// Validate an already-decoded JSON object as a world-ref envelope.
pub fn parse_world_envelope_value(
    obj: &Map<String, Value>,
) -> Result<WorldEnvelope, WorldEnvelopeError> {
    let version = obj
        .get("version")
        .ok_or_else(|| WorldEnvelopeError::Malformed("missing version field".into()))?;
    let found = version
        .as_u64()
        .ok_or_else(|| WorldEnvelopeError::Malformed("version must be an integer".into()))?;
    if found != WORLD_ENVELOPE_VERSION {
        return Err(WorldEnvelopeError::UnsupportedMajor { found });
    }

    let world_ref = obj
        .get("world_ref")
        .and_then(Value::as_str)
        .ok_or_else(|| WorldEnvelopeError::Malformed("missing string world_ref field".into()))?
        .to_string();
    let world = obj
        .get("world")
        .and_then(Value::as_str)
        .ok_or_else(|| WorldEnvelopeError::Malformed("missing string world field".into()))?
        .to_string();
    let world_ref = canonicalize_world_ref(&world_ref)?;
    let expected_world = world_ref
        .strip_prefix(WORLD_REF_PREFIX)
        .expect("world_ref prefix validated above");
    if world != expected_world {
        return Err(WorldEnvelopeError::Malformed(format!(
            "world field {world:?} does not match world_ref {world_ref:?}"
        )));
    }

    let files = obj
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| WorldEnvelopeError::Malformed("missing files array field".into()))?
        .iter()
        .map(|entry| {
            let entry = entry.as_object().ok_or_else(|| {
                WorldEnvelopeError::Malformed("files entries must be objects".into())
            })?;
            let file = entry
                .get("file")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    WorldEnvelopeError::Malformed("file entry missing string file field".into())
                })?
                .to_string();
            let status = entry
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("clean")
                .to_string();
            Ok(WorldFileEntry { file, status })
        })
        .collect::<Result<Vec<_>, WorldEnvelopeError>>()?;

    Ok(WorldEnvelope {
        world_ref,
        world,
        files,
    })
}

/// Canonicalize `world/W<digits>`.
pub fn canonicalize_world_ref(world_ref: &str) -> Result<String, WorldEnvelopeError> {
    let wid = world_ref
        .strip_prefix(WORLD_REF_PREFIX)
        .ok_or_else(|| WorldEnvelopeError::InvalidWorldRef(world_ref.into()))?;
    let tail = wid.strip_prefix('W').unwrap_or("");
    if tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) {
        return Err(WorldEnvelopeError::InvalidWorldRef(world_ref.into()));
    }
    Ok(format!("{WORLD_REF_PREFIX}{wid}"))
}

/// Validate canonical `world/W<digits>` with at least one decimal digit.
pub fn validate_world_ref(world_ref: &str) -> Result<(), WorldEnvelopeError> {
    canonicalize_world_ref(world_ref).map(|_| ())
}

/// Bind an optional FSZero world envelope to a speculative blast request. Returns the effective
/// world ref: the envelope's `world_ref` when the request ref is empty, otherwise the request
/// ref (which must equal the envelope's). A missing envelope leaves the request ref unchanged.
pub fn bind_world_envelope(
    requested: &str,
    envelope_text: Option<&str>,
) -> Result<String, WorldEnvelopeError> {
    let Some(text) = envelope_text else {
        return Ok(requested.to_string());
    };
    let envelope = parse_world_envelope(text)?;
    if requested.is_empty() {
        return Ok(envelope.world_ref);
    }
    let requested = canonicalize_world_ref(requested)?;
    if requested != envelope.world_ref {
        return Err(WorldEnvelopeError::MismatchedWorldRef {
            envelope: envelope.world_ref,
            requested,
        });
    }
    Ok(requested)
}
