use crate::*;

fn release_os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        std::env::consts::OS
    }
}

fn core_surface_row(
    surface: &str,
    ok: bool,
    evidence: &str,
    details: serde_json::Value,
) -> serde_json::Value {
    json!({"surface":surface,"ok":ok,"daemon_required":false,"global_writes":false,"evidence":evidence,"details":details})
}

fn posix_shell_matrix_command(exe: &Path, cache: &Path) -> String {
    quote_for(
        "posix",
        &[
            exe.to_string_lossy().into_owned(),
            "run".into(),
            "--cache-path".into(),
            cache.to_string_lossy().into_owned(),
            "--".into(),
            "echo".into(),
            "ok".into(),
        ],
    )
}

pub(crate) fn run_shell_matrix(
    output_json: PathBuf,
    output_md: Option<PathBuf>,
) -> Result<serde_json::Value> {
    let exe = std::env::current_exe()?;
    let temp = tempdir()?;
    let cache = temp.path().join("matrix-cache.json");
    let mut rows = Vec::new();
    let mut direct = Command::new(&exe);
    direct
        .args(["run", "--cache-path"])
        .arg(&cache)
        .args(["--", "echo", "ok"]);
    rows.push(run_matrix_row("direct", &mut direct));
    if cfg!(unix) {
        let mut env_cmd = Command::new("env");
        env_cmd
            .arg("-i")
            .arg(&exe)
            .args(["run", "--cache-path"])
            .arg(&cache)
            .args(["--", "echo", "ok"]);
        rows.push(run_matrix_row("env-i", &mut env_cmd));
        let inv = posix_shell_matrix_command(&exe, &cache);
        for clean in [false, true] {
            for sh in ["/bin/sh", "/bin/bash", "/bin/zsh"] {
                let label = if clean {
                    format!("env-i {sh} -c")
                } else {
                    format!("{sh} -c")
                };
                if !Path::new(sh).exists() {
                    rows.push(json!({
                        "label": label,
                        "ok": false,
                        "skipped": true,
                        "status": "skip",
                        "reason": "shell binary not present on this host",
                        "alias_dependency": false
                    }));
                    continue;
                }
                let mut cmd = if clean {
                    Command::new("env")
                } else {
                    Command::new(sh)
                };
                if clean {
                    cmd.arg("-i").arg(sh);
                }
                cmd.arg("-c").arg(&inv);
                rows.push(run_matrix_row(&label, &mut cmd));
            }
        }
    }
    if cfg!(windows) {
        let args = vec![
            exe.display().to_string(),
            "run".into(),
            "--cache-path".into(),
            cache.display().to_string(),
            "--".into(),
            "echo".into(),
            "ok".into(),
        ];
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(quote_for("cmd", &args));
        rows.push(run_matrix_row("cmd /C", &mut cmd));
        let mut ps = Command::new("powershell");
        ps.arg("-NoProfile")
            .arg("-Command")
            .arg(format!("& {}", quote_for("powershell", &args)));
        rows.push(run_matrix_row("powershell -NoProfile", &mut ps));
    }
    let run_rows: Vec<_> = rows.iter().filter(|r| r["status"] != "skip").collect();
    let skipped = rows.iter().filter(|r| r["status"] == "skip").count();
    let ok = !run_rows.is_empty() && run_rows.iter().all(|r| r["ok"] == true);
    let report = json!({"schema_version":"tokenzero.shell_matrix.v1","status":if ok{"ok"}else{"blocked"},"ok":ok,"skipped_rows":skipped,"rows":rows,"windows":if cfg!(windows){"run"}else{"not_run_on_this_host"},"linux":if cfg!(target_os="linux"){"run"}else{"not_run_on_this_host"},"macos":if cfg!(target_os="macos"){"run"}else{"not_run_on_this_host"}});
    finish_artifact(
        &output_json,
        output_md.as_deref(),
        report,
        "Rust shell matrix",
    )
}

