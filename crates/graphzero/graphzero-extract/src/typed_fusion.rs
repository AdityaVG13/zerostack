//! Typed-edge fusion hook for the production extraction path.
//!
//! Tree-sitter extraction stays the baseline. When a typed resolver backed by
//! rust-analyzer or tsserver is installed, its resolutions are fused into the
//! freshly extracted facts: structural edges at the same call site are
//! superseded by call-accurate typed edges, and typed edges the structural pass
//! never saw are added. With no resolver installed the pipeline is
//! structural-only and produces exactly the same facts as before.
//!
//! # Concurrency contract (graphzero-bw60k)
//!
//! `install_typed_resolver` stores a process-wide [`Arc<dyn TypedResolver>`].
//! Index extract may call [`fuse_installed_typed_edges`] from many rayon
//! workers. Resolvers that wrap a single LSP subprocess (e.g.
//! [`crate::rust_analyzer_lsp::RustAnalyzerLspResolver`]) must document that
//! they serialize under an internal mutex: wall time with fusion installed is
//! closer to sequential blob count × per-blob LSP cost, not structural-only
//! parallel extract.
//!
//! Measure before promoting fusion to default:
//! - structural-only `extract_ms` on a multi-file chunk (rayon parallel)
//! - fusion-installed `extract_ms` with the same chunk (expect ~serial LSP)
//! Prefer multi-client pool or serial extract when fusion is on by default.

use std::sync::{Arc, PoisonError, RwLock};

use crate::rust_analyzer::{RustAnalyzerResolvedCall, apply_rust_analyzer_calls};
use crate::tsserver::{TsServerResolvedEdge, apply_tsserver_edges};
use crate::{BlobFacts, Language};

/// Typed resolutions for one blob, produced by an LSP-backed resolver.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TypedResolutions {
    pub rust_calls: Vec<RustAnalyzerResolvedCall>,
    pub typescript_edges: Vec<TsServerResolvedEdge>,
}

/// Source of typed resolutions. Implementations own the LSP binary detection
/// and subprocess lifetime; returning empty resolutions is the correct
/// behaviour when the language server is unavailable.
pub trait TypedResolver: Send + Sync {
    fn resolve(
        &self,
        path_hint: Option<&str>,
        content: &[u8],
        facts: &BlobFacts,
    ) -> TypedResolutions;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TypedFusionReport {
    pub applied: usize,
    pub superseded_structural: usize,
}

static RESOLVER: RwLock<Option<Arc<dyn TypedResolver>>> = RwLock::new(None);

pub fn install_typed_resolver(resolver: Arc<dyn TypedResolver>) {
    *RESOLVER.write().unwrap_or_else(PoisonError::into_inner) = Some(resolver);
}

pub fn clear_typed_resolver() {
    *RESOLVER.write().unwrap_or_else(PoisonError::into_inner) = None;
}

pub fn typed_resolver() -> Option<Arc<dyn TypedResolver>> {
    RESOLVER
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Fuse resolutions from the process-wide resolver, if one is installed.
///
/// Returns `None` when running structural-only.
pub fn fuse_installed_typed_edges(
    facts: &mut BlobFacts,
    path_hint: Option<&str>,
    content: &[u8],
) -> Option<TypedFusionReport> {
    let resolver = typed_resolver()?;
    Some(fuse_typed_edges(
        facts,
        path_hint,
        content,
        resolver.as_ref(),
    ))
}

pub fn fuse_typed_edges(
    facts: &mut BlobFacts,
    path_hint: Option<&str>,
    content: &[u8],
    resolver: &dyn TypedResolver,
) -> TypedFusionReport {
    if !matches!(facts.language, Language::Rust | Language::TypeScript) {
        return TypedFusionReport::default();
    }

    let resolutions = resolver.resolve(path_hint, content, facts);
    match facts.language {
        Language::Rust => {
            let report = apply_rust_analyzer_calls(facts, &resolutions.rust_calls);
            TypedFusionReport {
                applied: report.applied,
                superseded_structural: report.superseded_structural,
            }
        }
        Language::TypeScript => {
            let report = apply_tsserver_edges(facts, &resolutions.typescript_edges);
            TypedFusionReport {
                applied: report.applied,
                superseded_structural: report.superseded_structural,
            }
        }
        _ => TypedFusionReport::default(),
    }
}
