//! Batch planner: fusion, dedupe (CSE), and cost-based scheduling (fszero-h46w).
//!
//! Partial planner (acceptance slice):
//! 1. CSE: identical items share one execution (`dedupe_execute`).
//! 2. Fusion: text-search + AST-search over the same path universe → one visit plan.
//! 3. Cost schedule: tiny warm → inline; medium → worker pool; large scans → partitioned.

use std::collections::{BTreeSet, HashMap};

/// Dedupe identical batch items by identity key; returns shared result slots.
///
/// `run` is invoked once per unique key; duplicate items reuse the same `R`.
pub fn dedupe_execute<K, R, F>(items: Vec<K>, mut run: F) -> Vec<R>
where
    K: std::hash::Hash + Eq + Clone,
    R: Clone,
    F: FnMut(&K) -> R,
{
    let mut cache: HashMap<K, R> = HashMap::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if let Some(r) = cache.get(&item) {
            out.push(r.clone());
            continue;
        }
        let r = run(&item);
        cache.insert(item, r.clone());
        out.push(r);
    }
    out
}

/// Execution shape chosen by cost (not blind parallelism).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecShape {
    /// Tiny warm ops: single core, no pool hop.
    Inline,
    /// Medium independent ops: persistent worker pool.
    WorkerPool,
    /// Large scans: partition files across cores.
    Partitioned,
}

impl ExecShape {
    pub fn as_str(self) -> &'static str {
        match self {
            ExecShape::Inline => "inline",
            ExecShape::WorkerPool => "worker_pool",
            ExecShape::Partitioned => "partitioned",
        }
    }
}

/// Thresholds are intentionally conservative; measure and adjust later.
const INLINE_MAX_ITEMS: usize = 8;
const INLINE_MAX_FILES: usize = 32;
const PARTITION_MIN_FILES: usize = 256;

/// Choose execution shape from batch size, estimated file fanout, and warm cache hint.
pub fn choose_exec_shape(item_count: usize, estimated_files: usize, warm_hint: bool) -> ExecShape {
    if estimated_files >= PARTITION_MIN_FILES {
        return ExecShape::Partitioned;
    }
    // Tiny batches stay inline even on cold paths -- pool hop dominates.
    if item_count <= INLINE_MAX_ITEMS && estimated_files <= INLINE_MAX_FILES {
        return ExecShape::Inline;
    }
    let _ = warm_hint; // reserved for finer warm-vs-cold tuning
    ExecShape::WorkerPool
}

/// Fusion plan for text-search and AST-search sharing a path universe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FusionPlan {
    /// Sorted unique paths (empty means "whole index").
    pub path_union: Vec<String>,
    pub text_queries: usize,
    pub ast_queries: usize,
    /// True when both text and AST share this plan (single visit).
    pub fused: bool,
}

/// Build a fused path visit plan from per-query path filters.
/// Empty path filter means unconstrained (whole corpus).
pub fn plan_search_ast_fusion(
    text_path_filters: &[Vec<String>],
    ast_path_filters: &[Vec<String>],
) -> FusionPlan {
    let text_queries = text_path_filters.len();
    let ast_queries = ast_path_filters.len();
    let mut union = BTreeSet::new();
    let mut any_unconstrained = false;
    for filters in text_path_filters.iter().chain(ast_path_filters.iter()) {
        if filters.is_empty() {
            any_unconstrained = true;
        } else {
            for p in filters {
                union.insert(p.clone());
            }
        }
    }
    let path_union = if any_unconstrained {
        Vec::new()
    } else {
        union.into_iter().collect()
    };
    FusionPlan {
        path_union,
        text_queries,
        ast_queries,
        fused: text_queries > 0 && ast_queries > 0,
    }
}

/// Count unique CSE keys for instrumentation (unique_inputs-style).
pub fn cse_unique_count<K: std::hash::Hash + Eq>(keys: &[K]) -> usize {
    let mut seen = HashMap::new();
    for k in keys {
        seen.entry(k).or_insert(());
    }
    seen.len()
}
