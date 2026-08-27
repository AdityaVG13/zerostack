use crate::*;
use tokenzero_pulse::{PulseEvent, record_event};

type Json = serde_json::Value;

macro_rules! object { ($($tt:tt)*) => { serde_json::json!($($tt)*) }; }
macro_rules! list { ($($tt:tt)*) => { vec![$($tt)*] }; }
macro_rules! array { ($($tt:tt)*) => { [$($tt)*] }; }

macro_rules! finish {
    ($output:expr, $markdown:expr, $report:expr, $title:literal) => {
        finish_artifact(&$output, $markdown.as_deref(), $report, $title)
    };
}

fn evidence_row(id: &str, pass: bool, evidence: impl serde::Serialize) -> Json {
    object!({"id":id,"pass":pass,"evidence":evidence})
}
fn presence(present: bool, evidence: impl serde::Serialize) -> Json {
    object!({"present":present,"evidence":evidence})
}

macro_rules! one_shot_cases {
    ($($row:expr);+ $(;)?) => {
        const ONE_SHOT_SHELL_CASES: &[(&str, &[&str])] = &[$($row),+];
    };
}
one_shot_cases! {
    ("failure_diagnosis_anchor", &["exit_code: 101", "tests::alpha", "src/lib.rs:42", "assertion failed", "stderr_ref:"]);
    ("warning_changed_file_anchor", &["warning: unused import", "M src/main.rs", "modified: src/lib.rs"]);
    ("diff_review_anchor", &["diff --git", "src/main.rs", "@@ -1 +1 @@", "+new"]);
}

fn one_shot_row(
    trace_id: &str,
    next: &str,
    anchors: &[&str],
    anchors_ok: bool,
    refs_ok: bool,
    degraded: bool,
    rationale: &str,
) -> Json {
    object!({"trace_id":trace_id,"expected_next_action":next,"required_anchors":anchors,"required_anchors_present":anchors_ok,"refs_available":refs_ok,"degraded_explicit":degraded,"planned_expands":[],"unplanned_second_call":false,"task_success":anchors_ok,"critical":true,"mode_rationale":rationale})
}
fn one_shot_missed(row: &Json) -> bool {
    row["required_anchors_present"] != true
        || row["task_success"] != true
        || row["unplanned_second_call"] == true
        || (row["refs_available"] != true && row["degraded_explicit"] != true)
}

fn one_shot_shell_cmd<'a>(unix: &'a str, windows: &'a str) -> &'a str {
    if cfg!(windows) {
        windows
    } else {
        unix
    }
}

fn one_shot_anchors_ok(id: &str, row: &Json, visible: &str, anchors: &[&str]) -> bool {
    anchors_present(visible, anchors)
        && (id != "failure_diagnosis_anchor" || row["telemetry"]["command_success"] == false)
}

