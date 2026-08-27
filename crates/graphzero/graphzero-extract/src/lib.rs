//! GraphZero P0.2 Tier-A Extraction — tree-sitter pipeline.
//!
//! Extracts symbol nodes and confidence-marked edges with byte-exact
//! evidence refs from Rust, TypeScript, and Python source blobs.
//! Embarrassingly parallel, zero cross-blob state, deterministic.

pub mod confidence_band;
pub mod detect;
pub mod engine;
pub mod queries;
pub mod rust_analyzer;
pub mod rust_analyzer_lsp;
pub mod tsserver;
pub mod typed_fusion;

use std::fmt;
use std::process::Command;

pub use graphzero_types::ContentHash;

/// Edge kinds extracted by Tier-A (per architecture.md section 2,
/// ref-contract.md section 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EdgeKind {
    /// File contains symbol. Confidence 1.0.
    Contains,
    /// Caller calls callee (intra-blob). Confidence 0.9.
    Calls,
    /// Source imports a path. Confidence 0.8.
    Imports,
    /// Type implements trait (intra-blob). Confidence 0.85.
    Implements,
}

impl fmt::Display for EdgeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EdgeKind::Contains => write!(f, "contains"),
            EdgeKind::Calls => write!(f, "calls"),
            EdgeKind::Imports => write!(f, "imports"),
            EdgeKind::Implements => write!(f, "implements"),
        }
    }
}

/// Symbol node kinds (FR-004).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeKind {
    Function,
    Struct,
    Enum,
    Trait,
    Type,
    Module,
    Variable,
    Class,
    Interface,
    Method,
}

impl fmt::Display for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeKind::Function => write!(f, "function"),
            NodeKind::Struct => write!(f, "struct"),
            NodeKind::Enum => write!(f, "enum"),
            NodeKind::Trait => write!(f, "trait"),
            NodeKind::Type => write!(f, "type"),
            NodeKind::Module => write!(f, "module"),
            NodeKind::Variable => write!(f, "variable"),
            NodeKind::Class => write!(f, "class"),
            NodeKind::Interface => write!(f, "interface"),
            NodeKind::Method => write!(f, "method"),
        }
    }
}

/// Synthetic node id offset for path/external target nodes.
pub const PATH_NODE_OFFSET: u32 = 0x8000_0000;

/// Synthetic node id used when a caller cannot be resolved to a symbol node.
pub const FILE_NODE_ID: u32 = 0xFFFF_FFFE;

/// Provenance of an extracted edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    TreeSitter,
    RustAnalyzer,
    TypeScriptServer,
    Both,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::TreeSitter => write!(f, "tree-sitter"),
            Source::RustAnalyzer => write!(f, "rust-analyzer"),
            Source::TypeScriptServer => write!(f, "tsserver"),
            Source::Both => write!(f, "both"),
        }
    }
}

/// Invalid byte bounds supplied for an evidence reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvidenceRefError {
    pub start: u32,
    pub end: u32,
}

impl fmt::Display for EvidenceRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "evidence span start {} exceeds end {}",
            self.start, self.end
        )
    }
}

impl std::error::Error for EvidenceRefError {}

/// Byte-span evidence reference (INV-001: every edge must have one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceRef {
    pub blob_hash: ContentHash,
    pub start: u32,
    pub end: u32,
}

impl EvidenceRef {
    pub fn new(blob_hash: ContentHash, start: u32, end: u32) -> Result<Self, EvidenceRefError> {
        if start > end {
            return Err(EvidenceRefError { start, end });
        }
        Ok(Self::new_unchecked(blob_hash, start, end))
    }

    pub(crate) fn new_unchecked(blob_hash: ContentHash, start: u32, end: u32) -> Self {
        debug_assert!(start <= end);
        Self {
            blob_hash,
            start,
            end,
        }
    }

    /// Render as gz://blob/HASH#BSTART-END (ref-contract.md section 4).
    pub fn to_gz_ref(&self) -> String {
        format!(
            "gz://blob/{}#B{}-{}",
            self.blob_hash.to_hex(),
            self.start,
            self.end
        )
    }

