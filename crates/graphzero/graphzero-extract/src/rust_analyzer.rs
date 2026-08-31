//! rust-analyzer typed-call edge adapter. This module is deliberately optional:
//! tree-sitter extraction remains the baseline and callers may enrich Rust facts
//! with rust-analyzer/LSP resolved call targets when that side channel is available.

use crate::confidence_band;
use crate::{
    BlobFacts, Edge, EdgeKind, EvidenceRef, Language, Source, SymbolNode, ensure_external_target,
    find_enclosing_caller, probe_binary_version, supersede_structural_edge,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResolutionConfidence {
    Exact,
    TypeQualified,
    TraitDispatch,
    Inferred,
}

impl ResolutionConfidence {
    pub fn as_f64(self) -> f64 {
        match self {
            ResolutionConfidence::Exact => confidence_band::RUST_ANALYZER_EXACT_CALL,
            ResolutionConfidence::TypeQualified => {
                confidence_band::RUST_ANALYZER_TYPE_QUALIFIED_CALL
            }
            ResolutionConfidence::TraitDispatch => {
                confidence_band::RUST_ANALYZER_TRAIT_DISPATCH_CALL
            }
            ResolutionConfidence::Inferred => confidence_band::RUST_ANALYZER_INFERRED_CALL,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            ResolutionConfidence::Exact => "exact",
            ResolutionConfidence::TypeQualified => "type-qualified",
            ResolutionConfidence::TraitDispatch => "trait-dispatch",
            ResolutionConfidence::Inferred => "inferred",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RustAnalyzerResolvedCall {
    pub call_start: u32,
    pub call_end: u32,
    pub resolved_callee: String,
    pub resolved_caller: Option<String>,
    pub confidence: ResolutionConfidence,
}

impl RustAnalyzerResolvedCall {
    pub fn new(
        call_start: u32,
        call_end: u32,
        resolved_callee: impl Into<String>,
        confidence: ResolutionConfidence,
    ) -> Self {
        Self {
            call_start,
            call_end,
            resolved_callee: resolved_callee.into(),
            resolved_caller: None,
            confidence,
        }
    }

    pub fn with_caller(mut self, resolved_caller: impl Into<String>) -> Self {
        self.resolved_caller = Some(resolved_caller.into());
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RustAnalyzerAdapterReport {
    pub attempted: usize,
    pub applied: usize,
    pub skipped_non_rust: usize,
    pub skipped_invalid_span: usize,
    pub superseded_structural: usize,
    pub fused: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RustAnalyzerLiveStatus {
    pub available: bool,
    pub binary: String,
    pub version: Option<String>,
    pub reason: Option<String>,
}

impl RustAnalyzerLiveStatus {
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

/// Probe the real rust-analyzer executable used for live LSP enrichment. Contract fixtures call
/// apply_rust_analyzer_calls directly with synthetic resolutions.
pub fn probe_live_rust_analyzer() -> RustAnalyzerLiveStatus {
    let binary = std::env::var("GRAPHZERO_RUST_ANALYZER_BIN")
        .unwrap_or_else(|_| "rust-analyzer".to_string());
    probe_live_rust_analyzer_binary(binary)
}

/// Spawn contract: the executable and `--version` are discrete argv entries; the probe inherits the
/// caller's environment, blocks until the diagnostic command exits, and reports spawn failures or
/// non-zero status with the exact executable name rather than treating missing output as success.
fn probe_live_rust_analyzer_binary(binary: String) -> RustAnalyzerLiveStatus {
    match probe_binary_version(&binary) {
        Ok(version) => RustAnalyzerLiveStatus::available(binary, version),
        Err(reason) => RustAnalyzerLiveStatus::unavailable(binary, reason),
    }
}

pub fn apply_rust_analyzer_calls(
    facts: &mut BlobFacts,
    calls: &[RustAnalyzerResolvedCall],
) -> RustAnalyzerAdapterReport {
    let mut report = RustAnalyzerAdapterReport {
        attempted: calls.len(),
        ..RustAnalyzerAdapterReport::default()
    };

    if facts.language != Language::Rust {
        report.skipped_non_rust = calls.len();
        return report;
    }

    for call in calls {
        if call.call_start >= call.call_end {
            report.skipped_invalid_span += 1;
            continue;
        }

        let src = call
            .resolved_caller
            .as_deref()
            .and_then(|caller| find_symbol_id(&facts.nodes, caller))
            .unwrap_or_else(|| find_enclosing_caller(&facts.nodes, call.call_start));
        let dst = find_symbol_id(&facts.nodes, &call.resolved_callee).unwrap_or_else(|| {
            ensure_external_target(&mut facts.path_nodes, &call.resolved_callee)
        });

        let (superseded, source) = supersede_structural_edge(
            &mut facts.edges,
            EdgeKind::Calls,
            call.call_start,
            call.call_end,
            Source::RustAnalyzer,
        );
        report.superseded_structural += superseded;
        report.fused += (superseded > 0) as usize;

        facts.edges.push(Edge {
            src,
            dst,
            kind: EdgeKind::Calls,
            confidence: call.confidence.as_f64(),
            source,
            evidence: EvidenceRef::new_unchecked(facts.blob_hash, call.call_start, call.call_end),
        });
        report.applied += 1;
    }

    report
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
            .rsplit("::")
            .next()
            .is_some_and(|tail| tail == local_name)
}
