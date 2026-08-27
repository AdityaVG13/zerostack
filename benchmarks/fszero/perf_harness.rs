//! In-process perf scenarios for JeffreySkill profiling (measurement only).

use fs_zero::{FSZeroSession, RecoveryStore, codemode_execute_plan};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: perf_harness <scenario> [args...]");
        std::process::exit(2);
    }
    let scenario = args[1].as_str();
    match scenario {
        "index_build" => run_index_build(),
        "session_init" => {
            let root = args.get(2).map(|s| s.as_str()).unwrap_or(".");
            run_session_init(root);
        }
        "read_hot" => run_read_hot(args.get(2).map(|s| s.as_str())),
        "read_cold" => run_read_cold(args.get(2).map(|s| s.as_str())),
        "read_range" => run_read_range(args.get(2).map(|s| s.as_str())),
        "search_struct" => run_search("defs:main"),
        "search_grep" => run_search("fn main"),
        "resolve_empty" => run_resolve_empty(),
        "resolve_populated" => run_resolve_populated(),
        "readmany_50" => run_readmany_50(),
        "read_50_singles" => run_read_50_singles(),
        "verified_edit_ok" => run_verified_edit(true),
        "verified_edit_fail" => run_verified_edit(false),
        "durable_read" => run_durable_read(),
        "memory_read" => run_memory_read(),
        "codemode_trivial" => run_codemode_trivial(),
        "codemode_50step" => run_codemode_50step(),
        "mcp_cold_codemode" => run_mcp_cold_codemode(),
        "mcp_loop_50" => run_mcp_loop_50(args.get(2).map(|s| s.as_str()).unwrap_or("mcp")),
        "store_open_seed" => run_store_open_seed(&args),
        "store_open_measure" => run_store_open_measure(&args),
        "store_open_benchmark" => run_store_open_benchmark(&args),
        "store_retention_measure" => run_store_retention_measure(&args),
        "store_retention_benchmark" => run_store_retention_benchmark(&args),
        _ => {
            eprintln!("unknown scenario: {scenario}");
            std::process::exit(2);
        }
    }
}

fn proc_rss_bytes() -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmRSS:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|kib| kib.parse::<u64>().ok())
        })
        .unwrap_or(0)
        * 1024
}

fn run_store_open_seed(args: &[String]) {
    let db = PathBuf::from(args.get(2).expect("store_open_seed requires DB path"));
    let rows = args
        .get(3)
        .expect("store_open_seed requires row count")
        .parse::<usize>()
        .expect("row count must be an integer");
    if let Some(parent) = db.parent() {
        fs::create_dir_all(parent).expect("create store directory");
    }
    let _ = fs::remove_file(&db);
    let mut store = RecoveryStore::with_durable(&db);
    for chunk_start in (0..rows).step_by(10_000) {
        let began = store.begin_benchmark_batch();
        for row in chunk_start..rows.min(chunk_start + 10_000) {
            store
                .try_put_key(&format!("history/{row:09}"), b"x")
                .expect("seed payload");
        }
        store.end_benchmark_batch(began);
        if let Some(error) = store.take_store_error() {
            panic!("seed batch failed: {error}");
        }
    }
    drop(store);
    let validated =
        RecoveryStore::try_open_existing_durable_pub(&db).expect("validate seeded store");
    let (pack_rows, memory_rows) = validated.open_maintenance_rows_scanned();
    println!(
        "{}",
        json!({
            "phase": "seed", "row_count": rows,
            "validation_pack_rows_scanned": pack_rows,
            "validation_memory_rows_scanned": memory_rows
        })
    );
}

fn run_store_open_measure(args: &[String]) {
    if args.get(4).map(String::as_str) == Some("cpu_probe") {
        let started = Instant::now();
        let mut work = 0u64;
        while started.elapsed() < Duration::from_millis(100) {
            work = std::hint::black_box(work.wrapping_mul(31).wrapping_add(1));
        }
        println!("{}", json!({"phase": "cpu_probe", "work": work}));
        return;
    }
    let db = PathBuf::from(args.get(2).expect("store_open_measure requires DB path"));
    let rows = args
        .get(3)
        .expect("store_open_measure requires row count")
        .parse::<usize>()
        .expect("row count must be an integer");
    let baseline_rss_bytes = proc_rss_bytes();
    let started = Instant::now();
    let store = RecoveryStore::try_open_existing_durable_pub(&db).expect("reopen validated store");
    let open_wall_ns = started.elapsed().as_nanos();
    let (pack_rows, memory_rows) = store.open_maintenance_rows_scanned();
    println!(
        "{}",
        json!({
            "phase": "measure", "row_count": rows,
            "open_wall_ns_internal": open_wall_ns,
            "baseline_rss_bytes": baseline_rss_bytes,
            "open_pack_rows_scanned": pack_rows,
            "open_memory_rows_scanned": memory_rows,
            "payload_rows_scanned": pack_rows + memory_rows
        })
    );
}