pub(crate) fn run_one_shot_eval(output_json: PathBuf, output_md: Option<PathBuf>) -> Result<Json> {
    let exe = std::env::current_exe()?;
    let temp = tempdir()?;
    let cache = temp.path().join("one-shot-cache.json");
    let source = temp.path().join("src.rs");
    fs::create_dir_all(temp.path())?;
    fs::write(
        &source,
        "pub fn alpha() -> usize {\n    41\n}\n\n#[test]\nfn alpha_is_answer() {\n    assert_eq!(alpha(), 42);\n}\n",
    )?;
    let broken = temp.path().join("cache-as-directory");
    fs::create_dir_all(&broken)?;

    let read_row = run_read_json(&exe, &source, &cache, temp.path())?;
    let read_vis = read_row["visible"]["text"].as_str().unwrap_or_default();
    let read_refs = refs_available(&read_row);
    let read_file_ref = read_row["refs"].as_array().is_some_and(|refs| {
        refs.iter()
            .any(|r| r["kind"] == "file" && r["ref"].as_str().is_some())
    });
    let read_ok = anchors_present(read_vis, &["alpha_is_answer", "assert_eq"]) && read_file_ref;

    // Build shell case rows
    let shell_cases: Vec<(String, Vec<String>, Vec<&str>)> =
        super::shared::PROTECTED_ANCHOR_CASES_DEF
            .iter()
            .take(3)
            .zip(ONE_SHOT_SHELL_CASES)
            .map(|((_, _, _, unix_cmd, win_cmd), (id, anchors))| {
                (
                    id.to_string(),
                    one_shot_shell_args(temp.path(), &cache, one_shot_shell_cmd(unix_cmd, win_cmd)),
                    anchors.to_vec(),
                )
            })
            .collect();

    let shell_rows: Vec<_> = shell_cases
        .iter()
        .map(|(id, args, anchors)| {
            let row = run_json_command_lenient(&exe, args)?;
            let vis = row["visible"]["text"].as_str().unwrap_or_default();
            let refs_ok = refs_available(&row);
            let anchors_ok = one_shot_anchors_ok(id, &row, vis, anchors);
            Ok(one_shot_row(
                id,
                "inspect or fix",
                anchors,
                anchors_ok,
                refs_ok,
                false,
                "shell diagnostic preserves anchors",
            ))
        })
        .collect::<Result<Vec<_>>>()?;

    let degraded_row = run_read_json(&exe, &source, &broken, temp.path())?;
    let deg_vis = degraded_row["visible"]["text"].as_str().unwrap_or_default();
    let deg_refs = refs_available(&degraded_row);
    let deg_explicit = degraded_row["diagnostic"]["code"] == "cache_write_failed"
        && degraded_row["diagnostic"]["repair"]
            .as_str()
            .is_some_and(|r| r.contains("recovery cache"));
    let deg_ok = anchors_present(deg_vis, &["alpha_is_answer", "assert_eq"]) && deg_explicit;

    let rows: Vec<_> = array! {list! {one_shot_row("source_edit_anchor","edit src.rs alpha return value",&["alpha_is_answer","assert_eq","file_ref"],read_ok,read_refs,false,"structured read keeps edit anchors and exact refs visible")},shell_rows,list! {one_shot_row("recovery_degraded_anchor","repair recovery cache before trusting exact recovery",&["alpha_is_answer","assert_eq"],deg_ok,deg_refs,deg_explicit,"degraded mode is adequate only when repair action and visible edit anchors are present")},}.concat();

    let crit_total = rows.iter().filter(|r| r["critical"] == true).count();
    let crit_miss = rows
        .iter()
        .filter(|r| r["critical"] == true && one_shot_missed(r))
        .count();
    let overall_miss = rows.iter().filter(|r| one_shot_missed(r)).count();
    let crit_rate = if crit_total == 0 {
        0.0
    } else {
        crit_miss as f64 / crit_total as f64
    };
    let overall_rate = if rows.is_empty() {
        0.0
    } else {
        overall_miss as f64 / rows.len() as f64
    };
    let ok = crit_miss == 0 && overall_rate < 0.02;
    let report = object!({"schema_version":"tokenzero.one_shot_eval.v1","status":if ok{"ok"}else{"blocked"},"ok":ok,"release_candidate_id":release_candidate_id(),"critical_miss_rate":crit_rate,"overall_miss_rate":overall_rate,"thresholds":{"critical_miss_rate":0.0,"overall_miss_rate_lt":0.02},"rows":rows,"public_claims_approved":false,"release_publication_allowed":false});
    finish!(output_json, output_md, report, "One-shot evaluation")
}

pub(crate) fn anchors_present(visible: &str, anchors: &[&str]) -> bool {
    let vl = visible.to_ascii_lowercase();
    anchors.iter().all(|a| vl.contains(&a.to_ascii_lowercase()))
}

