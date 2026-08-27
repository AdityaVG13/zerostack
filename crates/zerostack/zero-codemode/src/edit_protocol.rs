//! Zero Edit Protocol v1: the compact, ref-based edit contract shared by
//! FSZero, GraphZero and TokenZero surfaces.
//!
//! The wire contract and validation rules are defined by this module.
//!
//! Schema budget: the protocol is exposed as ONE generic `EDIT` operation whose
//! argument is a list of [`EditOp`] values. Verbs live in the payload (`v`
//! discriminant), not in the tool namespace.
//!
//! Ref grammar is NOT redefined here. It is the existing one:
//! * FSZero snap-to-file target refs `<path>#L<start>-L<end>` (1-based, inclusive),
//! * ZeroRef portable blob refs `fz://blob/<sha256>[#L..|#B..]`,
//! * GraphZero symbol refs `gz://node/<symbol>` (and `gz://blob/...` evidence).

use serde::{Deserialize, Serialize};

/// Protocol version string carried by [`EditPlan`].
pub const EDIT_PROTOCOL_VERSION: &str = "zep/1";

/// Which existing grammar produced a ref.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// `<path>#L<start>-L<end>` -- FSZero snap-to-file target grammar.
    FileSpan,
    /// `fz://blob/<sha256>[#L..|#B..]` -- ZeroRef portable blob ref.
    BlobSpan,
    /// `gz://node/<symbol>` -- GraphZero symbol ref.
    Symbol,
    /// A bare workspace-relative path (whole-object operand).
    Path,
}

/// Error classes for payload validation. Names are stable wire strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditErrorClass {
    /// A ref did not match any accepted grammar.
    MalformedRef,
    /// A ref parsed, but its kind is not accepted by this verb's slot.
    RefKindMismatch,
    /// A required field was empty.
    EmptyField,
    /// The protocol version is not understood.
    UnsupportedVersion,
}

/// A rejected payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditError {
    pub class: EditErrorClass,
    pub message: String,
}

impl EditError {
    fn new(class: EditErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.class, self.message)
    }
}

impl std::error::Error for EditError {}

/// Classify a ref string against the existing grammars.
///
/// This is a syntactic classifier only: resolution, digest verification and
/// clamp policy stay in the owning engine (`zero_ref` for blob refs, FSZero for
/// file spans, GraphZero for symbols).
const BLOB_SCHEMES: &[&str] = &["fz://", "tz://"];

pub fn classify_ref(input: &str) -> Result<RefKind, EditError> {
    if input.is_empty() {
        return Err(EditError::new(EditErrorClass::MalformedRef, "empty ref"));
    }
    if let Some(rest) = input.strip_prefix("gz://node/") {
        return if rest.is_empty() {
            Err(EditError::new(
                EditErrorClass::MalformedRef,
                "gz://node/ requires a symbol",
            ))
        } else {
            Ok(RefKind::Symbol)
        };
    }
    if let Some(rest) = input.strip_prefix("gz://") {
        return if rest.starts_with("blob/") {
            Ok(RefKind::BlobSpan)
        } else {
            Err(EditError::new(
                EditErrorClass::MalformedRef,
                "unknown gz:// kind",
            ))
        };
    }
    // Both `fz://` and `tz://` are 5 bytes; keep the original slice length.
    if BLOB_SCHEMES.iter().any(|scheme| input.starts_with(scheme)) {
        return if input["fz://".len()..].starts_with("blob/") {
            Ok(RefKind::BlobSpan)
        } else {
            Err(EditError::new(
                EditErrorClass::MalformedRef,
                "unknown blob ref kind",
            ))
        };
    }
    if input.contains("#L") {
        return parse_file_span(input);
    }
    if input.contains("://") {
        return Err(EditError::new(
            EditErrorClass::MalformedRef,
            "unknown ref scheme",
        ));
    }
    Ok(RefKind::Path)
}

/// `<path>#L<start>-L<end>`, mirroring FSZero `parse_target_ref`.
fn parse_file_span(input: &str) -> Result<RefKind, EditError> {
    let malformed = || {
        EditError::new(
            EditErrorClass::MalformedRef,
            format!("malformed file span: {input}"),
        )
    };
    let (path, suffix) = input.rsplit_once("#L").ok_or_else(malformed)?;
    if path.is_empty() {
        return Err(malformed());
    }
    let (start, end) = suffix.split_once("-L").ok_or_else(malformed)?;
    let start: usize = start.parse().map_err(|_| malformed())?;
    let end: usize = end.parse().map_err(|_| malformed())?;
    if start == 0 || end < start {
        return Err(malformed());
    }
    Ok(RefKind::FileSpan)
}

fn require_kinds(field: &str, value: &str, allowed: &[RefKind]) -> Result<(), EditError> {
    let kind = classify_ref(value)?;
    if allowed.contains(&kind) {
        Ok(())
    } else {
        Err(EditError::new(
            EditErrorClass::RefKindMismatch,
            format!("{field}: {kind:?} is not accepted here (expected one of {allowed:?})"),
        ))
    }
}

fn require_nonempty(field: &str, value: &str) -> Result<(), EditError> {
    if value.is_empty() {
        Err(EditError::new(
            EditErrorClass::EmptyField,
            format!("{field} must not be empty"),
        ))
    } else {
        Ok(())
    }
}

/// Where an `INSERT` lands relative to its anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Before,
    #[default]
    After,
}

