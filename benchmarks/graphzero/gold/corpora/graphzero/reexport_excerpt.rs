// Excerpt mirroring GraphZero's crate facade
// (crates/graphzero-extract/src/lib.rs): pub use re-exports and cross-module
// paths that structural name matching cannot follow to a definition.
pub use crate::engine::extract_tier_a;
pub use crate::queries::QuerySet;
use crate::detect::detect_language;

pub struct Facade {
    queries: QuerySet,
}

impl Facade {
    pub fn new(queries: QuerySet) -> Self {
        Facade { queries }
    }

    pub fn run(&self, path: &str) -> u32 {
        let lang = detect_language(path);
        let _ = extract_tier_a(path, &self.queries);
        lang_code(lang)
    }
}

fn lang_code(lang: u32) -> u32 {
    lang
}

fn never_exported() -> u32 {
    0
}
