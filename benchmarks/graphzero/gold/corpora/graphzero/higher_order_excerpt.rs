// Excerpt mirroring GraphZero's extraction fan-out
// (crates/graphzero-extract/src/engine.rs): iterator adapters and function
// pointers hide call targets from a call_expression-only structural pass.
use crate::detect::detect_language;

pub struct Blob {
    path: String,
}

fn normalize(path: &str) -> String {
    path.trim().to_string()
}

fn score(blob: &Blob) -> usize {
    blob.path.len()
}

fn is_indexable(blob: &Blob) -> bool {
    score(blob) > 0
}

pub fn plan(blobs: Vec<Blob>) -> Vec<usize> {
    blobs
        .iter()
        .filter(|blob| is_indexable(blob))
        .map(score)
        .collect()
}

pub fn normalize_all(paths: Vec<String>) -> Vec<String> {
    paths.iter().map(|p| normalize(p)).collect()
}

pub fn dispatch(f: fn(&Blob) -> usize, blob: &Blob) -> usize {
    f(blob)
}

pub fn dispatch_score(blob: &Blob) -> usize {
    dispatch(score, blob)
}

pub fn language_of(blob: &Blob) -> u32 {
    detect_language(&blob.path)
}