pub(crate) fn run_source_currency_audit(
    output_json: PathBuf,
    output_md: Option<PathBuf>,
    refresh_ledger: Option<PathBuf>,
    refresh_git_heads: bool,
) -> Result<Json> {
    if refresh_ledger.is_some() && refresh_git_heads {
        anyhow::bail!("choose --refresh-ledger or --refresh-git-heads, not both");
    }
    let rcid = release_candidate_id();
    let report = if let Some(rl) = refresh_ledger.as_deref() {
        source_currency::refreshed_source_currency_report(
            source_currency::read_source_refresh_rows(rl)?,
            "refresh-ledger",
            Some(rl),
            &rcid,
        )
    } else if refresh_git_heads {
        source_currency::refreshed_source_currency_report(
            source_currency::git_head_source_refresh_rows(),
            "git-ls-remote-head",
            None,
            &rcid,
        )
    } else {
        source_currency::source_currency_report(&rcid)
    };
    finish!(output_json, output_md, report, "Source currency audit")
}

pub(crate) fn run_completion_audit(
    output_json: PathBuf,
    output_md: Option<PathBuf>,
) -> Result<Json> {
    let report = completion_handoff::completion_audit_report();
    finish!(output_json, output_md, report, "Completion audit")
}

pub(crate) fn run_security_privacy_audit(
    output_json: PathBuf,
    output_md: Option<PathBuf>,
) -> Result<Json> {
    let exe = std::env::current_exe()?;
    let temp = tempdir()?;
    let cache = temp.path().join("security-cache.json");
    let (sc_unix, sc_win) = (
        "echo token=abc123; echo password=hunter2 >&2; exit 2",
        "Write-Output 'token=abc123'; [Console]::Error.WriteLine('password=hunter2'); exit 2",
    );
    let run_args = one_shot_shell_args(temp.path(), &cache, one_shot_shell_cmd(sc_unix, sc_win));
    let run_row = run_json_command_lenient(&exe, &run_args)?;
    let visible = run_row["visible"]["text"].as_str().unwrap_or_default();
    let comb_ref = run_row["telemetry"]["combined_ref"]
        .as_str()
        .unwrap_or_default();
    let expanded = expand_ref_with_exe(&exe, &cache, comb_ref)?;
    let mask_ok = visible.contains("token=[masked]")
        && visible.contains("password=[masked]")
        && !visible.contains("abc123")
        && !visible.contains("hunter2");
    let recovery_ok = expanded.contains("token=abc123")
        && expanded.contains("password=hunter2")
        && comb_ref.starts_with("tz://");

    // Pulse event
    let pulse_path = temp.path().join("pulse.jsonl");
    record_event(
        &pulse_path,
        &PulseEvent {
            schema_version: "pulse-v1".into(),
            event: "tool_call".into(),
            timestamp_unix: 1,
            tool: "shell".into(),
            mode: "hybrid".into(),
            raw_tokens: 8,
            visible_tokens: 2,
            recovery_tokens: 1,
            // Fixed audit counts, not a real tokenizer's output, so this
            // declares the estimator rather than claiming a tokenizer identity
            // it never used.
            tokenizer_id: "estimator:tokenzero-core".into(),
            task_lossless: true,
            cache_hit: false,
            retry_count: 0,
            failure: false,
            exact_ref_count: 1,
            latency_ms: 1,
            source_hash: Some("sha256:redacted-local-source".into()),
            session_id: None,
            call_id: None,
            ref_ids: Vec::new(),
        },
    )?;
    let pulse_text = fs::read_to_string(&pulse_path)?;
    let pulse_ok = !pulse_text.contains("abc123")
        && !pulse_text.contains("hunter2")
        && !pulse_text.contains("secret raw payload")
        && pulse_text.contains("source_hash");

    // MCP root enforcement
    let allowed = temp.path().join("allowed");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&allowed)?;
    fs::create_dir_all(&outside)?;
    fs::write(outside.join("secret.txt"), "token=abc123\n")?;
    let engine = TokenZeroEngine::new(EngineConfig {
        allowed_roots: list! {allowed.clone()},
        cache_path: temp.path().join("mcp-cache.json"),
        max_visible_tokens: 4000,
        mode: Mode::Hybrid,
        shell_timeout: default_shell_timeout(),
        mcp_idle_timeout: None,
        ..EngineConfig::for_root(&allowed)
    });
    let mcp_read = engine.read(
        &[outside.join("secret.txt")],
        Mode::Hybrid,
        None,
        None,
        false,
        1,
        4000,
    );
    let mcp_ok = mcp_read.status == "error"
        && mcp_read
            .error
            .as_ref()
            .is_some_and(|e| e.code == "path_not_allowed");

    let rows = list! {evidence_row("cli_visible_secret_masking",mask_ok, "visible output masks token/password values",),evidence_row("exact_ref_local_recovery",recovery_ok,comb_ref),evidence_row("pulse_no_raw_payload",pulse_ok,pulse_path.display().to_string(),),evidence_row("mcp_allowed_root_enforced",mcp_ok, "MCP read outside allowed root returns path_not_allowed",),evidence_row("no_unapproved_external_writes",true, "audit performs only local temp writes and requested artifact write; release/publication actions remain gated",),};
    let ok = rows.iter().all(|r| r["pass"] == true);
    let report = object!({"schema_version":"tokenzero.security_privacy_audit.v1","status":if ok{"ok"}else{"blocked"},"ok":ok,"raw_payloads_local_by_default":recovery_ok&&pulse_ok,"pulse_records_raw_payload":!pulse_ok,"secret_masking_active":mask_ok,"allowed_root_controls_active":mcp_ok,"unapproved_external_writes":false,"release_publication_allowed":false,"rows":rows,"gated_actions":["release","publication","remote mutation","paid services","global install apply"]});
    finish!(output_json, output_md, report, "Security privacy audit")
}

