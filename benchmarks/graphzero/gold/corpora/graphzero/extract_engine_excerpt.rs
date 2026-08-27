use crate::detect::detect_language;
use crate::queries::QuerySet;
use crate::{BlobFacts, BlobInput, Language};

fn parse_blob_tree(input: &BlobInput, lang: Language) -> Option<()> {
    let _ = (input, lang);
    Some(())
}

fn run_tier_a_extractors() {}

pub fn extract_tier_a(input: &BlobInput, queries: &QuerySet) -> BlobFacts {
    let lang = input.path_hint.map(detect_language).unwrap_or(Language::Unknown);
    let _tree = parse_blob_tree(input, lang);
    run_tier_a_extractors();
    todo!()
}
