//! Language detection from file extension (FR-002).

use crate::Language;

fn extension_is_ts_js(lower: &str) -> bool {
    lower.ends_with(".ts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".js")
        || lower.ends_with(".jsx")
}

/// Detect the Tier-A language from a file path hint.
pub fn detect_language(path: &str) -> Language {
    let lower = path.to_lowercase();
    if lower.ends_with(".rs") {
        return Language::Rust;
    }
    if extension_is_ts_js(&lower) {
        return Language::TypeScript;
    }
    if lower.ends_with(".py") {
        return Language::Python;
    }
    Language::Unknown
}
