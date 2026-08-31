//! tsserver typed-edge adapter. The adapter consumes resolved TypeScript
//! server call/import targets from an LSP-side driver and enriches
//! tree-sitter facts without making tsserver a mandatory extraction dependency.

use crate::confidence_band;
use crate::{
    BlobFacts, Edge, EdgeKind, EvidenceRef, FILE_NODE_ID, Language, Source, SymbolNode,
    ensure_external_target, find_enclosing_caller, probe_binary_version, supersede_structural_edge,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TsServerResolutionConfidence {
    Exact,
    InterfaceDispatch,
    ReExport,
    Inferred,
}

impl TsServerResolutionConfidence {
    pub fn as_f64(self) -> f64 {
        match self {
            TsServerResolutionConfidence::Exact => confidence_band::TSSERVER_EXACT,
            TsServerResolutionConfidence::InterfaceDispatch => confidence_band::TSSERVER_INTERFACE,
            TsServerResolutionConfidence::ReExport => confidence_band::TSSERVER_REEXPORT,
            TsServerResolutionConfidence::Inferred => confidence_band::TSSERVER_INFERRED,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            TsServerResolutionConfidence::Exact => "exact",
            TsServerResolutionConfidence::InterfaceDispatch => "interface-dispatch",
            TsServerResolutionConfidence::ReExport => "re-export",
            TsServerResolutionConfidence::Inferred => "inferred",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TsServerResolvedEdgeKind {
    Call,
    Import,
}

impl TsServerResolvedEdgeKind {
    fn edge_kind(self) -> EdgeKind {
        match self {
            TsServerResolvedEdgeKind::Call => EdgeKind::Calls,
            TsServerResolvedEdgeKind::Import => EdgeKind::Imports,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TsServerResolvedEdge {
    pub span_start: u32,
    pub span_end: u32,
    pub resolved_target: String,
    pub resolved_source: Option<String>,
    pub kind: TsServerResolvedEdgeKind,
    pub confidence: TsServerResolutionConfidence,
}

impl TsServerResolvedEdge {
    pub fn call(
        span_start: u32,
        span_end: u32,
        resolved_target: impl Into<String>,
        confidence: TsServerResolutionConfidence,
    ) -> Self {
        Self {
            span_start,
            span_end,
            resolved_target: resolved_target.into(),
            resolved_source: None,
            kind: TsServerResolvedEdgeKind::Call,
            confidence,
        }
    }

    pub fn import(
        span_start: u32,
        span_end: u32,
        resolved_target: impl Into<String>,
        confidence: TsServerResolutionConfidence,
    ) -> Self {
        Self {
            span_start,
            span_end,
            resolved_target: resolved_target.into(),
            resolved_source: None,
            kind: TsServerResolvedEdgeKind::Import,
            confidence,
        }
    }

    pub fn with_source(mut self, resolved_source: impl Into<String>) -> Self {
        self.resolved_source = Some(resolved_source.into());
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TsServerAdapterReport {
    pub attempted: usize,
    pub applied: usize,
    pub skipped_non_typescript: usize,
    pub skipped_invalid_span: usize,
    pub superseded_structural: usize,
    pub fused: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TsServerLiveStatus {
    pub available: bool,
    pub binary: String,
    pub version: Option<String>,
    pub reason: Option<String>,
}

impl TsServerLiveStatus {
    fn available(binary: String, version: String) -> Self {
        Self {
            available: true,
            binary,
            version: Some(version),
            reason: None,
        }
    }

    fn unavailable(binary: String, reason: String) -> Self {
        Self {
            available: false,
            binary,
            version: None,
            reason: Some(reason),
        }
    }
}

/// Probe the real TypeScript language server executable used for live tsserver-backed enrichment.
/// Contract fixtures call apply_tsserver_edges directly with synthetic resolutions.
pub fn probe_live_tsserver() -> TsServerLiveStatus {
    let binary = std::env::var("GRAPHZERO_TSSERVER_BIN")
        .unwrap_or_else(|_| "typescript-language-server".to_string());
    probe_live_tsserver_binary(binary)
}

/// Spawn contract: the executable and `--version` are discrete argv entries; the probe inherits the
/// caller's environment, blocks until the diagnostic command exits, and reports spawn failures or
/// non-zero status with the exact executable name rather than treating missing output as success.
fn probe_live_tsserver_binary(binary: String) -> TsServerLiveStatus {
    match probe_binary_version(&binary) {
        Ok(version) => TsServerLiveStatus::available(binary, version),
        Err(reason) => TsServerLiveStatus::unavailable(binary, reason),
    }
}

pub fn apply_tsserver_edges(
    facts: &mut BlobFacts,
    edges: &[TsServerResolvedEdge],
) -> TsServerAdapterReport {
    let mut report = TsServerAdapterReport {
        attempted: edges.len(),
        ..TsServerAdapterReport::default()
    };

    if facts.language != Language::TypeScript {
        report.skipped_non_typescript = edges.len();
        return report;
    }

    for resolved in edges {
        if resolved.span_start >= resolved.span_end {
            report.skipped_invalid_span += 1;
            continue;
        }

        let kind = resolved.kind.edge_kind();
        let src = resolved
            .resolved_source
            .as_deref()
            .and_then(|source| find_symbol_id(&facts.nodes, source))
            .unwrap_or_else(|| default_source(&facts.nodes, resolved.span_start, kind));
        let dst = find_symbol_id(&facts.nodes, &resolved.resolved_target).unwrap_or_else(|| {
            ensure_external_target(&mut facts.path_nodes, &resolved.resolved_target)
        });

        let (superseded, source) = supersede_structural_edge(
            &mut facts.edges,
            kind,
            resolved.span_start,
            resolved.span_end,
            Source::TypeScriptServer,
        );
        report.superseded_structural += superseded;
        report.fused += (superseded > 0) as usize;

        facts.edges.push(Edge {
            src,
            dst,
            kind,
            confidence: resolved.confidence.as_f64(),
            source,
            evidence: EvidenceRef::new_unchecked(
                facts.blob_hash,
                resolved.span_start,
                resolved.span_end,
            ),
        });
        report.applied += 1;
    }

    report
}

fn default_source(nodes: &[SymbolNode], span_start: u32, kind: EdgeKind) -> u32 {
    match kind {
        EdgeKind::Calls => find_enclosing_caller(nodes, span_start),
        _ => FILE_NODE_ID,
    }
}

fn find_symbol_id(nodes: &[SymbolNode], resolved: &str) -> Option<u32> {
    nodes
        .iter()
        .find(|node| symbol_matches(&node.name, resolved))
        .map(|node| node.id)
}

fn symbol_matches(local_name: &str, resolved: &str) -> bool {
    resolved == local_name
        || resolved
            .rsplit(['.', '/', '#'])
            .next()
            .is_some_and(|tail| tail == local_name)
}