/// The nine v1 verbs, carried as one compact tagged payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "v")]
pub enum EditOp {
    /// Expand an existing ref into bytes.
    #[serde(rename = "READ")]
    Read { r: String },
    /// Overwrite the span identified by `r` with `text`.
    #[serde(rename = "REPLACE")]
    Replace { r: String, text: String },
    /// Insert `text` adjacent to the anchor span `at`.
    #[serde(rename = "INSERT")]
    Insert {
        at: String,
        text: String,
        #[serde(default, skip_serializing_if = "is_default_side")]
        side: Side,
    },
    /// Remove the span identified by `r`.
    #[serde(rename = "DELETE")]
    Delete { r: String },
    /// Move a whole object to `to`.
    #[serde(rename = "MOVE")]
    Move { from: String, to: String },
    /// Copy a whole object to `to`.
    #[serde(rename = "COPY")]
    Copy { from: String, to: String },
    /// Rename the symbol identified by a GraphZero symbol ref.
    #[serde(rename = "RENAME")]
    Rename { sym: String, to: String },
    /// Apply a unified diff against `base`.
    #[serde(rename = "APPLY_PATCH")]
    ApplyPatch { base: String, patch: String },
    /// Execute a command ref (or literal command line).
    #[serde(rename = "RUN")]
    Run { cmd: String },
}

fn is_default_side(side: &Side) -> bool {
    *side == Side::After
}

/// Span slots accept file spans and blob spans.
const SPAN_SLOT: &[RefKind] = &[RefKind::FileSpan, RefKind::BlobSpan];
/// Object slots address whole files.
const OBJ_SLOT: &[RefKind] = &[RefKind::Path, RefKind::BlobSpan];

impl EditOp {
    /// Stable verb name as it appears on the wire.
    pub fn verb(&self) -> &'static str {
        match self {
            EditOp::Read { .. } => "READ",
            EditOp::Replace { .. } => "REPLACE",
            EditOp::Insert { .. } => "INSERT",
            EditOp::Delete { .. } => "DELETE",
            EditOp::Move { .. } => "MOVE",
            EditOp::Copy { .. } => "COPY",
            EditOp::Rename { .. } => "RENAME",
            EditOp::ApplyPatch { .. } => "APPLY_PATCH",
            EditOp::Run { .. } => "RUN",
        }
    }

    /// Validate ref slots and required fields. Structural (serde) validity is
    /// necessary but not sufficient; every accepted op must also pass this.
    pub fn validate(&self) -> Result<(), EditError> {
        match self {
            EditOp::Read { r } => require_kinds(
                "r",
                r,
                &[RefKind::FileSpan, RefKind::BlobSpan, RefKind::Path],
            ),
            EditOp::Replace { r, .. } => require_kinds("r", r, SPAN_SLOT),
            EditOp::Insert { at, .. } => require_kinds("at", at, SPAN_SLOT),
            EditOp::Delete { r } => require_kinds("r", r, SPAN_SLOT),
            EditOp::Move { from, to } => {
                require_kinds("from", from, OBJ_SLOT)?;
                require_kinds("to", to, &[RefKind::Path])
            }
            EditOp::Copy { from, to } => {
                require_kinds("from", from, OBJ_SLOT)?;
                require_kinds("to", to, &[RefKind::Path])
            }
            EditOp::Rename { sym, to } => {
                require_kinds("sym", sym, &[RefKind::Symbol])?;
                require_nonempty("to", to)
            }
            EditOp::ApplyPatch { base, patch } => {
                require_kinds("base", base, &[RefKind::Path, RefKind::BlobSpan])?;
                require_nonempty("patch", patch)
            }
            EditOp::Run { cmd } => require_nonempty("cmd", cmd),
        }
    }

    /// Level-0 rendering: the plain-text form a producer can always emit instead
    /// of the compact payload. Every verb has one, so falling back to Level 0
    /// never loses capability.
    pub fn level0(&self) -> String {
        match self {
            EditOp::Read { r } => format!("read {r}"),
            EditOp::Replace { r, text } => format!("replace {r}\n<<<\n{text}\n>>>"),
            EditOp::Insert { at, text, side } => {
                let side = match side {
                    Side::Before => "before",
                    Side::After => "after",
                };
                format!("insert {side} {at}\n<<<\n{text}\n>>>")
            }
            EditOp::Delete { r } => format!("delete {r}"),
            EditOp::Move { from, to } => format!("move {from} -> {to}"),
            EditOp::Copy { from, to } => format!("copy {from} -> {to}"),
            EditOp::Rename { sym, to } => format!("rename {sym} -> {to}"),
            EditOp::ApplyPatch { base, patch } => format!("apply_patch {base}\n{patch}"),
            EditOp::Run { cmd } => format!("run {cmd}"),
        }
    }
}

/// The single generic `EDIT` operation payload: a version tag plus ordered ops.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditPlan {
    #[serde(default = "default_version")]
    pub p: String,
    pub ops: Vec<EditOp>,
}

fn default_version() -> String {
    EDIT_PROTOCOL_VERSION.to_string()
}

impl EditPlan {
    pub fn new(ops: Vec<EditOp>) -> Self {
        Self {
            p: default_version(),
            ops,
        }
    }

    /// Parse and validate a compact plan.
    pub fn parse(json: &str) -> Result<Self, EditError> {
        let plan: EditPlan = serde_json::from_str(json)
            .map_err(|e| EditError::new(EditErrorClass::MalformedRef, e.to_string()))?;
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<(), EditError> {
        if self.p != EDIT_PROTOCOL_VERSION {
            return Err(EditError::new(
                EditErrorClass::UnsupportedVersion,
                format!("expected {EDIT_PROTOCOL_VERSION}, got {}", self.p),
            ));
        }
        for op in &self.ops {
            op.validate()?;
        }
        Ok(())
    }

    /// Full Level-0 fallback rendering of the plan.
    pub fn level0(&self) -> String {
        self.ops
            .iter()
            .map(EditOp::level0)
            .collect::<Vec<_>>()
            .join("\n")
    }
}