fn child_proc_metrics(pid: u32) -> (u64, u64) {
    let rss = fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmRSS:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|kib| kib.parse::<u64>().ok())
        })
        .unwrap_or(0)
        * 1024;
    let cpu_ticks = fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| stat.rsplit_once(") ").map(|(_, fields)| fields.to_owned()))
        .and_then(|fields| {
            let mut fields = fields.split_whitespace();
            let user = fields.nth(11)?.parse::<u64>().ok()?;
            let system = fields.next()?.parse::<u64>().ok()?;
            Some(user + system)
        })
        .unwrap_or(0);
    (rss, cpu_ticks)
}

fn run_store_open_child(
    exe: &Path,
    db: &Path,
    rows: usize,
    cpu_probe: bool,
) -> Result<(serde_json::Value, u128, u64, u64), String> {
    let started = Instant::now();
    let mut command = Command::new(exe);
    command
        .arg("store_open_measure")
        .arg(db)
        .arg(rows.to_string());
    if cpu_probe {
        command.arg("cpu_probe");
    }
    let mut child = command
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn measure child: {error}"))?;
    let mut peak_rss = 0;
    let mut cpu_ticks = 0;
    let status = loop {
        let (rss, ticks) = child_proc_metrics(child.id());
        peak_rss = peak_rss.max(rss);
        cpu_ticks = cpu_ticks.max(ticks);
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("wait for measure child: {error}"))?
        {
            break status;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let wall_ns = started.elapsed().as_nanos();
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .expect("piped measure stdout")
        .read_to_string(&mut stdout)
        .map_err(|error| format!("read measure output: {error}"))?;
    if !status.success() {
        return Err(format!("measure child exited with {status}"));
    }
    let line = stdout
        .lines()
        .last()
        .ok_or_else(|| "measure child emitted no JSON".to_owned())?;
    let value =
        serde_json::from_str(line).map_err(|error| format!("parse measure JSON: {error}"))?;
    Ok((value, wall_ns, cpu_ticks, peak_rss))
}

fn percentile<T: Ord + Copy>(values: &[T], percentile: usize) -> T {
    assert!(
        !values.is_empty(),
        "percentile requires at least one sample"
    );
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let index = (percentile * (sorted.len() - 1) + 50) / 100;
    sorted[index.min(sorted.len() - 1)]
}

fn run_store_open_benchmark(args: &[String]) {
    const CPU_TICKS_PER_SECOND: u64 = 100;
    const MEASURED_RUNS: usize = 20;
    const RATIO_LIMIT: f64 = 3.0;
    const RSS_LIMIT_BYTES: u64 = 64 * 1024 * 1024;

    let output_dir = PathBuf::from(
        args.get(2)
            .expect("store_open_benchmark requires output directory"),
    );
    fs::create_dir_all(&output_dir).expect("create benchmark output directory");
    let exe = env::current_exe().expect("resolve perf_harness executable");
    let mut failures = Vec::new();
    let mut sizes = serde_json::Map::new();
    let mut wall_medians = Vec::new();
    let mut cpu_medians = Vec::new();
    let cpu_profiler_probe_ticks = if cfg!(target_os = "linux") {
        match run_store_open_child(&exe, Path::new("."), 0, true) {
            Ok((_, _, ticks, _)) if ticks > 0 => Some(ticks),
            Ok(_) => {
                failures.push("CPU profiler probe returned zero ticks".to_owned());
                None
            }
            Err(error) => {
                failures.push(format!("CPU profiler probe failed: {error}"));
                None
            }
        }
    } else {
        failures.push(format!(
            "CPU profiler unsupported on target OS {}",
            std::env::consts::OS
        ));
        None
    };

    for rows in [100_000usize, 1_000_000] {
        let work = output_dir.join(format!("store-open-{}-{rows}", std::process::id()));
        let db = work.join("store.sqlite3");
        fs::create_dir_all(&work).expect("create store benchmark directory");
        let seed = Command::new(&exe)
            .arg("store_open_seed")
            .arg(&db)
            .arg(rows.to_string())
            .output();
        let seed_ok = match seed {
            Ok(output) if output.status.success() => true,
            Ok(output) => {
                failures.push(format!("{rows}: seed child exited with {}", output.status));
                false
            }
            Err(error) => {
                failures.push(format!("{rows}: spawn seed child: {error}"));
                false
            }
        };

        let mut open_wall_ns = Vec::new();
        let mut process_wall_ns = Vec::new();
        let mut cpu_ns = Vec::new();
        let mut incremental_rss = Vec::new();
        let mut pack_scans = Vec::new();
        let mut memory_scans = Vec::new();
        let mut payload_scans = Vec::new();
        if seed_ok {
            for _ in 0..MEASURED_RUNS {
                match run_store_open_child(&exe, &db, rows, false) {
                    Ok((sample, wall, ticks, peak_rss)) => {
                        let baseline = sample["baseline_rss_bytes"].as_u64().unwrap_or(0);
                        open_wall_ns.push(
                            sample["open_wall_ns_internal"]
                                .as_u64()
                                .map(u128::from)
                                .unwrap_or(u128::MAX),
                        );
                        process_wall_ns.push(wall);
                        cpu_ns.push(
                            u128::from(ticks) * 1_000_000_000 / u128::from(CPU_TICKS_PER_SECOND),
                        );
                        incremental_rss.push(peak_rss.saturating_sub(baseline));
                        pack_scans.push(
                            sample["open_pack_rows_scanned"]
                                .as_u64()
                                .unwrap_or(u64::MAX),
                        );
                        memory_scans.push(
                            sample["open_memory_rows_scanned"]
                                .as_u64()
                                .unwrap_or(u64::MAX),
                        );
                        payload_scans
                            .push(sample["payload_rows_scanned"].as_u64().unwrap_or(u64::MAX));
                    }
                    Err(error) => failures.push(format!("{rows}: {error}")),
                }
            }
        }

        if open_wall_ns.len() == MEASURED_RUNS {
            let p50_wall = percentile(&open_wall_ns, 50);
            let p95_wall = percentile(&open_wall_ns, 95);
            let p99_wall = percentile(&open_wall_ns, 99);
            let p50_process_wall = percentile(&process_wall_ns, 50);
            let p95_process_wall = percentile(&process_wall_ns, 95);
            let p99_process_wall = percentile(&process_wall_ns, 99);
            let p50_cpu = percentile(&cpu_ns, 50);
            let p95_cpu = percentile(&cpu_ns, 95);
            let p99_cpu = percentile(&cpu_ns, 99);
            let p50_rss = percentile(&incremental_rss, 50);
            let p95_rss = percentile(&incremental_rss, 95);
            let p99_rss = percentile(&incremental_rss, 99);
            let max_rss = incremental_rss.iter().copied().max().unwrap_or(0);
            if payload_scans.iter().any(|&count| count != 0) {
                failures.push(format!("{rows}: reopen scanned payload rows"));
            }
            if max_rss > RSS_LIMIT_BYTES {
                failures.push(format!("{rows}: incremental RSS exceeded limit"));
            }
            wall_medians.push(p50_wall);
            if p50_cpu == 0 {
                failures.push(format!("{rows}: CPU p50 unavailable at tick resolution"));
            } else {
                cpu_medians.push(p50_cpu);
            }
            sizes.insert(
                rows.to_string(),
                json!({
                    "measured_runs": MEASURED_RUNS,
                    "p50_wall_ns": p50_wall,
                    "p95_wall_ns": p95_wall,
                    "p99_wall_ns": p99_wall,
                    "p50_process_wall_ns": p50_process_wall,
                    "p95_process_wall_ns": p95_process_wall,
                    "p99_process_wall_ns": p99_process_wall,
                    "p50_cpu_ns": p50_cpu,
                    "p95_cpu_ns": p95_cpu,
                    "p99_cpu_ns": p99_cpu,
                    "p50_incremental_rss_bytes": p50_rss,
                    "p95_incremental_rss_bytes": p95_rss,
                    "p99_incremental_rss_bytes": p99_rss,
                    "max_incremental_rss_bytes": max_rss,
                    "open_wall_ns": open_wall_ns,
                    "process_wall_ns": process_wall_ns,
                    "cpu_ns": cpu_ns,
                    "incremental_rss_bytes": incremental_rss,
                    "open_pack_rows_scanned": pack_scans,
                    "open_memory_rows_scanned": memory_scans,
                    "payload_rows_scanned": payload_scans
                }),
            );
        } else {
            failures.push(format!(
                "{rows}: completed {} of {MEASURED_RUNS} measure children",
                open_wall_ns.len()
            ));
        }
        if let Err(error) = fs::remove_dir_all(&work) {
            failures.push(format!("{rows}: clean benchmark DB: {error}"));
        }
    }

    let ratio = |values: &[u128]| -> f64 {
        let low = values.iter().copied().min().unwrap_or(0) as f64;
        let high = values.iter().copied().max().unwrap_or(0) as f64;
        high / low.max(1.0)
    };
    let wall_ratio = ratio(&wall_medians);
    let cpu_ratio = (cpu_medians.len() == 2).then(|| ratio(&cpu_medians));
    if wall_medians.len() == 2 && wall_ratio > RATIO_LIMIT {
        failures.push(format!("wall ratio {wall_ratio:.3} exceeded {RATIO_LIMIT}"));
    }
    if let Some(value) = cpu_ratio.filter(|value| *value > RATIO_LIMIT) {
        failures.push(format!("CPU ratio {value:.3} exceeded {RATIO_LIMIT}"));
    }
    let passed = failures.is_empty() && wall_medians.len() == 2 && cpu_medians.len() == 2;
    println!(
        "{}",
        json!({
            "sizes": sizes,
            "ratios": {"wall": wall_ratio, "cpu": cpu_ratio},
            "limits": {"ratio": RATIO_LIMIT, "incremental_rss_bytes": RSS_LIMIT_BYTES},
            "statistical_profile": {
                "minimum_measured_runs": MEASURED_RUNS,
                "measured_runs_per_size": MEASURED_RUNS,
                "warmup_runs": 0,
                "percentile_method": "nearest index after sorting a copy of ordered raw samples",
                "outlier_policy": "none; retain every ordered raw sample"
            },
            "cpu_profiler": {
                "available": cpu_profiler_probe_ticks.is_some(),
                "probe_ticks": cpu_profiler_probe_ticks,
                "target_os": std::env::consts::OS,
                "procfs_self_stat_available": Path::new("/proc/self/stat").is_file()
            },
            "cpu_clock_ticks_per_second": CPU_TICKS_PER_SECOND,
            "failures": failures,
            "passed": passed
        })
    );
    if !passed {
        std::process::exit(1);
    }
}

const RETENTION_SCHEMA: &str = "fszero.store_retention";
const RETENTION_IDLE_LIMIT_BYTES: u64 = 256 * 1024 * 1024;
const RETENTION_DEFAULT_ROWS: usize = 4_096;
const RETENTION_PAYLOAD_BYTES: usize = 8 * 1024;
const RETENTION_BATCH_ROWS: usize = 256;

#[derive(Clone, Copy)]
struct AllocatorTelemetry {
    live_bytes: Option<u64>,
    retained_free_bytes: Option<u64>,
    arena_bytes: Option<u64>,
    status: &'static str,
    method: Option<&'static str>,
    reason: Option<&'static str>,
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[repr(C)]
struct Mallinfo2 {
    arena: usize,
    ordblks: usize,
    smblks: usize,
    hblks: usize,
    hblkhd: usize,
    usmblks: usize,
    fsmblks: usize,
    uordblks: usize,
    fordblks: usize,
    keepcost: usize,
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
unsafe extern "C" {
    fn mallinfo2() -> Mallinfo2;
}

fn proc_rss_bytes_optional() -> Option<u64> {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmRSS:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|kib| kib.parse::<u64>().ok())
                .map(|kib| kib.saturating_mul(1024))
        })
}

