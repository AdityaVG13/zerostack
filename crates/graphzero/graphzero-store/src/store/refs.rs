//! `gz://` ref scheme parser (FR-017, ref-contract.md §3).
//!
//! This is the engine-internal grammar: it accepts short hash prefixes,
//! engine-owned kinds, and compact forms. The portable cross-engine subset
//! is ZeroRef v1 (`(fz|gz|tz)://blob/<full-sha256>` only, strict), defined in
//! docs/adr/002-zeroref-v1.md and implemented by [`super::zeroref::ZeroRef`].
//! Engine-internal refs must not be presented to other engines as portable.
//!
//! Forms:
//! ```text
//! gz://blob/<sha256-prefix>              whole blob
//! gz://blob/<hash>#B<start>-<end>        byte span (half-open)
//! gz://blob/<hash>#B<start>+<len>        byte span by length
//! gz://blob/<hash>#L<a>-<b>              line span (display convenience)
//! gz://node/<id>                         symbol/file/module node
//! gz://edge/<id>                         relationship edge
//! gz://entity/<sha256>                   knowledge entity (fact identity; not bytes)
//! gz://query/<id> (alias gz://q/<id>)    stored query result
//! gz://snap/<id>                         snapshot
//! gz://codemode/execution/<id>[/part]    CodeMode execution record part
//! g:<loc_id>                             compact locate ref (expands losslessly)
//! gz://g:<loc_id>                        alias of g:<loc_id>
//! q:<query_id>                           compact query spill ref (expands losslessly)
//! ```

use std::fmt;

use anyhow::{Result, bail};

