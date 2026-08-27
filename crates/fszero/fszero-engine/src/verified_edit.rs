//! Journaled edit + optional verify command with rollback on failure.

use super::access_log::{content_hash_bytes, rel_path_for_log};
use super::edit_spec::{EditTarget, apply_unique_replace, parse_edit_spec};
use super::*;
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

const VERIFY_TIMEOUT_SECS: u64 = 120;
const VERIFY_TAIL_LINES: usize = 30;

#[cfg(test)]
std::thread_local! {
    static TEST_VERIFIED_EDIT_BETWEEN_READ_AND_WRITE: std::cell::Cell<Option<fn(&Path)>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
struct VerifiedEditInterfereGuard(Option<fn(&Path)>);

#[cfg(test)]
impl Drop for VerifiedEditInterfereGuard {
    fn drop(&mut self) {
        TEST_VERIFIED_EDIT_BETWEEN_READ_AND_WRITE.with(|hook| hook.set(self.0));
    }
}

#[cfg(test)]
fn test_verified_edit_between_read_and_write(
    interfere: Option<fn(&Path)>,
) -> VerifiedEditInterfereGuard {
    let previous = TEST_VERIFIED_EDIT_BETWEEN_READ_AND_WRITE.with(|hook| hook.replace(interfere));
    VerifiedEditInterfereGuard(previous)
}

#[inline]
fn ve0(detail: impl std::fmt::Display) -> String {
    super::op_result::op0("verifiedEdit", detail)
}

#[derive(Debug, Clone)]
struct EditHunk {
    old: String,
    new: String,
}

fn parse_verified_edit_args(
    args: &Value,
) -> Result<(String, Vec<EditHunk>, Option<String>), String> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| "missing path".to_string())?
        .to_string();
    let verify = args
        .get("verify")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let hunks = if let Some(edits) = args.get("edits").and_then(Value::as_array) {
        let mut out = Vec::new();
        for item in edits {
            let old = item
                .get("old")
                .or_else(|| item.get("find"))
                .and_then(Value::as_str)
                .ok_or_else(|| "edits[] requires old/find".to_string())?;
            let new = item
                .get("new")
                .or_else(|| item.get("replace"))
                .and_then(Value::as_str)
                .ok_or_else(|| "edits[] requires new/replace".to_string())?;
            out.push(EditHunk {
                old: old.to_string(),
                new: new.to_string(),
            });
        }
        if out.is_empty() {
            return Err("edits must be non-empty".to_string());
        }
        out
    } else if let Some(spec) = args.get("spec").and_then(Value::as_str) {
        let parsed = parse_edit_spec(spec).map_err(|e| e.to_string())?;
        let EditTarget::Path(_) = parsed.target else {
            return Err("verifiedEdit spec must be a path".to_string());
        };
        vec![EditHunk {
            old: parsed.old,
            new: parsed.new,
        }]
    } else {
        return Err("missing edits or spec".to_string());
    };

    Ok((path, hunks, verify))
}

fn line_delta(pre: &str, post: &str) -> String {
    let pre_lines = pre.lines().count();
    let post_lines = post.lines().count();
    if post_lines >= pre_lines {
        format!("+{}-{}", post_lines - pre_lines, 0)
    } else {
        format!("+{}-{}", 0, pre_lines - post_lines)
    }
}

fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= n {
        return text.to_string();
    }
    lines[lines.len() - n..].join("\n")
}

pub fn run_verify_command(root: &Path, command: &str) -> Result<(bool, String), String> {
    crate::runtime_metrics::record_process_start();
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("verify spawn failed: {e}"))?;

    let (stdout, stderr) = super::substrate_child::read_piped_stdio(&mut child);
    let status = wait_timeout_child(&mut child)?;

    let combined = format!("{stdout}{stderr}");
    Ok((status.success(), combined))
}

fn wait_timeout_child(child: &mut std::process::Child) -> Result<std::process::ExitStatus, String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(VERIFY_TIMEOUT_SECS);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("verify timed out after {VERIFY_TIMEOUT_SECS}s"));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("verify wait failed: {e}")),
        }
    }
}

