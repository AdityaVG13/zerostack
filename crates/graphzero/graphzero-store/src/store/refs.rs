//! Graph domain-internal ref parser. Portable content handles are ZeroRef / ZeroHandle
//! `z://blob/<blake3>`. Domain keys are unprefixed kinds (`node/<id>`, `query/<id>`, …).

use std::fmt;

use anyhow::{Result, bail};
use zero_ref::{ZeroFragment, ZeroRef};

use super::entity::validate_entity_ref_id;
use super::path_safety::{validate_blob_hash_component, validate_safe_id};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fragment {
    None,
    Bytes { start: u64, end: u64 },
    Lines { start: u64, end: u64 },
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
    /// Compact locate id (`g:<u32>`) → canonical graph ref via snapshot locate index.
    Loc {
        id: u32,
    },
    /// Decision memory fact (`mem/<id>`).
    Mem {
        id: String,
    },
    /// Knowledge entity (`entity/<64-hex>`): fact identity, not bytes.
    /// Byte-level views link to the entity; expand returns the registry record.
    Entity {
        id: String,
    },
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
        if input.starts_with("z://") {
            return parse_portable_blob(input);
        }
        if input.contains("://") {
            bail!("retired or unsupported graph ref scheme: {input}");
        }
        let (kind, tail) = match input.split_once('/') {
            Some((kind, tail)) => (kind, tail),
            None => bail!("not a graph ref, g:, or q: ref: {input}"),
        };
        anyhow::ensure!(!tail.is_empty(), "malformed graph ref, empty id: {input}");
        parse_gz_kind(kind, tail, input)
    }
}

fn parse_gz_kind(kind: &str, tail: &str, input: &str) -> Result<GzRef> {
    match kind {
        "blob" => bail!("portable blob refs require z://blob/<digest>: {input}"),
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
        other => bail!("unknown graph ref form '{other}': {input}"),
    }
}

fn parse_portable_blob(input: &str) -> Result<GzRef> {
    let parsed = ZeroRef::parse(input).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let fragment = match parsed.fragment {
        ZeroFragment::None => Fragment::None,
        ZeroFragment::Bytes { start, end } => Fragment::Bytes { start, end },
        ZeroFragment::Lines { start, end } => Fragment::Lines { start, end },
    };
    Ok(GzRef::Blob {
        hash: parsed.hash,
        fragment,
    })
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
                Fragment::None => write!(f, "z://blob/{hash}"),
                Fragment::Bytes { start, end } => {
                    write!(f, "z://blob/{hash}#B{start}-{end}")
                }
                Fragment::Lines { start, end } => {
                    write!(f, "z://blob/{hash}#L{start}-{end}")
                }
            },
            GzRef::Node { id } => write!(f, "node/{id}"),
            GzRef::Edge { id } => write!(f, "edge/{id}"),
            GzRef::Query { id } => write!(f, "query/{id}"),
            GzRef::Snap { id } => write!(f, "snap/{id}"),
            GzRef::Loc { id } => write!(f, "g:{id}"),
            GzRef::Mem { id } => write!(f, "mem/{id}"),
            GzRef::Entity { id } => write!(f, "entity/{id}"),
        }
    }
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
    s.push_str("z://blob/");
    s.push_str(hash_hex);
    s.push_str("#B");
    let _ = write!(s, "{start}-{end}");
    s
}