use super::entity::validate_entity_ref_id;
use super::path_safety::{validate_blob_hash_component, validate_safe_id};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fragment {
    None,
    Bytes { start: u64, end: u64 },
    Lines { start: u64, end: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodeModeExecutionPart {
    Execution,
    Code,
    Steps,
    Telemetry,
    Result,
    Error,
}

impl CodeModeExecutionPart {
    pub fn parse(s: Option<&str>, input: &str) -> Result<Self> {
        match s.unwrap_or("execution") {
            "execution" => Ok(Self::Execution),
            "code" => Ok(Self::Code),
            "steps" => Ok(Self::Steps),
            "telemetry" => Ok(Self::Telemetry),
            "result" => Ok(Self::Result),
            "error" => Ok(Self::Error),
            other => bail!("unknown codemode execution part '{other}': {input}"),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Execution => "execution",
            Self::Code => "code",
            Self::Steps => "steps",
            Self::Telemetry => "telemetry",
            Self::Result => "result",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GzRef {
    Blob {
        hash: String,
        fragment: Fragment,
    },
    Node {
        id: String,
    },
    Edge {
        id: String,
    },
    Query {
        id: String,
    },
    Snap {
        id: String,
    },
    CodeModeExecution {
        id: String,
        part: CodeModeExecutionPart,
    },
    /// Compact locate id (`g:<u32>`) → canonical gz:// ref via snapshot locate index.
    Loc {
        id: u32,
    },
    /// Decision memory fact (`gz://mem/<id>`).
    Mem {
        id: String,
    },
    /// Knowledge entity (`gz://entity/<64-hex>`): fact identity, not bytes.
    /// Byte-level views link to the entity; expand returns the registry record.
    Entity {
        id: String,
    },
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn validate_node_edge_id(tail: &str, input: &str) -> Result<()> {
    if let Some((hash, span)) = tail.split_once("@B") {
        validate_blob_hash_component(hash, input)?;
        let (s, e) = span
            .split_once('-')
            .ok_or_else(|| anyhow::anyhow!("invalid span in ref: {input}"))?;
        let _ = parse_num(s, input)?;
        let _ = parse_num(e, input)?;
        Ok(())
    } else {
        validate_safe_id(tail, input)
    }
}

impl GzRef {
    pub fn parse(input: &str) -> Result<Self> {
        if let Some(id) = parse_compact_query_ref(input)? {
            return Ok(GzRef::Query { id });
        }
        if let Some(loc_id) = parse_loc_ref(input)? {
            return Ok(GzRef::Loc { id: loc_id });
        }
        let Some(rest) = input.strip_prefix("gz://") else {
            bail!("not a gz://, g:, or q: ref: {input}");
        };
        // Alias: gz://g:<id> (and gz://g<id>) → same Loc as compact g: form.
        if let Some(loc_id) = parse_loc_ref(rest)? {
            return Ok(GzRef::Loc { id: loc_id });
        }
        let (kind, tail) = match rest.split_once('/') {
            Some((k, t)) => (k, t),
            None => bail!("malformed gz:// ref, missing path: {input}"),
        };
        anyhow::ensure!(!tail.is_empty(), "malformed gz:// ref, empty id: {input}");
        parse_gz_kind(kind, tail, input)
    }
}

fn parse_gz_kind(kind: &str, tail: &str, input: &str) -> Result<GzRef> {
    match kind {
        "blob" => parse_blob_ref(tail, input),
        "node" => {
            validate_node_edge_id(tail, input)?;
            Ok(GzRef::Node {
                id: tail.to_string(),
            })
        }
        "edge" => {
            validate_node_edge_id(tail, input)?;
            Ok(GzRef::Edge {
                id: tail.to_string(),
            })
        }
        "query" | "q" => {
            validate_safe_id(tail, input)?;
            Ok(GzRef::Query {
                id: tail.to_string(),
            })
        }
        "snap" => {
            validate_safe_id(tail, input)?;
            Ok(GzRef::Snap {
                id: tail.to_string(),
            })
        }
        "mem" => {
            validate_safe_id(tail, input)?;
            Ok(GzRef::Mem {
                id: tail.to_string(),
            })
        }
        "entity" => {
            validate_entity_ref_id(tail).map_err(|e| anyhow::anyhow!("{e}: {input}"))?;
            Ok(GzRef::Entity {
                id: tail.to_string(),
            })
        }
        "codemode" => parse_codemode_ref(tail, input),
        other => bail!("unknown gz:// ref form '{other}': {input}"),
    }
}

fn parse_blob_ref(tail: &str, input: &str) -> Result<GzRef> {
    let (hash, frag) = match tail.split_once('#') {
        Some((h, f)) => (h, Some(f)),
        None => (tail, None),
    };
    anyhow::ensure!(
        is_hex(hash) && hash.len() <= 64,
        "invalid blob hash in ref: {input}"
    );
    let fragment = match frag {
        None => Fragment::None,
        Some(f) => parse_fragment(f, input)?,
    };
    Ok(GzRef::Blob {
        hash: hash.to_ascii_lowercase(),
        fragment,
    })
}

fn parse_fragment(f: &str, input: &str) -> Result<Fragment> {
    if let Some(span) = f.strip_prefix('B') {
        if let Some((s, e)) = span.split_once('-') {
            let (start, end) = (parse_num(s, input)?, parse_num(e, input)?);
            if end < start {
                bail!("byte span end before start: {input}");
            }
            return Ok(Fragment::Bytes { start, end });
        }
        if let Some((s, l)) = span.split_once('+') {
            // Deprecated alias (ZeroRef v1 §3): normalized on input with
            // checked arithmetic, never emitted.
            let (start, len) = (parse_num(s, input)?, parse_num(l, input)?);
            let end = start
                .checked_add(len)
                .ok_or_else(|| anyhow::anyhow!("byte span start+len overflows: {input}"))?;
            return Ok(Fragment::Bytes { start, end });
        }
        bail!("malformed byte fragment '#B{span}': {input}");
    }
    if let Some(span) = f.strip_prefix('L') {
        if let Some((a, b)) = span.split_once('-') {
            let (start, end) = (parse_num(a, input)?, parse_num(b, input)?);
            if start == 0 {
                bail!("line span start must be one-based: {input}");
            }
            if end < start {
                bail!("line span end before start: {input}");
            }
            return Ok(Fragment::Lines { start, end });
        }
        bail!("malformed line fragment '#L{span}': {input}");
    }
    bail!("unknown fragment '{f}': {input}");
}

fn parse_num(s: &str, input: &str) -> Result<u64> {
    if s.is_empty() {
        bail!("missing number in ref: {input}");
    }
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        bail!("invalid decimal number '{s}' in ref: {input}");
    }
    s.parse::<u64>()
        .map_err(|_| anyhow::anyhow!("number '{s}' overflows u64 in ref: {input}"))
}

impl fmt::Display for GzRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GzRef::Blob { hash, fragment } => match fragment {
                Fragment::None => write!(f, "gz://blob/{hash}"),
                Fragment::Bytes { start, end } => {
                    write!(f, "gz://blob/{hash}#B{start}-{end}")
                }
                Fragment::Lines { start, end } => {
                    write!(f, "gz://blob/{hash}#L{start}-{end}")
                }
            },
            GzRef::Node { id } => write!(f, "gz://node/{id}"),
            GzRef::Edge { id } => write!(f, "gz://edge/{id}"),
            GzRef::Query { id } => write!(f, "gz://query/{id}"),
            GzRef::Snap { id } => write!(f, "gz://snap/{id}"),
            GzRef::CodeModeExecution { id, part } => {
                if matches!(part, CodeModeExecutionPart::Execution) {
                    write!(f, "gz://codemode/execution/{id}")
                } else {
                    write!(f, "gz://codemode/execution/{id}/{}", part.as_str())
                }
            }
            GzRef::Loc { id } => write!(f, "g:{id}"),
            GzRef::Mem { id } => write!(f, "gz://mem/{id}"),
            GzRef::Entity { id } => write!(f, "gz://entity/{id}"),
        }
    }
}

fn parse_codemode_ref(tail: &str, input: &str) -> Result<GzRef> {
    let mut parts = tail.split('/');
    let Some("execution") = parts.next() else {
        bail!("malformed codemode ref, expected execution path: {input}");
    };
    let Some(id) = parts.next() else {
        bail!("malformed codemode ref, missing execution id: {input}");
    };
    validate_safe_id(id, input)?;
    let part = CodeModeExecutionPart::parse(parts.next(), input)?;
    if parts.next().is_some() {
        bail!("malformed codemode ref, too many path segments: {input}");
    }
    Ok(GzRef::CodeModeExecution {
        id: id.to_string(),
        part,
    })
}

fn parse_loc_ref(input: &str) -> Result<Option<u32>> {
    let Some(rest) = input.strip_prefix('g') else {
        return Ok(None);
    };
    let colon_form = rest.strip_prefix(':');
    let id_str = colon_form.unwrap_or(rest);
    if id_str.is_empty() {
        bail!("malformed locate ref, empty id: {input}");
    }
    if !id_str.bytes().all(|b| b.is_ascii_digit()) {
        if colon_form.is_some() {
            bail!("malformed locate ref, id must be decimal: {input}");
        }
        return Ok(None);
    }
    let id = id_str
        .parse::<u32>()
        .map_err(|_| anyhow::anyhow!("locate ref id overflows u32: {input}"))?;
    Ok(Some(id))
}

fn parse_compact_query_ref(input: &str) -> Result<Option<String>> {
    let Some(id) = input.strip_prefix("q:") else {
        return Ok(None);
    };
    validate_safe_id(id, "malformed compact query ref")?;
    Ok(Some(id.to_string()))
}

/// Render the canonical evidence ref for a byte span in a blob.
pub fn blob_span_ref(hash_hex: &str, start: u32, end: u32) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(10 + hash_hex.len() + 20);
    s.push_str("gz://blob/");
    s.push_str(hash_hex);
    s.push_str("#B");
    let _ = write!(s, "{start}-{end}");
    s
}