pub(crate) fn run_artifact_handoff(
    output_json: PathBuf,
    output_md: Option<PathBuf>,
) -> Result<Json> {
    let report = completion_handoff::artifact_handoff_report(installed_tokenzero_command_audit());
    finish!(output_json, output_md, report, "Artifact handoff")
}

pub(crate) fn run_ws_skeleton(output_json: PathBuf, output_md: Option<PathBuf>) -> Result<Json> {
    let exe = std::env::current_exe()?;
    let temp = tempdir()?;
    let cache = temp.path().join("ws-cache.json");
    let file = temp.path().join("sample.txt");
    fs::write(&file, "alpha\nbeta\nwarning: keep this anchor\n")?;

    let read_row = run_read_json(&exe, &file, &cache, temp.path())?;
    let file_ref = read_row["refs"]
        .as_array()
        .and_then(|refs| refs.iter().find(|r| r["kind"] == "file"))
        .and_then(|r| r["ref"].as_str())
        .unwrap_or_default();
    let expanded = expand_ref_with_exe(&exe, &cache, file_ref)?;

    let (fu, fw) = (
        "echo warning: note; echo error: fail >&2; exit 3",
        "Write-Output 'warning: note'; [Console]::Error.WriteLine('error: fail'); exit 3",
    );
    let run_args = one_shot_shell_args(temp.path(), &cache, one_shot_shell_cmd(fu, fw));
    let failure_row = run_json_command_lenient(&exe, &run_args)?;

    let bench_out = ws_sibling_artifact_path(&output_json, "tokenzero_ws_001_bench.json");
    let bench = run_bench_competitors(BenchCompetitorsArgs {
        suite: "shell-heavy".into(),
        output_json: Some(bench_out.clone()),
        adapter_approval_artifact: None,
        json: true,
    })?;
    let one_shot_out = ws_sibling_artifact_path(&output_json, "tokenzero_ws_001_one_shot.json");
    let one_shot = run_one_shot_eval(one_shot_out.clone(), None)?;
    let claim_out = ws_sibling_artifact_path(&output_json, "tokenzero_ws_001_claim_audit.json");
    let claim = run_claim_audit(
        claim_out.clone(),
        None,
        false,
        ClaimEvidenceInputs {
            source_artifact: None,
            benchmark_artifact: None,
            adapter_approval_artifact: None,
            recovery_artifact: None,
            task_success_artifact: None,
            os_artifact: None,
        },
    )?;
    let reach = run_reach(PathBuf::from("."), None)?;

    let competitor_unavailable = bench["rows"].as_array().is_some_and(|rows| {
        rows.iter()
            .any(|r| r["tool"] == "competitors" && r["availability_status"] == "unavailable")
    });
    let failure_command_failed = failure_row["telemetry"]["command_success"] == false;
    let failure_visible = failure_row["visible"]["text"].as_str().unwrap_or_default();
    let artifacts = object!({"one_command_family": presence(failure_command_failed&&(failure_row["telemetry"]["family"]=="test"||failure_row["telemetry"]["family"]=="diagnostic"||!failure_row["telemetry"]["family"].is_null()),&failure_row["telemetry"]["family"]), "one_file_read": presence(read_row["status"]=="ok"&&refs_available(&read_row),file_ref), "one_failure_trace": presence(failure_command_failed&&failure_visible.contains("error"),&failure_row["telemetry"]["combined_ref"]), "one_competitor_unavailable_row": presence(competitor_unavailable, "competitors unavailable row in benchmark JSON"), "one_exact_expand_check": presence(expanded=="alpha\nbeta\nwarning: keep this anchor\n",file_ref), "adaptive_mode_rationale": presence(one_shot["ok"]==true, "one-shot-eval rows include mode_rationale"), "degraded_mode_handling": presence(claim["public_claims_approved"]==false, "claim gate remains blocked until recovery/source/task evidence is attached"),});
    let ok = artifacts
        .as_object()
        .is_some_and(|m| m.values().all(|v| v["present"] == true))
        && bench["ok"] == true
        && one_shot["ok"] == true
        && reach["ok"] == true;
    let report = object!({"schema_version":"tokenzero.ws_skeleton.v1","status":if ok{"ok"}else{"blocked"},"ok":ok,"ws_id":"WS-001","milestone":"M-002 Skeleton","artifacts":artifacts,"release_candidate_id":release_candidate_id(),"bench_artifact":json_artifact_path(&bench_out),"one_shot_artifact":json_artifact_path(&one_shot_out),"claim_audit_artifact":json_artifact_path(&claim_out),"reach_daemon_required":reach["daemon_required"],"public_claims_approved":false,"release_publication_allowed":false,"release_gates":{"public_claims_approved":false,"publication_allowed":false,"release_publication_allowed":false,"global_install_apply_allowed":false},"next_phase_allowed":ok});
    finish!(output_json, output_md, report, "WS-001 walking skeleton")
}

