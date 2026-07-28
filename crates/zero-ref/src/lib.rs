#![forbid(unsafe_code)]

//! ZeroRef v1: the portable cross-engine blob ref subset.
//!
//! Canonical contract: GraphZero docs/adr/002-zeroref-v1.md, shared verbatim
//! across TokenZero, FSZero, and GraphZero. Golden vectors live in
//! fixtures/zeroref_v1_vectors.json and are asserted by this crate's tests.
//!
//! Interoperability scope is blob refs only:
//! (fz|gz|tz)://blob/<sha256>              whole blob
//! (fz|gz|tz)://blob/<sha256>#B<s>-<e>     byte span, zero-based half-open
//! (fz|gz|tz)://blob/<sha256>#L<a>-<b>     line span, one-based inclusive
//!
//! The hash is the full lowercase 64-hex SHA-256 of the complete unfragmented
//! bytes. Short prefixes, uppercase, non-hex, and extra path segments are
//! rejected. Non-blob kinds remain engine-owned and parse as unsupported.
//!
//! The legacy GraphZero #B<start>+<len> form is accepted as a deprecated
//! input alias, normalized internally, and never emitted.
//!
//! Engine-internal ref grammars (fz://seq/, gz node/query refs, compact
//! g:/q: forms, tz session keys) are wider and stay engine-owned; this crate
//! is the strict v1 layer only.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest as ShaDigest, Sha256};

/// Version tag for capability negotiation and fixture manifests.
pub const ZEROREF_VERSION: &str = "v1";

/// Contract major: peers with a different major must refuse before payload
/// work. Minor bumps are additive and forward-compatible.
pub const ZEROREF_MAJOR: u64 = 1;
pub const ZEROREF_MINOR: u64 = 0;

/// Identity algorithm and hex length shared by the parser and the fixture.
pub const HASH_ALGORITHM: &str = "sha256";
pub const HASH_HEX_LEN: usize = 64;

/// Hash case accepted by the parser and advertised by capability
/// descriptors. v1 is lowercase-only; uppercase is malformed.
pub const HASH_CASE: &str = "lower";

/// The only portable ref kind under v1. Everything else is engine-owned.
pub const PORTABLE_KINDS: [&str; 1] = ["blob"];

/// Exact fragment-semantics strings shared by the annex, capability
/// descriptors, and conformance tests.
pub const BYTE_FRAGMENT_SEMANTICS: &str = "#B zero-based half-open";
pub const LINE_FRAGMENT_SEMANTICS: &str = "#L one-based inclusive";

/// Deprecated GraphZero byte-span alias: accepted on input, never emitted.
pub const LEGACY_BYTE_FRAGMENT_ALIAS: &str = "#B<start>+<len>";

/// Stable error classes shared verbatim across the three engines
/// (fixtures error_classes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZeroRefErrorClass {
    /// Input does not match the v1 grammar (bad hash, bad fragment, overflow).
    Malformed,
    /// Recognizable ref that is not a portable v1 blob ref (engine-owned
    /// kinds, unknown schemes, compact forms).
    Unsupported,
    /// Fragment bounds exceed the real byte length or line count under strict
    /// selection. Strict selection never clamps.
    RangeOutOfBounds,
    /// #L selection over bytes that are not valid UTF-8.
    NotUtf8,
    /// Object not present in any reachable store.
    Missing,
    /// Store I/O failed while resolving.
    Io,
    /// Resolved bytes do not hash to the ref identity.
    DigestMismatch,
    /// Resolution denied by storage policy (e.g. shared root not opted in).
    PolicyDenied,
    /// Peer speaks an incompatible ZeroRef version.
    IncompatibleVersion,
    /// Legacy short-prefix input matched zero-or-many objects during legacy
    /// resolution. v1 parsing itself rejects prefixes as malformed.
    LegacyAmbiguity,
}

impl ZeroRefErrorClass {
    pub const ALL: [ZeroRefErrorClass; 10] = [
        Self::Malformed,
        Self::Unsupported,
        Self::RangeOutOfBounds,
        Self::NotUtf8,
        Self::Missing,
        Self::Io,
        Self::DigestMismatch,
        Self::PolicyDenied,
        Self::IncompatibleVersion,
        Self::LegacyAmbiguity,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::Unsupported => "unsupported",
            Self::RangeOutOfBounds => "range_out_of_bounds",
            Self::NotUtf8 => "not_utf8",
            Self::Missing => "missing",
            Self::Io => "io",
            Self::DigestMismatch => "digest_mismatch",
            Self::PolicyDenied => "policy_denied",
            Self::IncompatibleVersion => "incompatible_version",
            Self::LegacyAmbiguity => "legacy_ambiguity",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZeroRefError {
    pub class: ZeroRefErrorClass,
    pub message: String,
}

impl ZeroRefError {
    pub fn new(class: ZeroRefErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }
}

impl fmt::Display for ZeroRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.class.as_str(), self.message)
    }
}

