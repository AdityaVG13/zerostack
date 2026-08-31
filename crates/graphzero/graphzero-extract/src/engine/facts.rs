//! Fact assembly: BlobInput → BlobFacts.

use crate::detect::detect_language;
use crate::queries::QuerySet;
use crate::{BlobFacts, BlobInput, Language, PathNode, SymbolNode};
use std::collections::BTreeMap;

use super::extract::run_tier_a_extractors;
use super::parse::parse_blob_tree;

/// Mutable state accumulated during extraction of one blob.
pub(super) struct ExtractionState {
    pub(super) nodes: Vec<SymbolNode>,
    pub(super) path_nodes: Vec<PathNode>,
    pub(super) edges: Vec<crate::Edge>,
    pub(super) name_to_ids: BTreeMap<String, Vec<u32>>,
}

impl ExtractionState {
    pub(super) fn new() -> Self {
        Self {
            nodes: Vec::new(),
            path_nodes: Vec::new(),
            edges: Vec::new(),
            name_to_ids: BTreeMap::new(),
        }
    }
}

fn empty_facts(hash: crate::ContentHash, lang: Language) -> BlobFacts {
    BlobFacts {
        blob_hash: hash,
        language: lang,
        parse_ok: false,
        nodes: Vec::new(),
        path_nodes: Vec::new(),
        edges: Vec::new(),
    }
}

/// Extract Tier-A facts from a single blob.
pub fn extract_tier_a(input: &BlobInput, queries: &QuerySet) -> BlobFacts {
    let lang = input
        .path_hint
        .map(detect_language)
        .unwrap_or(Language::Unknown);

    if lang == Language::Unknown {
        return empty_facts(input.hash, Language::Unknown);
    }

    let Some(tree) = parse_blob_tree(input, lang) else {
        return empty_facts(input.hash, lang);
    };

    let Some(lang_queries) = queries.for_language(lang) else {
        return empty_facts(input.hash, lang);
    };

    let mut state = ExtractionState::new();
    run_tier_a_extractors(&tree, input, lang_queries, lang, &mut state);

    BlobFacts {
        blob_hash: input.hash,
        language: lang,
        parse_ok: true,
        nodes: state.nodes,
        path_nodes: state.path_nodes,
        edges: state.edges,
    }
}

/// Batch extraction using Rayon.
pub fn extract_batch(inputs: &[BlobInput], queries: &QuerySet) -> Vec<BlobFacts> {
    use rayon::prelude::*;
    inputs
        .par_iter()
        .map(|input| extract_tier_a(input, queries))
        .collect()
}