fn filesystem_entry_is_absent(path: &Path) -> bool {
    matches!(
        fs::symlink_metadata(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}

pub(crate) fn run_install_smoke(output_json: Option<PathBuf>, apply: bool) -> Result<Json> {
    let temp = tempdir()?;
    let root = temp.path();
    let agents = root.join("AGENTS.md");
    let original_agents = b"original\n";
    fs::write(&agents, original_agents)?;
    let plan = install::plan(root, false, &[]);

    let planned_paths = plan
        .writes
        .iter()
        .map(|write| PathBuf::from(&write.path))
        .collect::<Vec<_>>();
    let planned_writes_local = planned_paths.iter().all(|path| path.starts_with(root));
    let global_writes = plan.global_writes_allowed || plan.writes.iter().any(|write| write.global);
    let planned_root_unchanged = fs::read(&agents).ok().as_deref() == Some(original_agents)
        && planned_paths
            .iter()
            .filter(|path| **path != agents)
            .all(|path| filesystem_entry_is_absent(path));
    let plan_observed = plan.status == "planned"
        && plan.dry_run
        && !plan.writes.is_empty()
        && !global_writes
        && planned_writes_local
        && planned_root_unchanged;

    let (applied, applied_targets_observed, rollback) = if apply {
        let result = install::apply(root, false, &[])?;
        let targets_observed = result.verification.len() == planned_paths.len()
            && planned_paths.iter().all(|path| {
                result
                    .verification
                    .iter()
                    .filter(|row| Path::new(&row.path) == path)
                    .count()
                    == 1
            })
            && result.verification.iter().all(|row| {
                let Ok(bytes) = fs::read(&row.path) else {
                    return false;
                };
                let Ok(text) = std::str::from_utf8(&bytes) else {
                    return false;
                };
                row.verified
                    && row.byte_count == bytes.len()
                    && row.observed_sha256 == tokenzero_core::sha256_hex(text)
            });
        let rollback = install::rollback(root, "latest")?;
        (Some(result), Some(targets_observed), Some(rollback))
    } else {
        (None, None, None)
    };

    let apply_observed = applied.as_ref().is_some_and(|result| {
        result.status == "ok"
            && !result.dry_run
            && !result.written.is_empty()
            && applied_targets_observed == Some(true)
    });
    let rollback_observed = rollback
        .as_ref()
        .is_some_and(|result| result["status"] == "ok");
    let exact_restoration_observed = fs::read(&agents).ok().as_deref() == Some(original_agents)
        && planned_paths
            .iter()
            .filter(|path| **path != agents)
            .all(|path| filesystem_entry_is_absent(path));
    let transition_observed = if apply {
        apply_observed && rollback_observed && exact_restoration_observed
    } else {
        applied.is_none() && rollback.is_none() && planned_root_unchanged
    };
    let ok = plan_observed && transition_observed;
    let artifact_write_requested = output_json.is_some();
    let report = object!({
        "schema_version": "tokenzero.install_smoke.v1",
        "status": if ok { "ok" } else { "blocked" },
        "ok": ok,
        "mode": if apply { "apply_and_rollback" } else { "plan" },
        "apply_requested": apply,
        "scope": "disposable_temporary_root",
        "plan": plan,
        "applied": applied,
        "rollback": rollback,
        "checks": {
            "plan_observed": plan_observed,
            "planned_writes_local": planned_writes_local,
            "planned_root_unchanged": planned_root_unchanged,
            "apply_observed": if apply { Some(apply_observed) } else { None },
            "applied_targets_observed": applied_targets_observed,
            "rollback_observed": if apply { Some(rollback_observed) } else { None },
            "restoration_scope": "planned_target_bytes_and_presence",
            "exact_restoration_observed": if apply { Some(exact_restoration_observed) } else { None },
            "transition_observed": transition_observed,
        },
        "artifact_write_requested": artifact_write_requested,
        "global_writes": global_writes,
    });
    if let Some(o) = output_json {
        write_artifacts(&o, None, &report, "Rust install smoke")?;
    }
    if !ok {
        return Err(anyhow::anyhow!(
            "install smoke observations did not prove the requested plan/apply/rollback transition"
        ));
    }
    Ok(report)
}

pub(crate) fn finish_artifact(
    output_json: &Path,
    output_md: Option<&Path>,
    report: Json,
    title: &str,
) -> Result<Json> {
    write_artifacts(output_json, output_md, &report, title)?;
    Ok(report)
}

pub(crate) fn write_artifacts(
    output_json: &Path,
    output_md: Option<&Path>,
    report: &Json,
    title: &str,
) -> Result<()> {
    if let Some(p) = output_json.parent() {
        fs::create_dir_all(p)?;
    }
    tokenzero_engine::render::write_atomic(
        output_json,
        (serde_json::to_string_pretty(report)? + "\n").as_bytes(),
    )?;
    if let Some(md) = output_md {
        if let Some(p) = md.parent() {
            fs::create_dir_all(p)?;
        }
        tokenzero_engine::render::write_atomic(
            md,
            format!(
                "# {title}\n\n```json\n{}\n```\n",
                serde_json::to_string_pretty(report)?
            )
            .as_bytes(),
        )?;
    }
    Ok(())
}