impl FSZeroSession {
    pub fn do_verified_edit(&mut self, args: &Value) -> String {
        let root = match self.require_root() {
            Ok(r) => r.to_path_buf(),
            Err(e) => return ve0(e),
        };
        let (path_arg, hunks, verify_cmd) = match parse_verified_edit_args(args) {
            Ok(v) => v,
            Err(e) => return ve0(e),
        };

        let target_path = match validate_rollback_path(&root, Path::new(&path_arg)) {
            Ok(p) => p,
            Err(e) => return ve0(super::op_result::bad_path(e)),
        };
        if let Err(e) = ensure_path_under_root(Some(&root), &target_path) {
            return ve0(super::op_result::bad_path(e));
        }
        if let Err(e) = crate::path::refuse_non_regular_file(&target_path) {
            return ve0(e);
        }

        let source = match fs::read_to_string(&target_path) {
            Ok(t) => t,
            Err(e) => return ve0(super::op_result::read_failed(e)),
        };

        let mut updated = source.clone();
        for hunk in &hunks {
            updated = match apply_unique_replace(&updated, &hunk.old, &hunk.new) {
                Ok(t) => t,
                Err(e) => return ve0(e),
            };
        }

        if updated == source {
            return ve0("no change");
        }

        let pre_bytes = source.as_bytes().to_vec();
        let pre_meta = fs::metadata(&target_path).ok();
        let pre_mtime = pre_meta.as_ref().and_then(|m| m.modified().ok());
        let pre_perms = pre_meta.map(|m| m.permissions());
        let pre_xattrs = xattrs_of(&target_path);

        // Write-time TOCTOU guard (V6-F1 / ZS-SEC-001): the target resolved
        // canonical earlier, but a parent swapped for a symlink since then
        // must not redirect the verified write outside the root.
        if let Err(e) = guard_write_target_parent(&root, &target_path) {
            return ve0(super::op_result::bad_path(e));
        }
        if let Err(e) = crate::path::refuse_non_regular_file(&target_path) {
            return ve0(e);
        }
        // Re-read immediately before publish. Two sessions can both substitute
        // against the same preimage; without this check the later atomic_write
        // silently clobbers the first publisher (fszero-ai-filesystem-excellence-jqf.6.1).
        #[cfg(test)]
        TEST_VERIFIED_EDIT_BETWEEN_READ_AND_WRITE.with(|hook| {
            if let Some(interfere) = hook.get() {
                interfere(&target_path);
            }
        });
        match fs::read(&target_path) {
            Ok(live) if live == pre_bytes => {}
            Ok(_) => {
                return ve0("stale preimage: file changed before publish");
            }
            Err(e) => return ve0(super::op_result::read_failed(e)),
        }
        if atomic_write(&target_path, updated.as_bytes()).is_err() {
            return ve0("write failed");
        }

        let (pre_ref, post_ref) = self.put_pre_post(source.as_bytes(), updated.as_bytes());
        let delta = line_delta(&source, &updated);
        let rel = rel_path_for_log(Some(&root), &target_path);
        self.record_access("edit", &rel, &content_hash_bytes(updated.as_bytes()));
        self.refresh_path_after_mutation(&target_path);

        if let Some(cmd) = verify_cmd {
            match run_verify_command(&root, &cmd) {
                Ok((true, _out)) => {}
                Ok((false, out)) => {
                    if let Err(rb) = self.restore_file_for_rollback(
                        &target_path,
                        &pre_bytes,
                        pre_mtime,
                        pre_perms.clone(),
                        pre_xattrs.clone(),
                    ) {
                        return ve0(format!("verify failed; rollback failed: {rb}"));
                    }
                    let verify_tail = tail_lines(&out, VERIFY_TAIL_LINES);
                    let body = json!({
                        "ok": false, "ref_before": pre_ref, "ref_after": post_ref, "delta": delta, "verify_tail": verify_tail,
                    });
                    let payload = body.to_string();
                    let key = "verifiedEdit/err";
                    self.recovery.put_key(key, payload.as_bytes());
                    return format!("verifiedEdit:0 verify failed ref={key}");
                }
                Err(e) => {
                    let _ = self.restore_file_for_rollback(
                        &target_path,
                        &pre_bytes,
                        pre_mtime,
                        pre_perms,
                        pre_xattrs,
                    );
                    return ve0(e);
                }
            }
        }

        // Journal the mutation (fszero-chg): verifiedEdit writes were
        // invisible to fs.history / fs.undo. Recorded only after the verify
        // command passes — a verify-failed write already rolled itself back
        // and must not appear in the timeline. Fidelity meta comes from the
        // captured PRE state (the file on disk is already the post state).
        let pre_mtime_ns = pre_mtime
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
            .unwrap_or(0);
        #[cfg(unix)]
        let pre_mode = pre_perms
            .as_ref()
            .map(|p| {
                use std::os::unix::fs::PermissionsExt;
                p.mode() as i64
            })
            .unwrap_or(-1);
        #[cfg(not(unix))]
        let pre_mode = -1i64;
        let ve_seq = match self.record_mutation(
            "verifiedEdit",
            &rel,
            &pre_ref,
            &post_ref,
            false,
            pre_mtime_ns,
            pre_mode,
            &pre_xattrs.unwrap_or_default(),
        ) {
            Ok(seq) => seq,
            Err(e) => return ve0(super::op_result::journal_err(e)),
        };
        let body =
            json!({ "ok": true, "ref_before": pre_ref, "ref_after": post_ref, "delta": delta, });
        let payload = body.to_string();
        let key = "verifiedEdit/ok";
        self.recovery.put_key(key, payload.as_bytes());
        // V6-F1 (ZS-STORE-004): seal the uniform effect record and bind it
        // into the op receipt.
        let effects = self.seal_effect_record(
            "verifiedEdit",
            super::effect_capture::EffectScope::Session,
            vec![super::effect_capture::EffectPath {
                path: rel,
                action: super::effect_capture::EffectAction::Write,
                seq: ve_seq,
                pre_ref,
                post_ref,
            }],
            vec![],
        );
        let mut detail = format!("verifiedEdit:1 ref={key}");
        FSZeroSession::append_effect_token(&mut detail, &effects);
        detail
    }
}
