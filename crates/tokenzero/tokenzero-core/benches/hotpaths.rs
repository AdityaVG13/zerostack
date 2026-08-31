//! TokenZero core hot-path microbenchmarks.
//! Run with `cargo bench -p tokenzero-core --bench hotpaths --profile release-perf`.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use tokenzero_core::*;

fn bench_count_tokens(c: &mut Criterion) {
    let small = "hello world";
    let medium = "fn summarize_output(input: &str, budget: usize) -> String { \
                  enforce_token_budget(input, budget)";
    let large = "x ".repeat(5000);

    let mut group = c.benchmark_group("count_tokens");
    group.bench_with_input(BenchmarkId::new("11B", ""), small, |b, text| {
        b.iter(|| black_box(count_tokens(text)))
    });
    group.bench_with_input(BenchmarkId::new("173B", ""), medium, |b, text| {
        b.iter(|| black_box(count_tokens(text)))
    });
    group.bench_with_input(BenchmarkId::new("10KB", ""), &large, |b, text| {
        b.iter(|| black_box(count_tokens(text)))
    });
    group.finish();
}

fn bench_dedupe_lines(c: &mut Criterion) {
    // Repeated build output with lots of duplication
    let repeated = "Compiling tokenzero-core v0.1.0\nwarning: unused variable\nwarning: unused variable\nCompiling zero-token v0.1.0\nwarning: unused variable\nCompiling zero-kernel v0.1.0\n".repeat(10);

    c.bench_function("dedupe_lines_repeated", |b| {
        b.iter(|| black_box(dedupe_lines(black_box(&repeated), 6)))
    });
}

fn bench_mask_visible_secrets(c: &mut Criterion) {
    let with_secrets =
        "export API_KEY=sk-1234567890abcdef\nexport TOKEN=ghp_abcdefghijklmnop\nhello world";
    c.bench_function("mask_visible_secrets", |b| {
        b.iter(|| black_box(mask_visible_secrets(black_box(with_secrets))))
    });
}

fn bench_sha256_hex(c: &mut Criterion) {
    let data = "function summarizeOutput(input) { return compact(input); }";
    c.bench_function("sha256_hex", |b| {
        b.iter(|| black_box(sha256_hex(black_box(data))))
    });
}

fn bench_enforce_token_budget(c: &mut Criterion) {
    // Adversarial workload: a 5000-line over-budget output that exercises the
    // per-line loop in `enforce_token_budget`. With the prior O(n^2) shape this
    // scales super-linearly with line count; the O(n) rewrite should flatten the curve.
    let line = "fn example_function_call(arg_one: usize, arg_two: usize) -> usize {\n";
    let large: String = line.repeat(5000);
    let mut group = c.benchmark_group("enforce_token_budget");
    group.bench_with_input(BenchmarkId::new("5000_lines", ""), &large, |b, text| {
        b.iter(|| black_box(enforce_token_budget(black_box(text), black_box(800))))
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_count_tokens,
    bench_dedupe_lines,
    bench_mask_visible_secrets,
    bench_sha256_hex,
    bench_enforce_token_budget,
);
criterion_main!(benches);
