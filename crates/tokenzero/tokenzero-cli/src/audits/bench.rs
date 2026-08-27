use crate::*;

type Json = serde_json::Value;

macro_rules! object { ($($tt:tt)*) => { serde_json::json!($($tt)*) }; }

type BenchScenario = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    bool,
);

macro_rules! scenarios {
    ($name:ident, $($row:expr);+ $(;)?) => {
        const $name: &[BenchScenario] = &[$($row),+];
    };
}

scenarios! { REPO_DEBUG_SCENARIOS,
    ("repo_inventory", &["find . -type f | sort | wc -l && find . -type f | sort"], &["powershell", "-NoProfile", "-Command", "Get-ChildItem -Recurse -File | Sort-Object FullName | Select-Object -ExpandProperty FullName"], true);
    ("grep_warning", &["grep", "warning", "sample.txt"], &["findstr", "warning", "sample.txt"], true);
}
scenarios! { EXACT_RECOVERY_SCENARIOS,
    ("stdout_stderr", &["sh", "-c", "printf alpha; printf beta >&2"], &["powershell", "-NoProfile", "-Command", "[Console]::Out.Write('alpha'); [Console]::Error.Write('beta')"], true);
    ("line_range", &["cat", "sample.txt"], &["type", "sample.txt"], true);
}
scenarios! { HOSTILE_OUTPUT_SCENARIOS,
    ("hidden_error", &["sh", "-c", "yes noise | head -n 80; echo error: boom >&2; exit 2"], &["powershell", "-NoProfile", "-Command", "for ($i = 0; $i -lt 80; $i++) { Write-Output 'noise' }; [Console]::Error.WriteLine('error: boom'); exit 2"], false);
    ("masked_pipeline", &["false", "|", "true"], &["false", "|", "true"], false);
}
scenarios! { DEFAULT_SCENARIOS,
    ("small_success", &["echo", "ok"], &["echo", "ok"], true);
    ("diagnostic_failure", &["sh", "-c", "echo warning: note; echo error: fail >&2; exit 3"], &["powershell", "-NoProfile", "-Command", "Write-Output 'warning: note'; [Console]::Error.WriteLine('error: fail'); exit 3"], false);
    ("long_repeated_log", &["sh", "-c", "for i in $(seq 1 500); do echo repeated-noise; done"], &["powershell", "-NoProfile", "-Command", "for ($i = 0; $i -lt 500; $i++) { Write-Output 'repeated-noise' }"], true);
    ("repo_inventory", &["find", ".", "-type", "f", "|", "sort", "|", "wc", "-l", "&&", "find", ".", "-type", "f", "|", "sort"], &["powershell", "-NoProfile", "-Command", "Get-ChildItem -Recurse -File | Sort-Object FullName | Select-Object -ExpandProperty FullName"], true);
}

fn bench_scenarios(suite: &str) -> Vec<(&str, Vec<&'static str>, bool)> {
    let cases = match suite {
        "repo-debug" => REPO_DEBUG_SCENARIOS,
        "exact-recovery" => EXACT_RECOVERY_SCENARIOS,
        "hostile-output" => HOSTILE_OUTPUT_SCENARIOS,
        _ => DEFAULT_SCENARIOS,
    };
    cases
        .iter()
        .map(|(id, unix, windows, expected)| {
            (
                *id,
                if cfg!(windows) {
                    windows.to_vec()
                } else {
                    unix.to_vec()
                },
                *expected,
            )
        })
        .collect()
}

pub(crate) fn run_bench_competitors(args: BenchCompetitorsArgs) -> Result<Json> {
    let exe = std::env::current_exe()?;
    let temp = tempdir()?;
    fs::write(
        temp.path().join("sample.txt"),
        "alpha\nbeta\nwarning: check me\n",
    )?;
    let cache_path = temp.path().join("bench-cache.json");
    let scenarios = bench_scenarios(&args.suite);
    let mut rows: Vec<_> = scenarios
        .into_iter()
        .map(|(id, cmd, exp)| {
            run_tokenzero_bench_row(&exe, temp.path(), &cache_path, &args.suite, id, &cmd, exp)
        })
        .collect::<Result<Vec<_>>>()?;
    let adapter_approval =
        load_benchmark_adapter_approval(args.adapter_approval_artifact.as_ref())?;
    let adapter_rows = competitor_adapter_rows(&args.suite, adapter_approval.as_ref());
    let adapter_matrix = competitor_adapter_matrix(&adapter_rows);
    rows.extend(adapter_rows);
    rows.push(object!({"schema_version": "tokenzero.bench.v1", "suite": args.suite.clone(), "scenario_id": "external_competitors", "tool": "competitors", "availability_status": "unavailable", "availability_reason": "competitor clones and private traces are approval-gated and not run by this local proof command", "raw_tokens": 0, "visible_tokens": 0, "recovery_tokens": 0, "recovery_adjusted_savings": 0.0, "byte_perfect_recovery": false, "task_success": false, "harm_gate": "not_evaluated_unavailable", "harm_rate": 0.0, "latency_overhead_ms": 0, "host_coverage": ["cli"], "interception_depth": "not_available", "safe_savings": 0.0, "adapter_allowlisted": false, "blind_install_attempted": false, "fairness_notes": "aggregate unavailable competitor summary; per-adapter rows below are marked unavailable instead of fabricating competitor results"}));
    let aggregate = aggregate_bench_rows(&rows);
    let ok = rows
        .iter()
        .filter(|r| r["tool"] == "tokenzero")
        .all(|r| r["availability_status"] == "run" && r["byte_perfect_recovery"] == true);
    let output_json = args
        .output_json
        .unwrap_or_else(|| private_benchmark_path(&args.suite));
    let report = object!({"schema_version": "tokenzero.bench.v1", "status": if ok {"ok"} else {"blocked"}, "ok": ok, "release_candidate_id": release_candidate_id(), "suite": args.suite.clone(), "private_artifact": true, "artifact_path": output_json.display().to_string(), "rows": rows, "aggregate": aggregate, "adapter_matrix": adapter_matrix, "adapter_approval_artifact": args.adapter_approval_artifact.as_ref().map(|p| p.display().to_string()), "safe_savings_formula": "safe_savings = recovery_adjusted_savings * byte_perfect_recovery_pass * task_success_pass * harm_gate_pass", "public_claims_approved": false, "release_publication_allowed": false});
    finish_artifact(&output_json, None, report, "TokenZero private benchmark")
}