impl std::error::Error for ZeroRefError {}

/// Producer scheme. Denotes provenance, not authorization or storage
/// location: a matching suffix is the same identity claim, not proof the
/// bytes are reachable from this process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZeroScheme {
    Fz,
    Gz,
    Tz,
}

impl ZeroScheme {
    /// Every scheme the v1 parser accepts. The parser iterates THIS list and
    /// capability descriptors report it, so acceptance and advertisement
    /// cannot drift.
    pub const ALL: [ZeroScheme; 3] = [Self::Fz, Self::Gz, Self::Tz];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fz => "fz",
            Self::Gz => "gz",
            Self::Tz => "tz",
        }
    }

    /// Scheme lookup against [ZeroScheme::ALL].
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|scheme| scheme.as_str() == s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZeroFragment {
    None,
    /// Zero-based half-open byte span. start == end is an allowed empty
    /// selection; start > end never parses.
    Bytes {
        start: u64,
        end: u64,
    },
    /// One-based inclusive line span. start >= 1 and start <= end are
    /// enforced at parse time; the real line count is checked at selection.
    Lines {
        start: u64,
        end: u64,
    },
}

/// A parsed canonical ZeroRef v1 portable blob ref.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZeroRefV1 {
    pub scheme: ZeroScheme,
    /// Full lowercase 64-hex SHA-256 of the complete unfragmented bytes.
    pub hash: String,
    pub fragment: ZeroFragment,
}

fn malformed(message: impl Into<String>) -> ZeroRefError {
    ZeroRefError::new(ZeroRefErrorClass::Malformed, message)
}

fn unsupported(message: impl Into<String>) -> ZeroRefError {
    ZeroRefError::new(ZeroRefErrorClass::Unsupported, message)
}

/// Full lowercase 64-hex check shared by the parser and store layers.
pub fn is_full_lower_hex(s: &str) -> bool {
    s.len() == HASH_HEX_LEN
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Lowercase hex SHA-256 of the complete bytes (the v1 identity function).
pub fn content_hash_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// Binary SHA-256 digest used by structured identities and span references.
pub type Digest = [u8; 32];

/// Canonical structured identity for a complete object.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ObjectId(pub Digest);

/// Object identity metadata shared with structured certificate wires.
pub const OBJECT_ID_HASH_ALGORITHM: &str = HASH_ALGORITHM;
pub const OBJECT_ID_HEX_LENGTH: usize = HASH_HEX_LEN;

/// Non-hot-path portable rendering of the object identity convention.
pub fn object_identity_hex(bytes: &[u8]) -> String {
    content_hash_hex(bytes)
}

/// A digest-bound byte selection. Its serde field names and digest arrays are
/// the stable structured wire shape; it does not extend the v1 text grammar.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpanRef {
    pub object_id: ObjectId,
    pub byte_start: u64,
    pub byte_len: u64,
    pub object_digest: Digest,
    pub span_digest: Digest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpanRefError {
    Selection(ZeroRefError),
    RangeOverflow,
    RangeOutOfBounds,
    ObjectIdentityMismatch,
    ObjectDigestMismatch,
    SpanDigestMismatch,
    PayloadLengthMismatch,
}

impl fmt::Display for SpanRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Selection(error) => write!(f, "selection failed: {error}"),
            other => write!(f, "{other:?}"),
        }
    }
}

impl std::error::Error for SpanRefError {}

impl From<ZeroRefError> for SpanRefError {
    fn from(error: ZeroRefError) -> Self {
        Self::Selection(error)
    }
}

