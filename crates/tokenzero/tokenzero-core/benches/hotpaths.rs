//! Microbenchmarks for TokenZero core hot-path functions.
//!
//! Keep-gate invocation (never size-optimized `--release`):
//! `cargo bench -p tokenzero-core --bench hotpaths --profile release-perf`
//! Default `cargo bench` uses `[profile.bench]` which inherits `release-perf`.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use tokenzero_core::*;

fn bench_count_tokens(c: &mut Criterion) {
    let small = "hello world";
    let medium = "fn render_shell(input: ShellRenderInput<'_>) -> ShellRender { \
                  let status = classify_command_status(input.command, input.stdout, \
                  input.stderr, input.exit_code, input.timed_out);";
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

fn bench_id_for(c: &mut Criterion) {
    let data_short = "benchmark test data";
    let data_long = &"abcdefghijklmnopqrstuvwxyz".repeat(100);

    let mut group = c.benchmark_group("id_for");
    group.bench_with_input(BenchmarkId::new("19B", ""), data_short, |b, text| {
        b.iter(|| black_box(id_for('b', text)))
    });
    group.bench_with_input(BenchmarkId::new("2.6KB", ""), data_long, |b, text| {
        b.iter(|| black_box(id_for('b', text)))
    });
    group.finish();
}

fn bench_render_shell(c: &mut Criterion) {
    let command = "cargo test --workspace -- --test-threads=4";
    let stdout_ok = "running 105 tests\n...\ntest result: ok. 105 passed; 0 failed; 0 ignored\n";
    let stderr_empty = "";
    c.bench_function("render_shell_tiny_success", |b| {
        b.iter(|| {
            let i = ShellRenderInput {
                command,
                stdout: stdout_ok,
                stderr: stderr_empty,
                exit_code: Some(0),
                timed_out: false,
                mode: Mode::Auto,
                max_visible_tokens: 2000,
                stdout_ref: None,
                stderr_ref: None,
                combined_ref: None,
            };
            black_box(render_shell(i))
        })
    });

    // Large diagnostic output (failed build)
    let stdout_fail = "error[".repeat(50);
    let stderr_fail = "error: could not compile `tokenzero-core`\n".repeat(20);

    c.bench_function("render_shell_failed_build", |b| {
        b.iter(|| {
            let i = ShellRenderInput {
                command: "cargo build",
                stdout: &stdout_fail,
                stderr: &stderr_fail,
                exit_code: Some(101),
                timed_out: false,
                mode: Mode::Auto,
                max_visible_tokens: 2000,
                stdout_ref: Some("tz://blob/abc123"),
                stderr_ref: Some("tz://blob/def456"),
                combined_ref: Some("tz://blob/ghi789"),
            };
            black_box(render_shell(i))
        })
    });
}

fn bench_dedupe_lines(c: &mut Criterion) {
    // Repeated build output with lots of duplication
    let repeated = "Compiling tokenzero-core v0.1.1\nwarning: unused variable\nwarning: unused variable\nCompiling tokenzero-mcp v0.1.1\nwarning: unused variable\nCompiling tokenzero-runtime v0.1.1\n".repeat(10);

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
    let data = "function render_shell(input) { return processOutput(input); }";
    c.bench_function("sha256_hex", |b| {
        b.iter(|| black_box(sha256_hex(black_box(data))))
    });
}

fn bench_enforce_token_budget(c: &mut Criterion) {
    // Adversarial workload: a 5000-line over-budget output that exercises the
    // per-line loop in `enforce_token_budget`. With the prior O(n^2) shape
    // this scales super-linearly with line count; the O(n) rewrite should
    // flatten the curve.
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
    bench_id_for,
    bench_render_shell,
    bench_dedupe_lines,
    bench_mask_visible_secrets,
    bench_sha256_hex,
    bench_enforce_token_budget,
);
criterion_main!(benches);