fn allocator_telemetry() -> AllocatorTelemetry {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: mallinfo2 has no arguments and returns a glibc-owned value.
        let stats = unsafe { mallinfo2() };
        return AllocatorTelemetry {
            live_bytes: Some(stats.uordblks as u64),
            retained_free_bytes: Some(stats.fordblks as u64),
            arena_bytes: Some((stats.arena as u64).saturating_add(stats.hblkhd as u64)),
            status: "measured",
            method: Some("glibc mallinfo2"),
            reason: None,
        };
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        AllocatorTelemetry {
            live_bytes: None,
            retained_free_bytes: None,
            arena_bytes: None,
            status: "unsupported",
            method: None,
            reason: Some("glibc mallinfo2 is unavailable on this target"),
        }
    }
}

fn retention_sample(phase: &str) -> Value {
    let rss_bytes = proc_rss_bytes_optional();
    let allocator = allocator_telemetry();
    json!({
        "phase": phase,
        "rss_bytes": rss_bytes,
        "rss_status": if rss_bytes.is_some() { "measured" } else { "unsupported" },
        "rss_method": if rss_bytes.is_some() { Some("/proc/self/status VmRSS") } else { None::<&str> },
        "allocator_status": allocator.status,
        "allocator_method": allocator.method,
        "allocator_reason": allocator.reason,
        "allocator_live_bytes": allocator.live_bytes,
        "allocator_retained_free_bytes": allocator.retained_free_bytes,
        "allocator_arena_bytes": allocator.arena_bytes,
    })
}

fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(repo_root())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn sha256_file(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let digest = Sha256::digest(bytes);
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn retention_provenance() -> Value {
    let binary = env::current_exe().ok();
    let binary_metadata = binary.as_ref().and_then(|path| fs::metadata(path).ok());
    let git_dirty =
        command_stdout("git", &["status", "--porcelain"]).map(|status| !status.is_empty());
    json!({
        "source_git_sha": command_stdout("git", &["rev-parse", "HEAD"]),
        "source_git_dirty": git_dirty,
        "target_os": std::env::consts::OS,
        "target_arch": std::env::consts::ARCH,
        "binary_path": binary.as_deref().map(Path::display).map(|path| path.to_string()),
        "binary_size_bytes": binary_metadata.as_ref().map(std::fs::Metadata::len),
        "binary_sha256": binary.as_deref().and_then(sha256_file),
    })
}

fn retention_payload(row: usize) -> Vec<u8> {
    let mut payload = vec![0u8; RETENTION_PAYLOAD_BYTES];
    let row_bytes = (row as u64).to_le_bytes();
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte = row_bytes[index % row_bytes.len()].wrapping_add(index as u8);
    }
    payload
}

fn retention_gate(samples: &[Value]) -> Vec<String> {
    let Some(idle) = samples.iter().find(|sample| sample["phase"] == "idle_10s") else {
        return vec!["idle_10s sample is missing".to_owned()];
    };
    if idle["rss_status"] != "measured" || idle["rss_bytes"].as_u64().is_none() {
        return vec!["idle RSS is unavailable; refusing to pass".to_owned()];
    }
    let rss = idle["rss_bytes"].as_u64().unwrap_or_default();
    if rss > RETENTION_IDLE_LIMIT_BYTES {
        return vec![format!(
            "idle RSS {rss} exceeded {} bytes",
            RETENTION_IDLE_LIMIT_BYTES
        )];
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        for sample in samples {
            if sample["allocator_status"] != "measured"
                || sample["allocator_live_bytes"].as_u64().is_none()
                || sample["allocator_retained_free_bytes"].as_u64().is_none()
                || sample["allocator_arena_bytes"].as_u64().is_none()
            {
                return vec![
                    "glibc mallinfo2 telemetry is incomplete; refusing to pass".to_owned(),
                ];
            }
        }
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        return vec![
            "allocator telemetry is unsupported on this platform; refusing to pass".to_owned(),
        ];
    }
    Vec::new()
}