fn digest_bytes(bytes: &[u8]) -> Digest {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

impl SpanRef {
    /// Construct a digest-bound span from one borrowed object buffer. Selection
    /// delegates to the canonical ZeroRef selector and returns the selected
    /// borrow with both digests in the SpanRef. No second read is required.
    pub fn from_fragment<'a>(
        object: &'a [u8],
        fragment: &ZeroFragment,
        context: &str,
    ) -> Result<(Self, &'a [u8]), SpanRefError> {
        let selected = select_fragment(object, fragment, context)?;
        let byte_start = (selected.as_ptr() as usize)
            .checked_sub(object.as_ptr() as usize)
            .ok_or(SpanRefError::RangeOverflow)?;
        let byte_start = u64::try_from(byte_start).map_err(|_| SpanRefError::RangeOverflow)?;
        let byte_len = u64::try_from(selected.len()).map_err(|_| SpanRefError::RangeOverflow)?;
        byte_start
            .checked_add(byte_len)
            .ok_or(SpanRefError::RangeOverflow)?;
        let object_digest = digest_bytes(object);
        let span = Self {
            object_id: ObjectId(object_digest),
            byte_start,
            byte_len,
            object_digest,
            span_digest: digest_bytes(selected),
        };
        Ok((span, selected))
    }

    /// Verify only a supplied selected payload. The complete object is not needed.
    pub fn verify_span(&self, payload: &[u8]) -> Result<(), SpanRefError> {
        if u64::try_from(payload.len()).ok() != Some(self.byte_len) {
            return Err(SpanRefError::PayloadLengthMismatch);
        }
        if digest_bytes(payload) != self.span_digest {
            return Err(SpanRefError::SpanDigestMismatch);
        }
        Ok(())
    }

    /// Verify complete-object identity and digest, select the bound byte range,
    /// then verify its independent span digest.
    pub fn verify_and_select<'a>(&self, object: &'a [u8]) -> Result<&'a [u8], SpanRefError> {
        let actual = digest_bytes(object);
        if actual != self.object_id.0 {
            return Err(SpanRefError::ObjectIdentityMismatch);
        }
        if actual != self.object_digest {
            return Err(SpanRefError::ObjectDigestMismatch);
        }
        let end = self
            .byte_start
            .checked_add(self.byte_len)
            .ok_or(SpanRefError::RangeOverflow)?;
        let start = usize::try_from(self.byte_start).map_err(|_| SpanRefError::RangeOutOfBounds)?;
        let end = usize::try_from(end).map_err(|_| SpanRefError::RangeOutOfBounds)?;
        let selected = object.get(start..end).ok_or(SpanRefError::RangeOutOfBounds)?;
        self.verify_span(selected)?;
        Ok(selected)
    }
}

fn parse_u64_strict(s: &str, input: &str) -> Result<u64, ZeroRefError> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(malformed(format!("invalid number '{s}' in ref: {input}")));
    }
    s.parse::<u64>()
        .map_err(|_| malformed(format!("number '{s}' overflows u64 in ref: {input}")))
}

impl ZeroRefV1 {
    /// Parse a portable v1 blob ref. Engine-owned forms (non-blob kinds,
    /// compact g:/q: refs) fail as [ZeroRefErrorClass::Unsupported];
    /// anything outside the grammar fails as [ZeroRefErrorClass::Malformed].
    pub fn parse(input: &str) -> Result<Self, ZeroRefError> {
        let Some((scheme_str, rest)) = input.split_once("://") else {
            if input.starts_with("g:") || input.starts_with("q:") {
                return Err(unsupported(format!(
                    "engine compact ref is not a portable ZeroRef: {input}"
                )));
            }
            return Err(malformed(format!("not a ZeroRef: {input}")));
        };
        let Some(scheme) = ZeroScheme::parse(scheme_str) else {
            return Err(unsupported(format!(
                "unknown ZeroRef scheme '{scheme_str}': {input}"
            )));
        };
        let Some((kind, tail)) = rest.split_once('/') else {
            return Err(malformed(format!("missing ref path: {input}")));
        };
        if !PORTABLE_KINDS.contains(&kind) {
            return Err(unsupported(format!(
                "non-blob ref kind '{kind}' is engine-owned, not portable: {input}"
            )));
        }
        let (hash, frag) = match tail.split_once('#') {
            Some((h, f)) => (h, Some(f)),
            None => (tail, None),
        };
        if hash.contains('/') {
            return Err(malformed(format!(
                "extra path segments after blob hash: {input}"
            )));
        }
        if !is_full_lower_hex(hash) {
            return Err(malformed(format!(
                "blob hash must be full lowercase 64-hex SHA-256: {input}"
            )));
        }
        let fragment = match frag {
            None => ZeroFragment::None,
            Some(f) => parse_fragment(f, input)?,
        };
        Ok(Self {
            scheme,
            hash: hash.to_string(),
            fragment,
        })
    }

