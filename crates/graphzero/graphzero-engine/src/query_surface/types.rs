use crate::accounting::PreventedReadAccounting;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Query surface names.
pub const SURFACE_NAMES: &[&str] = &[
    "symbol",
    "callers",
    "deps",
    "outline",
    "context",
    "hot",
    "changes",
    "word",
    "search",
    "locate",
    "delta",
    "recall",
    "callpath",
    "reading_set",
    "rg_l1",
    "rg_l2",
    "rg_l3",
    "rg_l4",
];

/// Nearest valid surface to `input`, if one is close enough to suggest. Case and separator
/// confusions are the common caller mistakes (`reading-set`, `Callers`), so those normalize to an
/// exact hit.
pub fn nearest_surface(input: &str) -> Option<&'static str> {
    let norm = |s: &str| s.to_ascii_lowercase().replace(['-', '_', ' '], "");
    let target = norm(input);
    if target.is_empty() {
        return None;
    }
    if let Some(hit) = SURFACE_NAMES.iter().find(|name| norm(name) == target) {
        return Some(hit);
    }
    let mut best: Option<(usize, &'static str)> = None;
    for name in SURFACE_NAMES {
        let distance = edit_distance(&target, &norm(name));
        if best.is_none_or(|(seen, _)| distance < seen) {
            best = Some((distance, name));
        }
    }
    best.filter(|(distance, name)| *distance <= 2 && *distance < name.len())
        .map(|(_, name)| name)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b_chars.len()).collect();
    let mut current = vec![0usize; b_chars.len() + 1];
    for (i, a_char) in a.chars().enumerate() {
        current[0] = i + 1;
        for (j, b_char) in b_chars.iter().enumerate() {
            let substitution = previous[j] + usize::from(a_char != *b_char);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b_chars.len()]
}

/// Build the canonical unknown-surface message from [`SURFACE_NAMES`].
pub fn unknown_surface_message(input: &str) -> String {
    let valid = SURFACE_NAMES.join(",");
    match nearest_surface(input) {
        Some(hit) => format!(
            "unknown surface {input}; valid: {valid}; retry orient with surface={hit} and a canonical target"
        ),
        None => format!(
            "unknown surface {input}; valid: {valid}; retry orient with surface=reading_set and a canonical target"
        ),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuerySurface {
    Symbol,
    Callers,
    Deps,
    Outline,
    Context,
    Hot,
    Changes,
    Word,
    Search,
    Locate,
    Delta,
    Recall,
    Callpath,
    ReadingSet,
    RgL1,
    RgL2,
    RgL3,
    RgL4,
}

impl QuerySurface {
    pub fn parse_surface(s: &str) -> Option<Self> {
        match s {
            "orient" => Some(Self::Symbol),
            "symbol" => Some(Self::Symbol),
            "callers" => Some(Self::Callers),
            "deps" => Some(Self::Deps),
            "outline" => Some(Self::Outline),
            "context" => Some(Self::Context),
            "hot" => Some(Self::Hot),
            "changes" => Some(Self::Changes),
            "word" => Some(Self::Word),
            "search" => Some(Self::Search),
            "locate" => Some(Self::Locate),
            "delta" => Some(Self::Delta),
            "recall" => Some(Self::Recall),
            "callpath" => Some(Self::Callpath),
            "reading_set" | "reading-set" | "readingset" => Some(Self::ReadingSet),
            "rg_l1" | "rg-l1" | "view_l1" => Some(Self::RgL1),
            "rg_l2" | "rg-l2" | "view_l2" => Some(Self::RgL2),
            "rg_l3" | "rg-l3" | "view_l3" => Some(Self::RgL3),
            "rg_l4" | "rg-l4" | "view_l4" => Some(Self::RgL4),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Symbol => "symbol",
            Self::Callers => "callers",
            Self::Deps => "deps",
            Self::Outline => "outline",
            Self::Context => "context",
            Self::Hot => "hot",
            Self::Changes => "changes",
            Self::Word => "word",
            Self::Search => "search",
            Self::Locate => "locate",
            Self::Delta => "delta",
            Self::Recall => "recall",
            Self::Callpath => "callpath",
            Self::ReadingSet => "reading_set",
            Self::RgL1 => "rg_l1",
            Self::RgL2 => "rg_l2",
            Self::RgL3 => "rg_l3",
            Self::RgL4 => "rg_l4",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QuerySurfaceRequest {
    pub surface: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub budget: Option<usize>,
    #[serde(default)]
    pub session: Option<String>,
    /// Durable `query/<id>` page cursor. Never a RAM token.
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GraphEdge {
    pub kind: String,
    pub to: String,
    pub from: Option<String>,
    pub confidence: f64,
    pub evidence_ref: String,
    pub source: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutlineItem {
    pub name: String,
    pub kind: String,
    pub evidence_ref: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchHit {
    pub label: String,
    /// Matched line(s) plus ~1 line of context; full payload stays behind
    /// `evidence_ref` (snippet rule: never inline whole nodes).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub snippet: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content_sha256: String,
    pub evidence_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadingSetEntry {
    pub target: String,
    pub kind: String,
    pub rank: u32,
    pub reason: String,
    pub evidence_ref: String,
    pub confidence: f64,
    pub confidence_level: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadingSetClosure {
    pub confidence_level: String,
    pub guarantee: String,
    pub sound_when: Vec<String>,
    pub out_of_scope: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeltaPayload {
    pub since: String,
    pub changed: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoverageFooter {
    pub tier_a: f64,
    pub tier_b: f64,
    pub tier_c: f64,
    pub freshness_verified: bool,
    pub snapshot_id: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuerySurfaceResponse {
    pub schema_version: u32,
    pub surface: String,
    pub coverage: CoverageFooter,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decl_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<GraphEdge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<OutlineItem>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub skeleton: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hits: Vec<SearchHit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reading_set: Vec<ReadingSetEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reading_set_closure: Option<ReadingSetClosure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capsule: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub absence_certificate: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs_footer: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skeletons: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<DeltaPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accounting: Option<PreventedReadAccounting>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// caller next-step hints on budget=1 success envelopes (expand / capsule /
    /// export). Empty on spilled/expand payloads; never part of expand exact-bytes documents.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
    /// Next page of a truncated result set (`query/<id>`). Durable across restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug)]
pub enum QuerySurfaceError {
    UnknownSurface(String),
    MissingArgument(&'static str),
    EvidenceMissing,
    MalformedIndex {
        blob_idx: u32,
        blob_hash_count: usize,
    },
    SymbolNotFound(String),
}

impl std::fmt::Display for QuerySurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownSurface(s) => write!(f, "{}", unknown_surface_message(s)),
            Self::MissingArgument(a) => write!(f, "missing argument {a}"),
            Self::EvidenceMissing => write!(f, "EVIDENCE_MISSING"),
            Self::MalformedIndex {
                blob_idx,
                blob_hash_count,
            } => write!(
                f,
                "MALFORMED_INDEX: blob_idx {blob_idx} out of range for {blob_hash_count} blob hashes"
            ),
            Self::SymbolNotFound(s) => write!(f, "SYMBOL_NOT_FOUND: {s}"),
        }
    }
}

impl std::error::Error for QuerySurfaceError {}