pub(crate) fn run_adapter_approval_audit(
    output_json: PathBuf,
    output_md: Option<PathBuf>,
    approval_file: Option<PathBuf>,
    execution_approval: bool,
) -> Result<Json> {
    let report = competitor_adapters::adapter_approval_audit_report(
        approval_file.as_deref(),
        execution_approval,
        &release_candidate_id(),
    )?;
    finish_artifact(
        &output_json,
        output_md.as_deref(),
        report,
        "Adapter approval audit",
    )
}

pub(crate) fn run_adapter_approval_template(
    output_json: PathBuf,
    output_md: Option<PathBuf>,
) -> Result<Json> {
    let report = competitor_adapters::adapter_approval_template_report(&release_candidate_id());
    finish_artifact(
        &output_json,
        output_md.as_deref(),
        report,
        "Adapter approval template",
    )
}

fn recovery_adjusted_savings(raw: f64, consumed: f64) -> f64 {
    if raw == 0.0 {
        0.0
    } else {
        1.0 - consumed / raw
    }
}

fn gated_savings(savings: f64, gates_pass: bool) -> f64 {
    if gates_pass {
        savings
    } else {
        0.0
    }
}

pub(crate) fn aggregate_bench_rows(rows: &[Json]) -> Json {
    let tz_rows: Vec<_> = rows.iter().filter(|r| r["tool"] == "tokenzero").collect();
    let raw: f64 = tz_rows
        .iter()
        .map(|r| r["raw_tokens"].as_f64().unwrap_or(0.0))
        .sum();
    let visible_rec: f64 = tz_rows
        .iter()
        .map(|r| {
            r["visible_tokens"].as_f64().unwrap_or(0.0)
                + r["recovery_tokens"].as_f64().unwrap_or(0.0)
        })
        .sum();
    let bp = tz_rows.iter().all(|r| r["byte_perfect_recovery"] == true);
    let ts = tz_rows.iter().all(|r| r["task_success"] == true);
    let hr = average_harm_rate(&tz_rows);
    let harm_gate_pass = hr == 0.0;
    let gates = bp && ts && harm_gate_pass;
    let ras = recovery_adjusted_savings(raw, visible_rec);
    let safe = gated_savings(ras, gates);
    object!({"raw_tokens": raw as u64, "visible_plus_recovery_tokens": visible_rec as u64, "recovery_adjusted_savings": ras, "byte_perfect_recovery_pass": bp, "task_success_pass": ts, "harm_rate": hr, "harm_gate_pass": harm_gate_pass, "safe_savings": safe, "target_safe_savings": 0.70, "target_met": safe >= 0.70})
}

fn average_harm_rate(rows: &[&Json]) -> f64 {
    if rows.is_empty() {
        return 0.0;
    }
    rows.iter()
        .filter(|r| r["harm_rate"].as_f64().unwrap_or(1.0) > 0.0)
        .count() as f64
        / rows.len() as f64
}