    /// Extract the spanned bytes from the original blob.
    pub fn slice_blob<'a>(&self, blob: &'a [u8]) -> Option<&'a [u8]> {
        blob.get(self.start as usize..self.end as usize)
    }
}

/// A symbol node extracted from a definition (FR-004).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolNode {
    /// Unique index within this BlobFacts (0-based).
    pub id: u32,
    pub name: String,
    pub kind: NodeKind,
    /// Byte span of the definition name identifier.
    pub span_start: u32,
    pub span_end: u32,
    /// Full definition node extent (`@def_node`); equals identifier span when unknown.
    pub block_start: u32,
    pub block_end: u32,
}

/// Lightweight path node for import edges (ADR-006).
/// Not a full SymbolNode because import targets have no content hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathNode {
    pub id: u32,
    pub path: String,
}

fn probe_binary_version(binary: &str) -> Result<String, String> {
    match Command::new(binary).arg("--version").output() {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(if stdout.is_empty() {
                String::from_utf8_lossy(&output.stderr).trim().to_string()
            } else {
                stdout
            })
        }
        Ok(output) => Err(format!(
            "{binary} --version exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(err) => Err(format!("failed to execute {binary}: {err}")),
    }
}

fn find_enclosing_caller(nodes: &[SymbolNode], span_start: u32) -> u32 {
    debug_assert!(
        nodes
            .windows(2)
            .all(|pair| pair[0].block_start <= pair[1].block_start),
        "find_enclosing_caller requires nodes sorted by block_start"
    );
    let idx = nodes.partition_point(|node| node.block_start <= span_start);
    idx.checked_sub(1).map_or(FILE_NODE_ID, |i| nodes[i].id)
}

fn ensure_external_target(path_nodes: &mut Vec<PathNode>, resolved: &str) -> u32 {
    if let Some(existing) = path_nodes.iter().find(|node| node.path == resolved) {
        return existing.id;
    }
    let id = PATH_NODE_OFFSET + path_nodes.len() as u32;
    path_nodes.push(PathNode {
        id,
        path: resolved.to_string(),
    });
    id
}

fn supersede_structural_edge(
    edges: &mut Vec<Edge>,
    kind: EdgeKind,
    start: u32,
    end: u32,
    typed_source: Source,
) -> (usize, Source) {
    let before = edges.len();
    edges.retain(|edge| {
        !(edge.kind == kind
            && edge.evidence.start == start
            && edge.evidence.end == end
            && edge.source == Source::TreeSitter)
    });
    let superseded = before - edges.len();
    let source = if superseded > 0 {
        Source::Both
    } else {
        typed_source
    };
    (superseded, source)
}

/// An extracted edge (FR-005 through FR-009, FR-010, FR-011).
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub src: u32,
    pub dst: u32,
    pub kind: EdgeKind,
    pub confidence: f64,
    pub source: Source,
    pub evidence: EvidenceRef,
}

/// Facts extracted from a single blob (the primary output of Tier-A).
#[derive(Clone, Debug, PartialEq)]
pub struct BlobFacts {
    pub blob_hash: ContentHash,
    pub language: Language,
    pub parse_ok: bool,
    pub nodes: Vec<SymbolNode>,
    pub path_nodes: Vec<PathNode>,
    pub edges: Vec<Edge>,
}

/// Supported languages for Tier-A extraction (FR-002, A-001).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    TypeScript,
    Python,
    Unknown,
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Language::Rust => write!(f, "rust"),
            Language::TypeScript => write!(f, "typescript"),
            Language::Python => write!(f, "python"),
            Language::Unknown => write!(f, "unknown"),
        }
    }
}

/// Input to the extraction pipeline.
pub struct BlobInput<'a> {
    pub path_hint: Option<&'a str>,
    pub content: &'a [u8],
    pub hash: ContentHash,
}

impl<'a> BlobInput<'a> {
    pub fn new(path_hint: Option<&'a str>, content: &'a [u8]) -> Self {
        let hash = ContentHash::of(content);
        Self {
            path_hint,
            content,
            hash,
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/graphzero/unit/graphzero-extract/lib_public_api_tests.rs"]
mod public_api_tests;
