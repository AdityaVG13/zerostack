//! Shared query/snap/capsule types.

use std::path::Path;

use super::super::delta_log::DeltaEntry;
use serde_json; // for ExportArtifact meta (skeleton)

/// P1.1 snap query route (ADR-011).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapRoute {
    Symbol,
    Trigram,
    Semantic,
    Hot,
    Changes,
}

impl SnapRoute {
    pub fn as_str(self) -> &'static str {
        match self {
            SnapRoute::Symbol => "symbol",
            SnapRoute::Trigram => "trigram",
            SnapRoute::Semantic => "semantic",
            SnapRoute::Hot => "hot",
            SnapRoute::Changes => "changes",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RouteDiagnostics {
    pub symbol_route: Option<&'static str>,
    pub degraded_tiers: Vec<&'static str>,
    pub notes: Vec<String>,
    /// Destinations dropped because the session already returned them (FR-007).
    pub removed_count: usize,
    /// Subset of removals that were identical-byte hits.
    pub byte_deduped: usize,
    /// Subset of removals that were cross-view entity hits.
    pub entity_deduped: usize,
}

#[derive(Clone, Debug)]
pub struct DestinationRef {
    pub destination_ref: String,
    pub evidence_ref: String,
    pub label: String,
    pub path: Option<String>,
    /// Canonical snap-to-file target `<path>#L<start>-L<end>` (bead 5htnw),
    /// same grammar as FSZero `docs/design/target-ref-grammar.md`.
    pub target: Option<String>,
    /// Intent metadata: which query produced this hit (`def`/`ref`/`blast`).
    pub kind: Option<String>,
    /// Enclosing symbol at the target span.
    pub symbol: Option<String>,
    /// Inlined content window for top hits (`| <line-no>: <text>` per line).
    pub content: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CoverageCertificate {
    pub tier_a: f64,
    pub tier_b: f64,
    pub tier_c: f64,
    /// P5.1 semantic index coverage percent (sidecar presence walking skeleton).
    pub semantic_tier_percent: f64,
    pub freshness_verified: bool,
}

#[derive(Clone, Debug)]
pub struct BudgetLedger {
    pub requested_budget: usize,
    pub used_budget: usize,
    pub remaining_budget: usize,
    pub truncated: bool,
    pub omitted_count: usize,
}

#[derive(Clone, Debug)]
pub struct QueryCapsule {
    pub schema_version: u32,
    pub query: String,
    pub budget: usize,
    pub route: SnapRoute,
    pub destinations: Vec<DestinationRef>,
    pub coverage: CoverageCertificate,
    pub diagnostics: RouteDiagnostics,
    pub ledger: BudgetLedger,
    pub snapshot_id: u64,
}

pub fn normalize_snap_query(query: &str) -> String {
    query.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Debug)]
pub struct PathRecord {
    pub mtime_nanos: u128,
    pub size: u64,
    pub tier_bits: u8,
    pub path: String,
}

/// Local freshness telemetry for cold queries (P2.1 FR-007).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FreshnessDiagnostics {
    pub check_freshness: bool,
    pub events: Vec<String>,
    pub hash_checks: usize,
    pub reextract_count: usize,
}

/// (name, blob hash, span start, span end)
pub type PendingDef = (String, [u8; 32], u32, u32);
/// (src, dst, kind, confidence, blob hash, span start, span end, optional source)
pub type PendingEdge = (String, String, u8, u8, [u8; 32], u32, u32, Option<String>);

/// Facts merged from wal segments not yet folded into the snapshot
/// (crash recovery and worktree overlays share this shape).
#[derive(Clone, Debug, Default)]
pub struct PendingFacts {
    pub defs: Vec<PendingDef>,
    pub edges: Vec<PendingEdge>,
    /// blob hash -> tier bits for blobs touched by pending entries.
    pub blobs: std::collections::BTreeMap<[u8; 32], u8>,
    /// blob hash -> repo-relative path (from BLOB entry payloads).
    pub paths: std::collections::BTreeMap<[u8; 32], String>,
}

impl PendingFacts {
    pub fn from_entries(entries: &[DeltaEntry]) -> Self {
        use super::super::delta_log::entry_type;
        use super::delta_codec::{decode_edge, decode_symbol};

        let mut out = Self::default();
        for e in entries {
            match e.entry_type {
                entry_type::SYMBOL => {
                    if let Some((name, kind_tier_span)) = decode_symbol(&e.payload) {
                        let (_kind, _tier, start, end) = kind_tier_span;
                        out.defs.push((name, e.blob_hash, start, end));
                        out.blobs.entry(e.blob_hash).or_insert(0b001);
                    }
                }
                entry_type::EDGE => {
                    if let Some((src, dst, kind, conf, start, end, source)) =
                        decode_edge(&e.payload)
                    {
                        out.edges
                            .push((src, dst, kind, conf, e.blob_hash, start, end, source));
                        out.blobs.entry(e.blob_hash).or_insert(0b001);
                    }
                }
                entry_type::COVERAGE => {
                    let bits = e.payload.first().copied().unwrap_or(0);
                    out.blobs.insert(e.blob_hash, bits);
                }
                entry_type::BLOB => {
                    out.blobs.entry(e.blob_hash).or_insert(0);
                    if let Ok(path) = std::str::from_utf8(&e.payload)
                        && !path.is_empty()
                    {
                        out.paths.insert(e.blob_hash, path.to_string());
                    }
                }
                _ => {}
            }
        }
        out
    }
}

#[derive(Clone, Debug)]
pub struct CapsuleDef {
    pub evidence_ref: String,
    pub path: Option<String>,
    pub stale: bool,
}

#[derive(Clone, Debug)]
pub struct CapsuleEdge {
    pub kind: u8,
    pub to: String,
    pub confidence: f64,
    pub evidence_ref: String,
    pub source: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CapsuleMatch {
    pub name: String,
    pub defs: Vec<CapsuleDef>,
    pub edges: Vec<CapsuleEdge>,
}

#[derive(Clone, Debug)]
pub struct Capsule {
    pub query: String,
    pub snapshot_id: u64,
    pub matches: Vec<CapsuleMatch>,
    pub tier_a: f64,
    pub tier_b: f64,
    pub tier_c: f64,
    pub budget: usize,
    pub freshness: FreshnessDiagnostics,
}

impl QueryCapsule {
    pub fn to_json(&self, store_root: Option<&Path>) -> String {
        super::capsule_json::query_capsule_to_json(self, store_root)
    }
}

/// Export formats for snap --to-file / --export (perf default: minimal).
/// minimal: tiny ref+meta (<512B target); capsule: full QueryCapsule json;
/// md: handoff markdown; zst: zstd-compressed capsule bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExportFormat {
    #[default]
    Minimal,
    Capsule,
    Md,
    Zst,
}

impl ExportFormat {
    /// Lossy parse: unknown spellings fall back to `Minimal`.
    pub fn parse_lossy(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "capsule" | "full" => ExportFormat::Capsule,
            "md" | "markdown" => ExportFormat::Md,
            "zst" | "zstd" | "compressed" => ExportFormat::Zst,
            _ => ExportFormat::Minimal,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            ExportFormat::Minimal => "minimal",
            ExportFormat::Capsule => "capsule",
            ExportFormat::Md => "md",
            ExportFormat::Zst => "zst",
        }
    }
}

/// Result of atomic export (path + meta for stdout/MCP; tiny overhead).
#[derive(Clone, Debug)]
pub struct ExportArtifact {
    pub path: std::path::PathBuf,
    pub size_bytes: u64,
    pub ref_str: String, // e.g. q:xxxx or gz://query/...
    pub format: ExportFormat,
}

impl ExportArtifact {
    pub fn to_meta_json(&self) -> String {
        serde_json::json!({
            "exported": self.path.display().to_string(),
            "size_bytes": self.size_bytes,
            "ref": self.ref_str,
            "format": self.format.as_str(),
        })
        .to_string()
    }
}