    /// Apply the fragment using the canonical v1 bounds policy.
    pub fn select<'a>(&self, bytes: &'a [u8]) -> Result<&'a [u8], ZeroRefError> {
        select_fragment(bytes, &self.fragment, &self.to_string())
    }

    /// Apply the fragment with an explicit line-end policy. This is for
    /// compatibility checks and strict validation, not engine defaults.
    pub fn select_with_policy<'a>(
        &self,
        bytes: &'a [u8],
        policy: LineEndPolicy,
    ) -> Result<&'a [u8], ZeroRefError> {
        select_fragment_with_policy(bytes, &self.fragment, &self.to_string(), policy)
    }

    /// Verify the complete unfragmented bytes against the ref identity, then
    /// select the fragment with the canonical v1 policy.
    pub fn verify_and_select<'a>(&self, bytes: &'a [u8]) -> Result<&'a [u8], ZeroRefError> {
        self.verify_and_select_with_policy(bytes, CANONICAL_LINE_END_POLICY)
    }

    /// Verify the complete bytes, then select with an explicit line-end policy.
    pub fn verify_and_select_with_policy<'a>(
        &self,
        bytes: &'a [u8],
        policy: LineEndPolicy,
    ) -> Result<&'a [u8], ZeroRefError> {
        let actual = content_hash_hex(bytes);
        if actual != self.hash {
            return Err(ZeroRefError::new(
                ZeroRefErrorClass::DigestMismatch,
                format!("bytes hash to {actual}, ref claims {}", self.hash),
            ));
        }
        self.select_with_policy(bytes, policy)
    }
}

fn parse_fragment(f: &str, input: &str) -> Result<ZeroFragment, ZeroRefError> {
    if let Some(span) = f.strip_prefix('B') {
        if let Some((s, e)) = span.split_once('-') {
            let (start, end) = (parse_u64_strict(s, input)?, parse_u64_strict(e, input)?);
            if start > end {
                return Err(malformed(format!("byte span end before start: {input}")));
            }
            return Ok(ZeroFragment::Bytes { start, end });
        }
        if let Some((s, l)) = span.split_once('+') {
            // Deprecated GraphZero alias: accepted on input, never emitted.
            let (start, len) = (parse_u64_strict(s, input)?, parse_u64_strict(l, input)?);
            let end = start
                .checked_add(len)
                .ok_or_else(|| malformed(format!("byte span start+len overflows: {input}")))?;
            return Ok(ZeroFragment::Bytes { start, end });
        }
        return Err(malformed(format!(
            "malformed byte fragment '#B{span}': {input}"
        )));
    }
    if let Some(span) = f.strip_prefix('L') {
        let Some((a, b)) = span.split_once('-') else {
            return Err(malformed(format!(
                "malformed line fragment '#L{span}': {input}"
            )));
        };
        let (start, end) = (parse_u64_strict(a, input)?, parse_u64_strict(b, input)?);
        if start == 0 {
            return Err(malformed(format!("line numbering is one-based: {input}")));
        }
        if start > end {
            return Err(malformed(format!("line span end before start: {input}")));
        }
        return Ok(ZeroFragment::Lines { start, end });
    }
    Err(malformed(format!("unknown fragment '{f}': {input}")))
}

/// How line spans whose end runs past EOF are treated at selection time.
///
/// The canonical v1 policy is LineEndPolicy::ClampEnd. Strict remains
/// available for compatibility checks and callers that validate exact bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEndPolicy {
    Strict,
    ClampEnd,
}

/// Canonical ZeroRef v1 line-end policy. Byte bounds and line starts remain strict.
pub const CANONICAL_LINE_END_POLICY: LineEndPolicy = LineEndPolicy::ClampEnd;

/// Shared canonical fragment selector: byte bounds and line starts are strict;
/// a line end past EOF clamps to the final line.
/// Callers must digest-verify the complete object bytes first.
pub fn select_fragment<'a>(
    bytes: &'a [u8],
    fragment: &ZeroFragment,
    context: &str,
) -> Result<&'a [u8], ZeroRefError> {
    select_fragment_with_policy(bytes, fragment, context, CANONICAL_LINE_END_POLICY)
}