fn run_retention_scenario(db: &Path, rows: usize, idle_sleep: bool) -> Value {
    if db.exists() {
        panic!(
            "retention scenario requires a new controlled store: {}",
            db.display()
        );
    }
    if let Some(parent) = db.parent() {
        fs::create_dir_all(parent).expect("create retention store directory");
    }
    let started = Instant::now();
    let mut samples = Vec::new();
    let mut store = RecoveryStore::with_durable(db);
    samples.push(retention_sample("after_init"));

    for chunk_start in (0..rows).step_by(RETENTION_BATCH_ROWS) {
        let began = store.begin_benchmark_batch();
        for row in chunk_start..rows.min(chunk_start + RETENTION_BATCH_ROWS) {
            store
                .try_put_key(&format!("retention/{row:09}"), &retention_payload(row))
                .expect("retention payload");
        }
        store.end_benchmark_batch(began);
        if let Some(error) = store.take_store_error() {
            panic!("retention batch failed: {error}");
        }
    }

    let (read_rows, read_bytes) = {
        let mut scratch = Vec::with_capacity(rows.saturating_mul(RETENTION_PAYLOAD_BYTES));
        let mut read_rows = 0usize;
        for row in 0..rows {
            let key = format!("retention/{row:09}");
            let payload = store.expand(&key).expect("retention payload read");
            scratch.extend_from_slice(&payload);
            read_rows += 1;
        }
        let read_bytes = std::hint::black_box(scratch.len());
        drop(scratch);
        (read_rows, read_bytes)
    };
    samples.push(retention_sample("after_heavy"));
    if idle_sleep {
        std::thread::sleep(Duration::from_secs(10));
        samples.push(retention_sample("idle_10s"));
    }
    drop(store);

    let store_size_bytes = fs::metadata(db).ok().map(|metadata| metadata.len());
    let failures = retention_gate(&samples);
    json!({
        "schema": RETENTION_SCHEMA,
        "provenance": retention_provenance(),
        "store": {
            "path": db.display().to_string(),
            "size_bytes": store_size_bytes,
            "row_count": rows,
            "store_schema_version": 1000,
            "schema": "RecoveryStore durable SQLite; store schema major=1 minor=0",
        },
        "workload": {
            "batch_rows": RETENTION_BATCH_ROWS,
            "payload_bytes_per_row": RETENTION_PAYLOAD_BYTES,
            "read_rows": read_rows,
            "read_bytes": read_bytes,
            "phase_elapsed_ms": started.elapsed().as_secs_f64() * 1000.0,
            "idle_sleep_seconds": if idle_sleep { Some(10) } else { None::<u64> },
            "scratch_released_before_after_heavy": true,
        },
        "phases": samples,
        "gate": {
            "idle_rss_limit_bytes": RETENTION_IDLE_LIMIT_BYTES,
            "idle_phase": "idle_10s",
            "passed": idle_sleep && failures.is_empty(),
            "failures": failures,
        },
    })
}

