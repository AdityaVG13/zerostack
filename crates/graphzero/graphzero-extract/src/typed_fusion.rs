//! Typed-edge fusion hook for the production extraction path. Tree-sitter extraction stays the
//! baseline.

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