pub(crate) fn run_os_reach_audit(
    output_json: PathBuf,
    output_md: Option<PathBuf>,
    root: PathBuf,
    os_artifacts: Vec<PathBuf>,
    release_approval: bool,
) -> Result<serde_json::Value> {
    let temp = tempdir()?;
    let sm = run_shell_matrix(temp.path().join("shell-matrix.json"), None)?;
    let install = run_install_smoke(None, true)?;
    let reach = run_reach(root, None)?;
    let cs = run_core_surface_audit(&sm, &install)?;
    let ext = load_os_release_artifacts(&os_artifacts)?;
    let release_oses = ["windows", "linux", "macos"];
    let cur = release_os();
    let rcid = release_candidate_id();

    let os_rows: Vec<_> = release_oses.iter().map(|os| {
        if *os == cur {
            json!({"os":os,"current_host":true,"artifact_source":"local","shell_matrix":sm[*os].as_str().unwrap_or("not_run_on_this_host"),"install_smoke":if install["ok"]==true{"run"}else{"not_run_on_this_host"},"daemon_required":false,"global_writes":false,"release_candidate_id":rcid,"claim_ready":sm[*os].as_str().unwrap_or("")=="run"&&install["ok"]==true,"evidence":"local release-path artifact"})
        } else if let Some(a) = ext.iter().find(|a| a["os"].as_str().unwrap_or_default()==*os && a["schema_version"]=="tokenzero.os_release_artifact.v1") {
            let sr = a["shell_matrix"]=="run"; let ir = a["install_smoke"]=="run";
            let dr = a["daemon_required"].as_bool().unwrap_or(true); let gw = a["global_writes"].as_bool().unwrap_or(true);
            json!({"os":os,"current_host":false,"artifact_source":"external","artifact_path":a["artifact_path"],"shell_matrix":a["shell_matrix"],"install_smoke":a["install_smoke"],"daemon_required":dr,"global_writes":gw,"release_candidate_id":a["release_candidate_id"],"claim_ready":sr&&ir&&!dr&&!gw,"evidence":a["evidence"]})
        } else {
            json!({"os":os,"current_host":false,"artifact_source":"missing","shell_matrix":"not_run_on_this_host","install_smoke":"not_run_on_this_host","daemon_required":false,"global_writes":false,"release_candidate_id":serde_json::Value::Null,"claim_ready":false,"evidence":"no artifact from this host"})
        }
    }).collect();

    let all_run = os_rows.iter().all(|r| r["claim_ready"] == true);
    let rcid_list: Vec<_> = os_rows
        .iter()
        .filter(|r| r["claim_ready"] == true)
        .filter_map(|r| r["release_candidate_id"].as_str())
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .collect();
    let unique_ids: Vec<_> = rcid_list.iter().fold(Vec::new(), |mut ids, id| {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
        ids
    });
    let same_rc = all_run && rcid_list.len() == release_oses.len() && unique_ids.len() == 1;

    let mut blocked: Vec<_> = os_rows
        .iter()
        .filter(|r| r["claim_ready"] != true)
        .map(|r| {
            format!(
                "{} not run with shell and install artifacts",
                r["os"].as_str().unwrap_or("unknown")
            )
        })
        .collect();
    if all_run && !same_rc {
        blocked.push("OS release artifacts are not from the same release candidate".into());
    }
    if blocked.is_empty() && !release_approval {
        blocked.push("explicit release approval not granted".into());
    }
    let claim_ok = all_run && same_rc && release_approval;
    let ok = sm["ok"] == true
        && install["ok"] == true
        && install["global_writes"] == false
        && reach["daemon_required"] == false
        && reach["global_writes"] == false;
    let report = json!({"schema_version":"tokenzero.os_reach_audit.v1","status":if ok{"ok"}else{"blocked"},"ok":ok,"release_candidate_id":rcid,"current_os":cur,"daemon_required":false,"global_writes":false,"release_approval":release_approval,"public_os_claim_approved":claim_ok,"all_release_oses_run":all_run,"same_release_candidate":same_rc,"release_candidate_ids":unique_ids,"blocked_reasons":blocked,"external_artifact_count":ext.len(),"os_rows":os_rows,"shell_matrix":sm,"install_smoke":install,"core_surfaces":cs,"reach":reach});
    finish_artifact(&output_json, output_md.as_deref(), report, "OS reach audit")
}