fn retention_cli_args(args: &[String]) -> Vec<&str> {
    args.iter()
        .skip(2)
        .map(String::as_str)
        .filter(|arg| *arg != "--bench")
        .collect()
}

fn run_store_retention_measure(args: &[String]) {
    let cli_args = retention_cli_args(args);
    let owned_dir = cli_args
        .first()
        .is_none()
        .then(|| fresh_store_dir("retention-measure"));
    let db = cli_args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| owned_dir.as_ref().unwrap().join("store.sqlite3"));
    let rows = cli_args
        .get(1)
        .map(|value| {
            value
                .parse()
                .expect("retention row count must be an integer")
        })
        .unwrap_or(RETENTION_DEFAULT_ROWS);
    let report = run_retention_scenario(&db, rows, false);
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("retention report JSON")
    );
}

fn run_store_retention_benchmark(args: &[String]) {
    let cli_args = retention_cli_args(args);
    let output_dir = cli_args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| fresh_store_dir("retention-benchmark"));
    let db = output_dir.join("store.sqlite3");
    let rows = cli_args
        .get(1)
        .map(|value| {
            value
                .parse()
                .expect("retention row count must be an integer")
        })
        .unwrap_or(RETENTION_DEFAULT_ROWS);
    let report = run_retention_scenario(&db, rows, true);
    let passed = report["gate"]["passed"].as_bool().unwrap_or(false);
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("retention report JSON")
    );
    if !passed {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;

    #[test]
    fn idle_gate_rejects_unsupported_and_enforces_limit() {
        let unsupported = json!({
            "phase": "idle_10s",
            "rss_status": "unsupported",
            "rss_bytes": null
        });
        assert!(!retention_gate(&[unsupported]).is_empty());
        let at_limit = json!({
            "phase": "idle_10s",
            "rss_status": "measured",
            "rss_bytes": RETENTION_IDLE_LIMIT_BYTES,
            "allocator_status": "measured",
            "allocator_live_bytes": 1,
            "allocator_retained_free_bytes": 1,
            "allocator_arena_bytes": 1
        });
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        assert!(retention_gate(&[at_limit]).is_empty());
        #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
        assert!(!retention_gate(&[at_limit]).is_empty());
        let over_limit = json!({
            "phase": "idle_10s",
            "rss_status": "measured",
            "rss_bytes": RETENTION_IDLE_LIMIT_BYTES + 1,
            "allocator_status": "measured",
            "allocator_live_bytes": 1,
            "allocator_retained_free_bytes": 1,
            "allocator_arena_bytes": 1
        });
        assert!(!retention_gate(&[over_limit]).is_empty());
    }
}