/// Fragment selector with an explicit line-end policy. Byte spans are always
/// exact; only line-span end handling varies by policy.
pub fn select_fragment_with_policy<'a>(
    bytes: &'a [u8],
    fragment: &ZeroFragment,
    context: &str,
    policy: LineEndPolicy,
) -> Result<&'a [u8], ZeroRefError> {
    match *fragment {
        ZeroFragment::None => Ok(bytes),
        ZeroFragment::Bytes { start, end } => {
            if start > end {
                // The v1 parser rejects reversed spans, but legacy surfaces
                // can construct fragments directly; never let one panic.
                return Err(malformed(format!("byte span end before start: {context}")));
            }
            let len = bytes.len() as u64;
            if end > len {
                return Err(ZeroRefError::new(
                    ZeroRefErrorClass::RangeOutOfBounds,
                    format!("byte span {start}-{end} exceeds blob length {len}: {context}"),
                ));
            }
            Ok(&bytes[start as usize..end as usize])
        }
        ZeroFragment::Lines { start, end } => {
            if start == 0 || start > end {
                return Err(malformed(format!("invalid line span: {context}")));
            }
            select_lines(bytes, start, end, context, policy)
        }
    }
}

/// Line selection semantics (annex line-fragment rules): lines terminate at
/// LF; a selected line keeps its terminating LF when present; CR is ordinary
/// content; the final line may be unterminated; the empty blob has zero lines.
fn select_lines<'a>(
    bytes: &'a [u8],
    start: u64,
    end: u64,
    context: &str,
    policy: LineEndPolicy,
) -> Result<&'a [u8], ZeroRefError> {
    if std::str::from_utf8(bytes).is_err() {
        return Err(ZeroRefError::new(
            ZeroRefErrorClass::NotUtf8,
            format!("line fragment over non-UTF-8 content: {context}"),
        ));
    }
    // line_starts[i] is the byte offset where line i+1 begins.
    let line_starts = line_start_offsets(bytes);
    let line_count = line_starts.len() as u64;
    let end = resolve_policy_end(policy, start, end, line_count, context)?;
    Ok(line_span_bytes(bytes, &line_starts, start, end))
}

/// Byte offsets where each 1-based line begins. Empty blob → empty vec.
fn line_start_offsets(bytes: &[u8]) -> Vec<usize> {
    let mut line_starts: Vec<usize> = Vec::new();
    if !bytes.is_empty() {
        line_starts.push(0);
        for (i, b) in bytes.iter().enumerate() {
            if *b == b'\n' && i + 1 < bytes.len() {
                line_starts.push(i + 1);
            }
        }
    }
    line_starts
}

/// Resolve inclusive end line under Strict or ClampEnd. Error messages stay
/// policy-specific (user-facing #L hints); do not merge arms into one predicate.
fn resolve_policy_end(
    policy: LineEndPolicy,
    start: u64,
    end: u64,
    line_count: u64,
    context: &str,
) -> Result<u64, ZeroRefError> {
    match policy {
        LineEndPolicy::Strict => {
            if end > line_count {
                return Err(ZeroRefError::new(
                    ZeroRefErrorClass::RangeOutOfBounds,
                    format!("line span {start}-{end} exceeds line count {line_count}: {context}"),
                ));
            }
            Ok(end)
        }
        LineEndPolicy::ClampEnd => {
            if line_count == 0 {
                return Err(ZeroRefError::new(
                    ZeroRefErrorClass::RangeOutOfBounds,
                    format!("line span {start}-{end} on empty blob (0 lines): {context}"),
                ));
            }
            if start > line_count {
                return Err(ZeroRefError::new(
                    ZeroRefErrorClass::RangeOutOfBounds,
                    format!(
                        "line span start {start} exceeds line count {line_count}: {context}; use #L1-{line_count} or omit the fragment for full content"
                    ),
                ));
            }
            Ok(end.min(line_count))
        }
    }
}

/// Slice bytes for 1-based inclusive line range [start, end] using precomputed starts.
fn line_span_bytes<'a>(
    bytes: &'a [u8],
    line_starts: &[usize],
    start: u64,
    end: u64,
) -> &'a [u8] {
    let start_byte = line_starts[(start - 1) as usize];
    let end_byte = if (end as usize) < line_starts.len() {
        line_starts[end as usize]
    } else {
        bytes.len()
    };
    &bytes[start_byte..end_byte]
}

impl fmt::Display for ZeroRefV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://blob/{}", self.scheme.as_str(), self.hash)?;
        match self.fragment {
            ZeroFragment::None => Ok(()),
            ZeroFragment::Bytes { start, end } => write!(f, "#B{start}-{end}"),
            ZeroFragment::Lines { start, end } => write!(f, "#L{start}-{end}"),
        }
    }
}