pub(crate) fn run_tokenzero_bench_row(
    exe: &Path,
    cwd: &Path,
    cache_path: &Path,
    suite: &str,
    scenario_id: &str,
    command: &[&str],
    expected_success: bool,
) -> Result<Json> {
    let start = Instant::now();
    let output = Command::new(exe)
        .arg("run")
        .arg("--json")
        .arg("--cache-path")
        .arg(cache_path)
        .arg("--allowed-root")
        .arg(cwd)
        .arg("--cwd")
        .arg(cwd)
        .arg("--")
        .args(command)
        .output()?;
    let latency = start.elapsed().as_millis();
    let parsed: Json = serde_json::from_slice(&output.stdout)?;
    let t = &parsed["telemetry"];
    let checks = expand_ref_checks(exe, cache_path, t)?;
    let byte_perfect = checks.iter().all(|c| c["byte_perfect"] == true)
        && checks
            .iter()
            .any(|c| c["kind"] == "combined" && c["bytes"].as_u64().unwrap_or(0) > 0);
    let a = &parsed["accounting"];
    let raw = a["raw_tokens"].as_u64().unwrap_or(0) as f64;
    let vis = a["visible_tokens"].as_u64().unwrap_or(0) as f64;
    let rec = a["recovery_tokens"].as_u64().unwrap_or(0) as f64;
    let ras = recovery_adjusted_savings(raw, vis + rec);
    let cmd_ok = t["command_success"].as_bool().unwrap_or(false);
    let task_ok = cmd_ok == expected_success;
    let hr = if task_ok { 0.0 } else { 1.0 };
    let safe = gated_savings(ras, byte_perfect && task_ok && hr == 0.0);
    Ok(
        object!({"schema_version": "tokenzero.bench.v1", "suite": suite, "scenario_id": scenario_id, "tool": "tokenzero", "availability_status": "run", "command": command.join(" "), "raw_tokens": raw as u64, "visible_tokens": vis as u64, "recovery_tokens": rec as u64, "recovery_adjusted_savings": ras, "byte_perfect_recovery": byte_perfect, "task_success": task_ok, "expected_command_success": expected_success, "observed_command_success": cmd_ok, "harm_rate": hr, "harm_gate_pass": hr == 0.0, "latency_overhead_ms": latency, "host_coverage": ["cli"], "interception_depth": "explicit_cli", "safe_savings": safe, "status_label": t["status_label"], "stdout_ref": t["stdout_ref"], "stderr_ref": t["stderr_ref"], "combined_ref": t["combined_ref"], "exact_expand_checks": checks, "fairness_notes": "uses built tokenzero CLI with exact expansion check"}),
    )
}

fn expand_ref_checks(exe: &Path, cache: &Path, telemetry: &Json) -> Result<Vec<Json>> {
    ["stdout", "stderr", "combined"]
        .iter()
        .filter_map(|kind| {
            let ref_id = telemetry[&format!("{kind}_ref")]
                .as_str()
                .unwrap_or_default();
            if ref_id.is_empty() {
                return None;
            }
            Some(super::recovery::expand_ref_check_row(
                exe, cache, kind, ref_id,
            ))
        })
        .collect()
}

pub(crate) fn private_benchmark_path(suite: &str) -> PathBuf {
    let root = crate::zerostack_store::tokenzero_work_root(None);
    root.parent()
        .unwrap_or(root.as_path())
        .join(".tokenzero-private-benchmarks")
        .join("matrix-current")
        .join(format!("{suite}.json"))
}

/// Shell-matrix success without substring agreement.
///
/// `"ok": false` JSON contains the letters `ok`. Text envelopes must have a
/// payload line that is exactly `ok` (the `echo ok` fixture), not a diagnostic
/// that happens to mention it.
pub(crate) fn matrix_row_ok(exit_success: bool, stdout: &str) -> bool {
    if !exit_success {
        return false;
    }
    let trimmed = stdout.trim();
    if let Ok(parsed) = serde_json::from_str::<Json>(trimmed) {
        if parsed["status"].as_str() != Some("ok") {
            return false;
        }
        return parsed
            .pointer("/telemetry/command_success")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
    }
    stdout.lines().any(|line| line.trim() == "ok")
}

pub(crate) fn run_matrix_row(label: &str, command: &mut Command) -> Json {
    let start = Instant::now();
    match command.output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let ok = matrix_row_ok(output.status.success(), &stdout);
            object!({"label": label, "ok": ok, "skipped": false, "status": if ok { "run" } else { "fail" }, "exit_code": output.status.code(), "stdout": stdout, "stderr": String::from_utf8_lossy(&output.stderr).to_string(), "duration_ms": start.elapsed().as_millis(), "alias_dependency": false})
        }
        Err(err) => {
            object!({"label": label, "ok": false, "skipped": false, "status": "fail", "error": err.to_string(), "alias_dependency": false})
        }
    }
}

#[cfg(test)]
mod matrix_row_ok_tests {
    use super::matrix_row_ok;

    #[test]
    fn json_ok_false_is_not_green() {
        assert!(
            !matrix_row_ok(true, r#"{"status":"error","ok":false,"visible":"ok"}"#),
            "JSON key/value ok must not substring-match as success"
        );
    }

    #[test]
    fn json_status_ok_with_failed_command_is_not_green() {
        assert!(!matrix_row_ok(
            true,
            r#"{"status":"ok","telemetry":{"command_success":false}}"#
        ));
    }

    #[test]
    fn text_payload_line_ok_is_green() {
        assert!(matrix_row_ok(true, "ok\ncombined_ref: tz://blob/abc\n"));
    }

    #[test]
    fn diagnostic_mentioning_ok_is_not_green() {
        assert!(!matrix_row_ok(true, "looks ok but the child failed\n"));
    }
}