pub(crate) fn run_os_release_artifact(
    output_json: PathBuf,
    output_md: Option<PathBuf>,
    root: PathBuf,
) -> Result<serde_json::Value> {
    let temp = tempdir()?;
    let sm = run_shell_matrix(temp.path().join("shell-matrix.json"), None)?;
    let install = run_install_smoke(None, true)?;
    let reach = run_reach(root, None)?;
    let cs = run_core_surface_audit(&sm, &install)?;
    let cur = release_os();
    let sr = sm[cur].as_str().unwrap_or("not_run") == "run";
    let ir = install["ok"] == true && install["global_writes"] == false;
    let dr = reach["daemon_required"] != false || cs.iter().any(|r| r["daemon_required"] != false);
    let gw = reach["global_writes"] != false
        || install["global_writes"] != false
        || cs.iter().any(|r| r["global_writes"] != false);
    let cs_ok = cs.iter().all(|r| r["ok"] == true);
    let ready = sr && ir && cs_ok && !dr && !gw;
    let report = json!({"schema_version":"tokenzero.os_release_artifact.v1","status":if ready{"ok"}else{"blocked"},"ok":ready,"release_candidate_id":release_candidate_id(),"os":cur,"shell_matrix":if sr{"run"}else{"not_run"},"install_smoke":if ir{"run"}else{"not_run"},"daemon_required":dr,"global_writes":gw,"claim_ready":ready,"release_publication_allowed":false,"evidence":"local os-release-artifact command; not a public OS-agnostic claim","shell_matrix_artifact":sm,"install_smoke_artifact":install,"core_surfaces":cs,"reach":reach});
    finish_artifact(
        &output_json,
        output_md.as_deref(),
        report,
        "OS release artifact",
    )
}

pub(crate) fn run_core_surface_audit(
    shell_matrix: &serde_json::Value,
    install_smoke: &serde_json::Value,
) -> Result<Vec<serde_json::Value>> {
    let temp = tempdir()?;
    let root = temp.path().to_path_buf();
    let doctor = doctor_report(&DoctorArgs {
        root: Some(root.clone()),
        cache_path: Some(root.join("recovery-cache.json")),
        runtime: true,
        json: true,
        robot_triage: false,
        fix: false,
        dry_run: false,
        explain: None,
        command: None,
    });
    let cache_engine = TokenZeroEngine::new(EngineConfig {
        allowed_roots: default_allowed_roots(&root),
        cache_path: root.join("recovery-cache.json"),
        max_visible_tokens: 4000,
        mode: Mode::Structured,
        shell_timeout: default_shell_timeout(),
        mcp_idle_timeout: None,
        ..EngineConfig::for_root(&root)
    });
    let cp = cache_engine.cache_pack("agent");
    let mcp = run_mcp_artifact(root.join("mcp-smoke.json"), None, 1)?;
    macro_rules! rows {
        ($(($surface:literal, $ok:expr, $evidence:literal, $details:expr));+ $(;)?) => {
            vec![$(core_surface_row($surface, $ok, $evidence, $details)),+]
        };
    }
    Ok(rows! {
        ("install", install_smoke["ok"] == true && install_smoke["global_writes"] == false, "install-smoke disposable local root", json!({"schema_version":install_smoke["schema_version"],"global_writes":install_smoke["global_writes"]}));
        ("doctor", doctor["ok"] == true, "doctor --runtime on disposable local root", json!({"schema_version":doctor["schema_version"],"root":doctor["root"]}));
        ("shell", shell_matrix["ok"] == true, "shell-matrix current host", json!({"schema_version":shell_matrix["schema_version"],"windows":shell_matrix["windows"],"linux":shell_matrix["linux"],"macos":shell_matrix["macos"]}));
        ("mcp", mcp["ok"] == true && mcp["unexpected_exits"] == 0, "mcp-smoke local stdio process", json!({"schema_version":mcp["schema_version"],"unexpected_exits":mcp["unexpected_exits"]}));
        ("cache_pack", cp.status == "ok", "cache-pack local recovery cache", json!({"tool":cp.tool,"status":cp.status,"refs":cp.refs.len()}));
    })
}

pub(crate) fn load_os_release_artifacts(paths: &[PathBuf]) -> Result<Vec<serde_json::Value>> {
    paths
        .iter()
        .map(|p| {
            let mut a: serde_json::Value = serde_json::from_slice(
                &fs::read(p).with_context(|| format!("read {}", p.display()))?,
            )
            .with_context(|| format!("parse {}", p.display()))?;
            a["artifact_path"] = json!(p.display().to_string());
            Ok(a)
        })
        .collect()
}