fn repo_root() -> PathBuf {
    env::var("FSZERO_PERF_ROOT")
        .map(PathBuf::from)
        .or_else(|_| env::current_dir())
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn set_bench_env(key: &str, value: impl AsRef<std::ffi::OsStr>) {
    // SAFETY: this single-threaded harness sets process env before opening a session.
    unsafe { env::set_var(key, value) };
}

fn fresh_store_dir(label: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = env::temp_dir().join(format!("fszero-perf-{label}-{stamp}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp store dir");
    dir
}

fn run_index_build() {
    let root = repo_root();
    let store = fresh_store_dir("idx");
    set_bench_env("ZEROSTACK_STORE_ROOT", &store);
    set_bench_env("FSZERO_SKIP_GITIGNORE", "1");
    let t0 = Instant::now();
    let _ = FSZeroSession::try_with_repo_store(&root).expect("index build");
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("index_build_ms={ms:.3}");
}

fn run_session_init(root_arg: &str) {
    let root = PathBuf::from(root_arg);
    let store = fresh_store_dir("sess");
    set_bench_env("ZEROSTACK_STORE_ROOT", &store);
    set_bench_env("FSZERO_SKIP_GITIGNORE", "1");
    let t0 = Instant::now();
    let _ = FSZeroSession::try_with_repo_store(&root).expect("session init");
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("session_init_ms={ms:.3}");
}

fn warmup_read(sess: &mut FSZeroSession, path: &str) {
    let _ = sess.execute('R', Some(path));
}

fn run_read_hot(path_arg: Option<&str>) {
    let root = repo_root();
    let path = path_arg.unwrap_or("src/lib.rs");
    let mut sess = FSZeroSession::with_root(&root);
    warmup_read(&mut sess, path);
    let t0 = Instant::now();
    let (_, ok, _) = sess.execute('R', Some(path));
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("read_hot_ms={ms:.3} ok={ok}");
}

fn run_read_cold(path_arg: Option<&str>) {
    let root = repo_root();
    let path = path_arg.unwrap_or("src/lib.rs");
    let mut sess = FSZeroSession::with_root(&root);
    let t0 = Instant::now();
    let (_, ok, _) = sess.execute('R', Some(path));
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("read_cold_ms={ms:.3} ok={ok}");
}

fn run_read_range(path_arg: Option<&str>) {
    let root = repo_root();
    let path = path_arg.unwrap_or("src/lib.rs");
    let spec = format!("{path}#B0-4096");
    let mut sess = FSZeroSession::with_root(&root);
    let t0 = Instant::now();
    let (_, ok, _) = sess.execute('R', Some(&spec));
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("read_range_ms={ms:.3} ok={ok}");
}

fn run_search(query: &str) {
    let root = repo_root();
    let mut sess = FSZeroSession::with_root(&root);
    let _ = sess.execute('S', Some(query));
    let t0 = Instant::now();
    let (_, ok, _) = sess.execute('S', Some(query));
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("search_ms={ms:.3} ok={ok} query={query}");
}

fn run_resolve_empty() {
    let root = repo_root();
    let store = fresh_store_dir("resolve-empty");
    set_bench_env("ZEROSTACK_STORE_ROOT", &store);
    set_bench_env("FSZERO_SKIP_GITIGNORE", "1");
    let mut sess = FSZeroSession::try_with_repo_store(&root).expect("resolve sess");
    let t0 = Instant::now();
    let (_, ok, _) = sess.execute('V', Some("nonexistent_symbol_xyz_zz9"));
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("resolve_empty_ms={ms:.3} ok={ok}");
}

fn run_resolve_populated() {
    let root = repo_root();
    let mut sess = FSZeroSession::with_root(&root);
    let _ = sess.execute('R', Some("src/lib.rs"));
    let _ = sess.execute('R', Some("src/core/session.rs"));
    let t0 = Instant::now();
    let (_, ok, _) = sess.execute('V', Some("FSZeroSession"));
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("resolve_populated_ms={ms:.3} ok={ok}");
}

fn collect_50_paths(root: &Path) -> Vec<String> {
    let mut paths = Vec::new();
    walk_collect(root, root, 0, &mut paths);
    paths.truncate(50);
    paths
}

fn walk_collect(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    if depth > 8 || out.len() >= 50 {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if out.len() >= 50 {
            break;
        }
        let p = e.path();
        if p.is_dir() {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if matches!(name, "target" | ".git" | "node_modules" | ".zerostack") {
                continue;
            }
            walk_collect(root, &p, depth + 1, out);
        } else if p.extension().map(|e| e == "rs").unwrap_or(false) {
            if let Ok(rel) = p.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

fn run_read_50_singles() {
    let root = repo_root();
    let paths = collect_50_paths(&root);
    let mut sess = FSZeroSession::with_root(&root);
    let t0 = Instant::now();
    let mut ok_count = 0;
    for p in &paths {
        if sess.execute('R', Some(p)).1 {
            ok_count += 1;
        }
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("read_50_singles_ms={ms:.3} ok={ok_count}/{}", paths.len());
}

fn run_readmany_50() {
    let root = repo_root();
    let paths = collect_50_paths(&root);
    let paths_json: Vec<serde_json::Value> = paths.iter().map(|p| json!(p)).collect();
    let plan = json!({"label": "readmany", "steps": [{"call": "fs.multiRead", "args": {"paths": paths_json}}]});
    let mut sess = FSZeroSession::with_root(&root);
    let t0 = Instant::now();
    let ack = codemode_execute_plan(&mut sess, &plan.to_string());
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("readmany_50_ms={ms:.3} ack={ack}");
}

fn scratch_file(root: &Path) {
    let p = root.join("tests/artifacts/perf/_scratch_edit.txt");
    if let Some(parent) = p.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(&p, "alpha\nbeta\ngamma\n").expect("scratch write");
}

fn run_verified_edit(pass_verify: bool) {
    let root = repo_root();
    scratch_file(&root);
    let rel = "tests/artifacts/perf/_scratch_edit.txt";
    let inner = json!({
        "name": "verifiedEdit",
        "path": rel,
        "edits": [{"old": "beta", "new": "BETA"}],
        "verify": if pass_verify { "true" } else { "false" }
    });
    let plan_str = format!(r#"{{"steps":[{{"call":"fs.compound","args":{}}}]}}"#, inner);
    let mut sess = FSZeroSession::with_root(&root);
    let t0 = Instant::now();
    let ack = codemode_execute_plan(&mut sess, &plan_str);
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("verified_edit_ms={ms:.3} ack={ack} pass_verify={pass_verify}");
}

fn run_durable_read() {
    let root = repo_root();
    let store = fresh_store_dir("durable");
    set_bench_env("ZEROSTACK_STORE_ROOT", &store);
    set_bench_env("FSZERO_SKIP_GITIGNORE", "1");
    let mut sess = FSZeroSession::try_with_repo_store(&root).expect("durable sess");
    let t0 = Instant::now();
    let (_, ok, _) = sess.execute('R', Some("src/lib.rs"));
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("durable_read_ms={ms:.3} ok={ok}");
}

fn run_memory_read() {
    let root = repo_root();
    let mut sess = FSZeroSession::with_root(&root);
    let t0 = Instant::now();
    let (_, ok, _) = sess.execute('R', Some("src/lib.rs"));
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("memory_read_ms={ms:.3} ok={ok}");
}

fn run_codemode_trivial() {
    let root = repo_root();
    let mut sess = FSZeroSession::with_root(&root);
    let t0 = Instant::now();
    let ack = codemode_execute_plan(&mut sess, "explore");
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("codemode_trivial_ms={ms:.3} ack={ack}");
}

fn run_codemode_50step() {
    let root = repo_root();
    let mut steps = Vec::new();
    for i in 0..50 {
        steps.push(json!({"call": "fs.ls", "args": {"arg": format!("--depth={}", i % 3 + 1)}}));
    }
    let plan = json!({"label": "steps50", "steps": steps});
    let mut sess = FSZeroSession::with_root(&root);
    let t0 = Instant::now();
    let ack = codemode_execute_plan(&mut sess, &plan.to_string());
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("codemode_50step_ms={ms:.3} ack={ack}");
}

fn fszero_bin() -> PathBuf {
    env::var("FSZERO_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| repo_root().join("target/release-perf/fszero"))
}

fn run_mcp_cold_codemode() {
    let bin = fszero_bin();
    let root = repo_root();
    let stdin = format!(
        "{}\n{}\n{}\n",
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"perf","version":"0"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"fz_codemode_search","arguments":{"query":"read"}}})
    );
    let t0 = Instant::now();
    let mut child = Command::new(&bin)
        .arg("--mode=codemode")
        .env("FSZERO_ROOT", &root)
        .env("FSZERO_SKIP_GITIGNORE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("mcp_cold_codemode_ms={ms:.3} exit={:?}", out.status.code());
}

fn run_mcp_loop_50(mode: &str) {
    let bin = fszero_bin();
    let root = repo_root();
    let flag = if mode == "codemode" {
        "--mode=codemode"
    } else {
        "--mode=mcp"
    };
    let mut lines = vec![
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"perf","version":"0"}}}).to_string(),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
    ];
    for i in 0..50 {
        let line = if mode == "codemode" {
            match i % 3 {
                0 => {
                    json!({"jsonrpc":"2.0","id":i+10,"method":"tools/call","params":{"name":"fz_codemode_search","arguments":{"query":"read"}}})
                }
                1 => {
                    json!({"jsonrpc":"2.0","id":i+10,"method":"tools/call","params":{"name":"fz_execute_code","arguments":{"plan":"explore"}}})
                }
                _ => {
                    json!({"jsonrpc":"2.0","id":i+10,"method":"tools/call","params":{"name":"fz_codemode_describe","arguments":{"name":"fs.read"}}})
                }
            }
        } else {
            let names = [
                "fszero.ls",
                "fszero.read",
                "fszero.search",
                "fszero.stat",
                "fszero.resolve",
            ];
            let n = names[i % names.len()];
            let args = match n {
                "fszero.read" | "fszero.stat" => json!({"path":"src/lib.rs"}),
                "fszero.search" => json!({"arg":"FSZeroSession"}),
                "fszero.resolve" => json!({"intent":"session"}),
                _ => json!({}),
            };
            json!({"jsonrpc":"2.0","id":i+10,"method":"tools/call","params":{"name":n,"arguments":args}})
        };
        lines.push(line.to_string());
    }
    let stdin = lines.join("\n") + "\n";
    let t0 = Instant::now();
    let mut child = Command::new(&bin)
        .arg(flag)
        .env("FSZERO_ROOT", &root)
        .env("FSZERO_SKIP_GITIGNORE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let line_count = out.stdout.iter().filter(|&&b| b == b'\n').count();
    println!("mcp_loop_50_ms={ms:.3} mode={mode} resp_lines={line_count}");
    let _ = io::stdout().flush();
}
