//! Shared release-gate helpers: ref-first accounting and JSON artifacts.

use std::fs;
use std::path::PathBuf;

use graphzero_store::store::query::tokens_for_str;
use serde_json::{Value, json};

pub const REF_FIRST_BUDGET: usize = 1;

pub const REF_FIRST_MAX_VISIBLE_TOKENS: usize = 80;

pub fn record_step(
    steps: &mut Vec<Value>,
    name: &str,
    body: &str,
    total_bytes: &mut usize,
    total_tokens: &mut usize,
) {
    record_bytes_step(steps, name, body.as_bytes(), total_bytes, total_tokens);
}

pub fn record_bytes_step(
    steps: &mut Vec<Value>,
    name: &str,
    body: &[u8],
    total_bytes: &mut usize,
    total_tokens: &mut usize,
) {
    let b = body.len();
    let t = body.len().div_ceil(4);
    *total_bytes += b;
    *total_tokens += t;
    steps.push(json!({
        "step": name,
        "bytes": b,
        "visible_tokens": t,
    }));
}

/// Budget=1 responses must not inline large payloads; they spill to gz://query or
/// expose compact gz://blob|node evidence refs (orient surfaces).
pub fn assert_ref_first(step: &str, body: &str, budget: usize) {
    if budget > 1 {
        return;
    }
    let has_ref = body.starts_with("g:")
        || body.starts_with("q:")
        || body.contains("gz://query/")
        || body.contains("gz://q/")
        || body.contains("\"full_ref\"")
        || body.contains("gz://blob/")
        || body.contains("gz://node/");
    assert!(
        has_ref,
        "{step} must carry gz:// evidence at budget=1: {body}"
    );
    let visible = tokens_for_str(body);
    assert!(
        visible <= REF_FIRST_MAX_VISIBLE_TOKENS,
        "{step} visible capsule too large at budget=1: {visible} tokens"
    );
    assert!(
        !body.contains("\"matches\":["),
        "{step} must not inline match arrays at budget=1"
    );
}

pub fn write_benchmark_artifact(subdir: &str, filename: &str, report: &Value) -> PathBuf {
    // Write to target/ (ignored by git) instead of source-controlled benchmarks/
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/gate-artifacts")
        .join(subdir);
    fs::create_dir_all(&out_dir).expect("failed to create benchmark artifact directory");
    let path = out_dir.join(filename);
    fs::write(
        &path,
        serde_json::to_string_pretty(report)
            .expect("failed to serialize benchmark report as pretty JSON"),
    )
    .expect("failed to write benchmark report artifact");
    path
}
