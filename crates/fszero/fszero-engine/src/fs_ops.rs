use super::access_log::{content_hash_bytes, rel_path_for_log};
use super::edit_spec::{EditSpec, EditTarget, apply_unique_replace, parse_edit_spec};
use super::op_result::{classify_op_result, visible_ack};
use super::*;

// Process-environment failpoints poison unrelated parallel libtest cases.
#[cfg(test)]
std::thread_local! {
    static TEST_EDIT_FAILPOINTS: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    static TEST_EDIT_BETWEEN_READ_AND_WRITE: std::cell::Cell<Option<fn(&std::path::Path)>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub struct TestEditFailpointsGuard(Option<&'static str>);

#[cfg(test)]
impl Drop for TestEditFailpointsGuard {
    fn drop(&mut self) {
        TEST_EDIT_FAILPOINTS.with(|failpoints| failpoints.set(self.0));
    }
}

#[cfg(test)]
pub fn test_edit_failpoints(value: Option<&'static str>) -> TestEditFailpointsGuard {
    let previous = TEST_EDIT_FAILPOINTS.with(|failpoints| failpoints.replace(value));
    TestEditFailpointsGuard(previous)
}

#[cfg(test)]
struct EditInterfereGuard(Option<fn(&std::path::Path)>);

#[cfg(test)]
impl Drop for EditInterfereGuard {
    fn drop(&mut self) {
        TEST_EDIT_BETWEEN_READ_AND_WRITE.with(|hook| hook.set(self.0));
    }
}

#[cfg(test)]
fn test_edit_between_read_and_write(interfere: Option<fn(&std::path::Path)>) -> EditInterfereGuard {
    let previous = TEST_EDIT_BETWEEN_READ_AND_WRITE.with(|hook| hook.replace(interfere));
    EditInterfereGuard(previous)
}

fn edit_fault(stage: &'static str) -> Result<(), String> {
    #[cfg(test)]
    let injected = TEST_EDIT_FAILPOINTS.with(|failpoints| {
        failpoints
            .get()
            .is_some_and(|value| value.split(',').any(|candidate| candidate == stage))
    });
    #[cfg(not(test))]
    let injected = std::env::var("FSZERO_EDIT_FAILPOINTS")
        .ok()
        .is_some_and(|value| value.split(',').any(|candidate| candidate == stage));
    if injected {
        Err(format!("fault injection at {stage}"))
    } else {
        Ok(())
    }
}

/// Process-kill oracle for the journal boundary matrix (fszero-k4ur.4).
/// Product behavior is unchanged unless the explicit test env names a stage.
fn maybe_crash_mutation_at(stage: &str) {
    if std::env::var("FSZERO_CRASH_MUTATION_AT").ok().as_deref() == Some(stage) {
        eprintln!("FSZERO_CRASH_MUTATION_AT={stage}: aborting");
        std::process::abort();
    }
}
fn rollback_edit(
    path: &std::path::Path,
    pre: &[u8],
    mtime: i64,
    mode: i64,
    xattrs: &str,
) -> Result<super::MutationState, String> {
    if let Err(e) = edit_fault("rollback") {
        return Err(e);
    }
    if let Err(e) = edit_fault("rollback_failure") {
        return Err(e);
    };
    match super::path::atomic_write_with_outcome(path, pre) {
        Ok(()) => {
            super::path::set_mode(path, mode)?;
            super::path::restore_xattrs(path, xattrs)?;
            super::path::set_mtime_ns(path, mtime)?;
            super::path::sync_file(path)?;
            Ok(super::MutationState::RolledBack)
        }
        Err(e) => Err(format!(
            "rollback {}: {e}",
            if e.published {
                "indeterminate"
            } else {
                "changed"
            }
        )),
    }
}

use std::sync::Arc;

/// CAS gate for base-anchored mutations (`fs.write` / `fs.edit` with `base`).
///
/// `MustNotExist` is the create gate (`base: null`); `MustMatch` carries the
/// 64-hex sha256 of the expected current content (`base: "fz://blob/<sha>"`).
/// Absence of `base` keeps the historical unconditional behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteBaseGate {
    MustNotExist,
    MustMatch(String),
}

/// Bounded line diff: skip the common prefix/suffix, then render one compact
/// hunk capped to `MAX_DIFF_LINES` removed plus added lines. Returns the diff
/// text and the (removed, added) line counts of the full change.
const MAX_DIFF_LINES: usize = 40;
pub fn bounded_line_diff(pre: &str, post: &str) -> (String, usize, usize) {
    let old: Vec<&str> = pre.lines().collect();
    let new: Vec<&str> = post.lines().collect();
    let mut start = 0usize;
    while start < old.len() && start < new.len() && old[start] == new[start] {
        start += 1;
    }
    let mut old_end = old.len();
    let mut new_end = new.len();
    while old_end > start && new_end > start && old[old_end - 1] == new[new_end - 1] {
        old_end -= 1;
        new_end -= 1;
    }
    let removed = old_end - start;
    let added = new_end - start;
    let mut out = format!("@@ -{},{removed} +{},{added} @@\n", start + 1, start + 1);
    let mut budget = MAX_DIFF_LINES;
    for line in &old[start..old_end] {
        if budget == 0 {
            out.push_str("… diff truncated\n");
            return (out, removed, added);
        }
        out.push('-');
        out.push_str(line);
        out.push('\n');
        budget -= 1;
    }
    for line in &new[start..new_end] {
        if budget == 0 {
            out.push_str("… diff truncated\n");
            return (out, removed, added);
        }
        out.push('+');
        out.push_str(line);
        out.push('\n');
        budget -= 1;
    }
    (out, removed, added)
}

#[inline]
fn write0(detail: impl std::fmt::Display) -> String {
    super::op_result::op0("write", detail)
}
#[inline]
fn transact0(detail: impl std::fmt::Display) -> String {
    super::op_result::op0("transact", detail)
}
#[inline]
fn undo0(detail: impl std::fmt::Display) -> String {
    super::op_result::op0("undo", detail)
}
#[inline]
fn edit0(detail: impl std::fmt::Display) -> String {
    super::op_result::op0("edit", detail)
}
#[inline]
fn expand0(detail: impl std::fmt::Display) -> String {
    super::op_result::op0("expand", detail)
}
#[inline]
fn history0(detail: impl std::fmt::Display) -> String {
    super::op_result::op0("history", detail)
}

impl FSZeroSession {
    /// Workspace-relative parent directories of `target` that do not exist yet
    /// (deepest-first) — exactly the set fs.write's `create_dir_all` will
    /// create. Captured BEFORE creation so the op's effect record lists them
    /// accurately.
    fn missing_parent_dirs(root: Option<&Path>, target: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = target.parent();
        while let Some(parent) = cur {
            if parent.exists() {
                break;
            }
            out.push(rel_path_for_log(root, parent));
            cur = parent.parent();
        }
        out
    }
    /// Public CLI/session execute — thin adapter over the typed domain dispatcher
    /// (fszero-ncib.2). All surfaces share `execute_kernel` for op semantics.
    pub fn execute(&mut self, code: char, arg: Option<&str>) -> (String, bool, Option<String>) {
        super::dispatcher::dispatch_opcode(self, super::dispatcher::DispatchSurface::Cli, code, arg)
            .into_execute_tuple()
    }

    /// Shared early-fail for kernel path (X0 + detail).
    fn fail_x0(&mut self, start: Instant, detail: String) -> (String, bool, Option<String>) {
        self.last_op_us = start.elapsed().as_micros();
        self.last_result = Some(detail.clone());
        ("X0".to_string(), false, Some(detail))
    }

    /// Shared early-fail when session has no workspace root.
    fn fail_missing_root(&mut self, start: Instant) -> (String, bool, Option<String>) {
        self.fail_x0(start, self.require_root().err().unwrap_or_default())
    }

    /// Tick op counters after root is confirmed; returns start Instant.
    /// Polls the in-flight MCP/CodeMode request guard here so every publishing
    /// kernel orifice (`execute_kernel`, edit-parts, snap-edit, CAS write,
    /// transact) fails closed before `do_*` mutation.
    fn begin_kernel_op(&mut self) -> Result<Instant, (String, bool, Option<String>)> {
        let start = Instant::now();
        if self.root.is_none() {
            return Err(self.fail_missing_root(start));
        }
        if self.request_expired() {
            let detail = self
                .request_expiry_detail()
                .unwrap_or("request cancelled")
                .to_string();
            return Err(self.fail_x0(start, detail));
        }
        self.record_internal_op();
        Ok(start)
    }

    /// Kernel op path: transport-neutral authorization, mutation, refs, telemetry.
    /// Called only from the domain dispatcher (and tests that pin kernel behavior).
    pub fn execute_kernel(
        &mut self,
        code: char,
        arg: Option<&str>,
    ) -> (String, bool, Option<String>) {
        let start = match self.begin_kernel_op() {
            Ok(s) => s,
            Err(fail) => return fail,
        };
        // Watch mode: fold pending FSEvents/inotify changes into the index
        // before the op observes it. Cheap no-op when the channel is empty.
        self.drain_watch_events();
        let Some(op) = OpCode::from_char(code) else {
            return self.fail_x0(
                start,
                format!(
                    "bad opcode: {code}; map: {}",
                    super::session::OPCODE_MAP_HINT
                ),
            );
        };
        // PathBuf clone is required: do_* take &mut self and Option<&Path> from
        // root; Arc root is a larger session-shape change deferred separately.
        let root = self.root.clone();
        let res_str = match op {
            OpCode::Ls => self.do_ls(root.as_deref(), arg),
            OpCode::Read => self.do_read(root.as_deref(), arg),
            OpCode::Search => self.do_search(root.as_deref(), arg),
            OpCode::Edit => self.do_edit(root.as_deref(), arg),
            OpCode::Compound => self.do_compound(root.as_deref(), arg),
            OpCode::Expand => self.do_expand(root.as_deref(), arg),
            OpCode::World => self.do_world(arg),
            OpCode::Stat => self.do_stat(root.as_deref(), arg),
            OpCode::Resolve => self.do_resolve(root.as_deref(), arg),
            OpCode::Write => self.do_write(root.as_deref(), arg),
            OpCode::History => self.do_history(arg),
            OpCode::Undo => self.do_undo(root.as_deref(), arg),
            OpCode::Memory => self.do_memory(arg.unwrap_or("")),
        };
        self.finish_op(op, start, res_str)
    }

    fn finish_op(
        &mut self,
        op: OpCode,
        start: Instant,
        mut res_str: String,
    ) -> (String, bool, Option<String>) {
        self.last_op_us = start.elapsed().as_micros();
        if let Some(budget_err) = self.enforce_ms_budget(start, op.as_letter()) {
            res_str = budget_err;
        }
        let detail = classify_op_result(&res_str);
        self.last_result = Some(res_str.clone());
        let visible = if detail.ok {
            visible_ack(op, Some(self.op_count))
        } else {
            detail.visible_error(op)
        };
        (visible, detail.ok, Some(res_str))
    }

    /// Structured edit entry — thin adapter over the domain dispatcher.
    pub fn execute_edit_parts(
        &mut self,
        path: &str,
        find: &str,
        replace: &str,
    ) -> (String, bool, Option<String>) {
        super::dispatcher::dispatch_edit_parts(
            self,
            super::dispatcher::DispatchSurface::Cli,
            path,
            find,
            replace,
        )
        .into_execute_tuple()
    }

    /// Kernel structured edit (dispatcher-only).
    pub fn execute_edit_parts_kernel(
        &mut self,
        path: &str,
        find: &str,
        replace: &str,
    ) -> (String, bool, Option<String>) {
        let start = match self.begin_kernel_op() {
            Ok(s) => s,
            Err(fail) => return fail,
        };
        let res = self.do_edit_parts(self.root.clone().as_deref(), path, find, replace);
        self.finish_op(OpCode::Edit, start, res)
    }

    /// Kernel structured edit constrained to a canonical line target.
    pub fn execute_edit_parts_window_kernel(
        &mut self,
        path: &str,
        find: &str,
        replace: &str,
        window: super::target_ref::LineWindow,
    ) -> (String, bool, Option<String>) {
        let start = match self.begin_kernel_op() {
            Ok(s) => s,
            Err(fail) => return fail,
        };
        let res =
            self.do_edit_parts_window(self.root.clone().as_deref(), path, find, replace, window);
        self.finish_op(OpCode::Edit, start, res)
    }

    /// One-dispatch fused discovery plus journaled target-window edit.
    pub fn execute_snap_edit_kernel(
        &mut self,
        query: &str,
        scope: &str,
        preimage: &str,
        replacement: &str,
    ) -> (String, bool, Option<String>) {
        let start = match self.begin_kernel_op() {
            Ok(s) => s,
            Err(fail) => return fail,
        };
        let res = self.do_snap_edit(
            self.root.clone().as_deref(),
            query,
            scope,
            preimage,
            replacement,
        );
        self.finish_op(OpCode::Edit, start, res)
    }

    /// CAS base-gate check for base-anchored mutations. On pass, returns the
    /// current on-disk bytes (`None` when the target does not exist). On
    /// violation, returns a named recovery detail that always carries a fresh
    /// `fz://blob/<sha>` ref of the current content, so the caller can retry
    /// against reality without a separate read.
    pub fn base_gate_check(
        &mut self,
        root: Option<&Path>,
        path: &str,
        gate: &WriteBaseGate,
    ) -> Result<Option<Vec<u8>>, String> {
        let Some(root_path) = root else {
            return Err("no root".to_string());
        };
        let target = validate_rollback_path(root_path, Path::new(path))
            .map_err(|e| super::op_result::bad_path(e))?;
        if let Err(e) = crate::path::refuse_non_regular_file(&target) {
            return Err(e);
        }
        let current = if target.exists() {
            Some(fs::read(&target).map_err(|e| super::op_result::read_failed(e))?)
        } else {
            None
        };
        match (gate, current.as_deref()) {
            (WriteBaseGate::MustNotExist, Some(bytes)) => {
                let fresh = self.recovery.put_content_ref(bytes);
                Err(format!(
                    "create conflict: {path} already exists; current content {fresh}; \
                     retry with base set to that ref to overwrite"
                ))
            }
            (WriteBaseGate::MustNotExist, None) => Ok(None),
            (WriteBaseGate::MustMatch(base_sha), None) => Err(format!(
                "stale preimage: {path} no longer exists (base fz://blob/{base_sha}); \
                 retry with base: null to create it"
            )),
            (WriteBaseGate::MustMatch(base_sha), Some(bytes)) => {
                let current_sha = content_hash_bytes(bytes);
                if &current_sha == base_sha {
                    Ok(current)
                } else {
                    let fresh = self.recovery.put_content_ref(bytes);
                    Err(format!(
                        "stale preimage: {path} changed since fz://blob/{base_sha}; \
                         current content {fresh}; re-anchor on that ref and retry"
                    ))
                }
            }
        }
    }

    /// Dispatcher-facing gate probe: `None` on pass, named detail on violation.
    pub fn base_gate_violation(&mut self, path: &str, gate: &WriteBaseGate) -> Option<String> {
        let root = self.root.clone();
        match self.base_gate_check(root.as_deref(), path, gate) {
            Ok(_) => None,
            Err(detail) => Some(detail),
        }
    }

    /// Kernel CAS write (dispatcher-only): base gate, then the journaled
    /// atomic write, then a bounded diff parked behind a named recovery ref.
    pub fn execute_write_cas_kernel(
        &mut self,
        path: &str,
        content: &str,
        gate: &WriteBaseGate,
    ) -> (String, bool, Option<String>) {
        let start = match self.begin_kernel_op() {
            Ok(s) => s,
            Err(fail) => return fail,
        };
        let root = self.root.clone();
        let res = self.do_write_cas(root.as_deref(), path, content, gate);
        self.finish_op(OpCode::Write, start, res)
    }

    fn do_write_cas(
        &mut self,
        root: Option<&Path>,
        path: &str,
        content: &str,
        gate: &WriteBaseGate,
    ) -> String {
        let pre_bytes = match self.base_gate_check(root, path, gate) {
            Ok(bytes) => bytes,
            Err(detail) => return write0(detail),
        };
        let ack = self.do_write(root, Some(&format!("{path}|{content}")));
        if !ack.starts_with("write:1") {
            return ack;
        }
        let pre_text = pre_bytes
            .as_deref()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .unwrap_or_default();
        let (diff, removed, added) = bounded_line_diff(&pre_text, content);
        let diff_ref = self
            .recovery
            .put_named_payload("write-diff", diff.as_bytes());
        if let Some(err) = self.store_error_suffix("write") {
            return err;
        }
        format!("{ack} removed={removed} added={added} diff={diff_ref}")
    }

    /// All-or-nothing multi-step mutation. Each step is
    /// `{op:"edit", path, find, replace, base?}` or
    /// `{op:"write", path, content, base?}` with the same CAS `base` gate as
    /// the single ops. Every gate is checked before any step applies; a step
    /// failure rolls back the already-applied steps in reverse order through
    /// the journaled undo path, so history stays coherent and undoable.
    pub fn execute_transact_kernel(
        &mut self,
        steps: &[serde_json::Value],
    ) -> (String, bool, Option<String>) {
        let start = match self.begin_kernel_op() {
            Ok(s) => s,
            Err(fail) => return fail,
        };
        let root = self.root.clone();
        let res = self.do_transact(root.as_deref(), steps);
        self.finish_op(OpCode::Write, start, res)
    }

    fn do_transact(&mut self, root: Option<&Path>, steps: &[serde_json::Value]) -> String {
        const MAX_TRANSACT_STEPS: usize = 64;
        if steps.is_empty() {
            return transact0("steps must be a non-empty array");
        }
        if steps.len() > MAX_TRANSACT_STEPS {
            return transact0(format!(
                "too many steps ({}); max {MAX_TRANSACT_STEPS}",
                steps.len()
            ));
        }
        enum PlannedOp {
            Edit { find: String, replace: String },
            Write { content: String },
        }
        struct Planned {
            path: String,
            op: PlannedOp,
            gate: Option<WriteBaseGate>,
        }
        // Phase 1: parse every step and check every CAS gate before any apply.
        let mut plans: Vec<Planned> = Vec::with_capacity(steps.len());
        for (index, step) in steps.iter().enumerate() {
            let Some(map) = step.as_object() else {
                return transact0(format!("step {index}: must be an object"));
            };
            let field = |key: &str| map.get(key).and_then(serde_json::Value::as_str);
            let Some(path) = field("path") else {
                return transact0(format!("step {index}: missing path"));
            };
            let gate = match map.get("base") {
                None => None,
                Some(serde_json::Value::Null) => Some(WriteBaseGate::MustNotExist),
                Some(serde_json::Value::String(reference)) => {
                    match reference
                        .strip_prefix("fz://blob/")
                        .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
                    {
                        Some(hex) => Some(WriteBaseGate::MustMatch(hex.to_ascii_lowercase())),
                        None => {
                            return transact0(format!(
                                "step {index}: base must be null or an fz://blob/<sha256> ref"
                            ));
                        }
                    }
                }
                Some(_) => {
                    return transact0(format!(
                        "step {index}: base must be null or an fz://blob/<sha256> ref"
                    ));
                }
            };
            let op = match field("op") {
                Some("edit") => {
                    if matches!(gate, Some(WriteBaseGate::MustNotExist)) {
                        return transact0(format!(
                            "step {index}: edit with base: null is not meaningful"
                        ));
                    }
                    match (
                        field("find").or_else(|| field("old")),
                        field("replace").or_else(|| field("new")),
                    ) {
                        (Some(find), Some(replace)) => PlannedOp::Edit {
                            find: find.to_owned(),
                            replace: replace.to_owned(),
                        },
                        _ => {
                            return transact0(format!(
                                "step {index}: edit requires find/old and replace/new"
                            ));
                        }
                    }
                }
                Some("write") => PlannedOp::Write {
                    content: field("content").unwrap_or("").to_owned(),
                },
                other => {
                    return transact0(format!(
                        "step {index}: op must be \"edit\" or \"write\"; got {other:?}"
                    ));
                }
            };
            if let Some(gate) = gate.as_ref() {
                if let Err(detail) = self.base_gate_check(root, path, gate) {
                    return transact0(format!("step {index}: {detail}"));
                }
            }
            plans.push(Planned {
                path: path.to_owned(),
                op,
                gate,
            });
        }
        // Phase 2: apply in order; roll back applied steps in reverse on failure.
        let mut applied: Vec<(usize, String, String)> = Vec::with_capacity(plans.len());
        for (index, plan) in plans.iter().enumerate() {
            let ack = match &plan.op {
                PlannedOp::Edit { find, replace } => {
                    self.do_edit_parts(root, &plan.path, find, replace)
                }
                PlannedOp::Write { content } => match plan.gate.as_ref() {
                    Some(gate) => self.do_write_cas(root, &plan.path, content, gate),
                    None => self.do_write(root, Some(&format!("{}|{content}", plan.path))),
                },
            };
            let ok = ack.starts_with("edit:1") || ack.starts_with("write:1");
            if ok {
                applied.push((index, plan.path.clone(), ack));
                continue;
            }
            let mut rolled_back = 0usize;
            let mut rollback_failures: Vec<String> = Vec::new();
            for (_, path, _) in applied.iter().rev() {
                let undo = self.do_undo(root, Some(path));
                if undo.starts_with("undo:1") {
                    rolled_back += 1;
                } else {
                    rollback_failures.push(format!("{path}: {undo}"));
                }
            }
            let mut detail = format!(
                "step {index} failed: {ack}; rolled_back={rolled_back}/{}",
                applied.len()
            );
            if !rollback_failures.is_empty() {
                self.set_mutation_outcome(super::MutationOutcome::new(
                    super::MutationState::Indeterminate,
                    "transact",
                    Some(rollback_failures.join("; ")),
                ));
                detail.push_str(&format!(
                    "; ROLLBACK INCOMPLETE: {}",
                    rollback_failures.join("; ")
                ));
            }
            return transact0(detail);
        }
        let receipt: Vec<serde_json::Value> = applied
            .iter()
            .map(|(index, path, ack)| serde_json::json!({"step": index, "path": path, "ack": ack}))
            .collect();
        let receipt_ref = self.recovery.put_named_payload(
            "transact",
            serde_json::Value::Array(receipt).to_string().as_bytes(),
        );
        if let Some(err) = self.store_error_suffix("transact") {
            return err;
        }
        // V6-F1 (ZS-STORE-004): seal the batch's uniform effect record — the
        // union of the per-step effect records (each step's `effects=` token
        // is bound into this receipt's rows too), so pre/post refs and seqs
        // stay journal-accurate. Deterministic: per-step records are sorted,
        // merged in step order.
        let mut batch_paths: Vec<super::effect_capture::EffectPath> = Vec::new();
        let mut batch_refused: Vec<String> = Vec::new();
        for (_, _, ack) in &applied {
            if let Some(eff_ref) = ack
                .split_whitespace()
                .find_map(|tok| tok.strip_prefix("effects="))
            {
                if let Some(bytes) = self.recovery.expand(eff_ref) {
                    if let Ok(rec) =
                        serde_json::from_slice::<super::effect_capture::EffectRecord>(&bytes)
                    {
                        batch_paths.extend(rec.paths);
                        batch_refused.extend(rec.refused);
                    }
                }
            }
        }
        let effects = self.seal_effect_record(
            "transact",
            super::effect_capture::EffectScope::Session,
            batch_paths,
            batch_refused,
        );
        let mut detail = format!("transact:1 steps={} receipt={receipt_ref}", applied.len());
        Self::append_effect_token(&mut detail, &effects);
        detail
    }

    /// Create-or-overwrite a file with full content. Spec: `path|content`
    /// (first `|` splits; content may contain anything). Unlike edit, the
    /// target may not exist yet — the CodeMode write surface previously had
    /// NO create path (the adapter emulated write as read-then-edit, which
    /// failed on new files and stranded agents in fallback loops).
    pub fn do_write(&mut self, root: Option<&Path>, arg: Option<&str>) -> String {
        let Some(spec) = arg else {
            return write0("missing spec");
        };
        let Some((path_part, content)) = spec.split_once('|') else {
            return write0("spec must be path|content");
        };
        let path_part = path_part.trim();
        if path_part.is_empty() {
            return write0("empty path");
        }
        let Some(root_path) = root else {
            return write0("no root");
        };
        // Root-guarded resolve that tolerates a not-yet-existing target
        // (same validator the rollback path trusts). Refusals seal an effect
        // record (`effects=` receipt token) so the escape attempt fails loud
        // AND is receipted (V6-F1 / ZS-SEC-001).
        let target = match validate_rollback_path(root_path, Path::new(path_part)) {
            Ok(p) => p,
            Err(e) => {
                let effects = self.seal_effect_record(
                    "write",
                    super::effect_capture::EffectScope::Session,
                    vec![],
                    vec![path_part.to_string()],
                );
                let mut detail = super::op_result::bad_path(e);
                Self::append_effect_token(&mut detail, &effects);
                return write0(detail);
            }
        };
        // Parent directories fs.write's create_dir_all is about to create
        // (deepest-first), captured for the effect record BEFORE they exist.
        let missing_parents = Self::missing_parent_dirs(root, &target);
        // Write-time TOCTOU guard: re-verify the target's parent still
        // resolves inside the root right before we create dirs / publish
        // (a parent swapped for a symlink since validation cannot redirect
        // the write outside the root).
        if let Err(e) = guard_write_target_parent(root_path, &target) {
            let effects = self.seal_effect_record(
                "write",
                super::effect_capture::EffectScope::Session,
                vec![],
                vec![path_part.to_string()],
            );
            let mut detail = super::op_result::bad_path(e);
            Self::append_effect_token(&mut detail, &effects);
            return write0(detail);
        }
        if let Err(e) = crate::path::refuse_non_regular_file(&target) {
            return write0(e);
        }
        let created = !target.exists();
        // Pre-content ref BEFORE overwriting: without it an overwrite is
        // un-undoable (edit always had pre+post; write only stored post).
        let pre_bytes = if created {
            None
        } else {
            fs::read(&target).ok()
        };
        let (pre_mtime_ns, pre_mode, pre_xattrs) = if created {
            (0, -1, String::new())
        } else {
            file_meta_snapshot(&target)
        };
        if let Some(parent) = target.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return write0(format!("mkdir failed: {e}"));
            }
        }
        if let Err(e) = guard_write_target_parent(root_path, &target) {
            let effects = self.seal_effect_record(
                "write",
                super::effect_capture::EffectScope::Session,
                vec![],
                vec![path_part.to_string()],
            );
            let mut detail = super::op_result::bad_path(e);
            Self::append_effect_token(&mut detail, &effects);
            return write0(detail);
        }
        if let Err(e) = atomic_write(&target, content.as_bytes()) {
            let state = if e.contains("after publication") {
                super::MutationState::Indeterminate
            } else {
                super::MutationState::MutationFree
            };
            self.set_mutation_outcome(super::MutationOutcome::new(
                state,
                "write",
                Some(target.display().to_string()),
            ));
            return write0(e);
        }
        let pre_ref = pre_bytes
            .as_deref()
            .map(|bytes| self.recovery.put_content_ref(bytes))
            .unwrap_or_default();
        // Content-addressed post ref (expandable) + named write-post slot so
        // CodeMode `fs.write` finalize_step and agents can expand the exact
        // bytes without re-reading the file. Named key (not fz://seq/…) so
        // expand is not rejected as execution-scoped.
        let post_ref = self
            .recovery
            .put_named_payload("write-post", content.as_bytes());
        if let Some(err) = self.store_error_suffix("write") {
            return err;
        }
        let rel = rel_path_for_log(root, &target);
        let hash = content_hash_bytes(content.as_bytes());
        self.record_access("write", &rel, &hash);
        let write_seq = match self.record_mutation(
            "write",
            &rel,
            &pre_ref,
            &post_ref,
            created,
            pre_mtime_ns,
            pre_mode,
            &pre_xattrs,
        ) {
            Ok(seq) => seq,
            Err(e) => {
                // Fail-closed: undo the published write so we never ack a hole.
                if created {
                    let _ = fs::remove_file(&target);
                } else if let Some(bytes) = pre_bytes.as_deref() {
                    let _ = atomic_write(&target, bytes);
                }
                self.refresh_path_after_mutation(&target);
                return write0(super::op_result::journal_err(e));
            }
        };
        // V6-F1 (ZS-STORE-004): seal the uniform effect record — file
        // write/create plus any parent directories create_dir_all created —
        // and bind it into the op receipt.
        let mut effect_paths = vec![super::effect_capture::EffectPath {
            path: rel.clone(),
            action: if created {
                super::effect_capture::EffectAction::Create
            } else {
                super::effect_capture::EffectAction::Write
            },
            seq: write_seq,
            pre_ref: pre_ref.clone(),
            post_ref: post_ref.clone(),
        }];
        effect_paths.extend(missing_parents.into_iter().map(|dir| {
            super::effect_capture::EffectPath {
                path: dir,
                action: super::effect_capture::EffectAction::Create,
                seq: write_seq,
                pre_ref: String::new(),
                post_ref: String::new(),
            }
        }));
        let effects = self.seal_effect_record(
            "write",
            super::effect_capture::EffectScope::Session,
            effect_paths,
            vec![],
        );
        self.refresh_path_after_mutation(&target);
        let mut detail = format!(
            "write:1 {rel} bytes={} created={created} pre={pre_ref} post={post_ref}",
            content.len()
        );
        Self::append_effect_token(&mut detail, &effects);
        detail
    }

    /// Mutation timeline for a path (or the whole repo). Spec: `path`,
    /// `path|N`, `|N`, or empty (default limit 20). Rows newest-first; the
    /// full JSON timeline is stored behind a content ref for expand.
    pub fn do_history(&mut self, arg: Option<&str>) -> String {
        let spec = arg.unwrap_or("").trim();
        let (path_part, limit) = match spec.rsplit_once('|') {
            Some((p, n)) => match n.trim().parse::<usize>() {
                Ok(v) if v > 0 => (p.trim(), v.min(500)),
                _ => return history0("bad limit"),
            },
            None => (spec, 20),
        };
        let rows = self.recovery.query_mutations(path_part, None, limit);
        let json_rows: Vec<serde_json::Value> = rows.iter().map(|row| {
                serde_json::json!({
                    "seq": row.seq, "ts": row.ts, "op": row.op, "path": row.path, "pre_ref": row.pre_ref, "post_ref": row.post_ref, "created": row.created,
                    "agent": row.agent, "pre_mtime_ns": row.pre_mtime_ns, "pre_mode": row.pre_mode, "pre_xattrs": row.pre_xattrs,
                })
            }).collect();
        let payload = serde_json::Value::Array(json_rows).to_string();
        let r = self.recovery.put_content_ref(payload.as_bytes());
        if let Some(err) = self.store_error_suffix("history") {
            return err;
        }
        let head: Vec<String> = rows
            .iter()
            .take(5)
            .map(|row| format!("{}:{}:{}", row.seq, row.op, row.path))
            .collect();
        format!("history:1 n={} ref={r} {}", rows.len(), head.join(" "))
    }

    /// Revert a journaled mutation. Spec: `path` (latest mutation for the
    /// path) or `path|seq`. Preimage-guarded: the file's CURRENT content
    /// must still match the mutation's post-state, else `undo:0 (stale`.
    /// Undoing a `created` write deletes the file. The undo itself is
    /// journaled (op=undo, refs reversed), so undo is redoable via history.
    fn finish_undo_rollback(
        &mut self,
        intent_id: i64,
        evidence_txn: bool,
        target: &Path,
        current: &[u8],
        mtime: i64,
        mode: i64,
        xattrs: &str,
        boundary: &str,
        message: String,
    ) -> String {
        self.recovery.rollback_exec_txn(evidence_txn);
        match rollback_edit(target, current, mtime, mode, xattrs) {
            Ok(state) => {
                self.refresh_path_after_mutation(target);
                match self.recovery.clear_edit_intent(intent_id) {
                    Ok(()) => self.edit_outcome_error(state, boundary, message),
                    Err(error) => self.edit_outcome_error(
                        super::MutationState::Indeterminate,
                        "finalize",
                        format!("{message}; undo intent clear failed: {error}"),
                    ),
                }
            }
            Err(error) => self.edit_outcome_error(
                super::MutationState::Indeterminate,
                "rollback",
                format!("{message}; undo rollback failed: {error}"),
            ),
        }
    }

    pub fn do_undo(&mut self, root: Option<&Path>, arg: Option<&str>) -> String {
        let spec = arg.unwrap_or("").trim();
        if spec.is_empty() {
            return undo0("missing path");
        }
        let (path_part, seq) = match spec.rsplit_once('|') {
            Some((p, n)) => match n.trim().parse::<i64>() {
                Ok(v) => (p.trim(), Some(v)),
                Err(_) => return undo0("bad seq"),
            },
            None => (spec, None),
        };
        let Some(root_path) = root else {
            return undo0("no root");
        };
        let rows = self.recovery.query_mutations(path_part, seq, 1);
        let Some(row) = rows.first().cloned() else {
            return undo0("no mutation recorded");
        };
        let (seq, op, path, pre_ref, post_ref, created) = (
            row.seq,
            row.op,
            row.path,
            row.pre_ref,
            row.post_ref,
            row.created,
        );
        if path != path_part {
            return undo0(format!("seq {seq} is for {path}, not {path_part}"));
        }
        if op == "undo" {
            return undo0(format!("seq {seq} is itself an undo; use history to redo"));
        }
        let target = match validate_rollback_path(root_path, Path::new(&path)) {
            Ok(p) => p,
            Err(e) => return undo0(super::op_result::bad_path(e)),
        };

        // Stale guard: refuse when someone changed the file after the
        // mutation being undone. FIFO/socket open blocks; refuse from
        // metadata before the content read.
        if let Err(e) = crate::path::refuse_non_regular_file(&target) {
            return undo0(e);
        }
        let current = fs::read(&target).unwrap_or_default();
        let current_hash = content_hash_bytes(&current);
        let post_hash = post_ref.rsplit('/').next().unwrap_or("");
        if current_hash != post_hash {
            return undo0(format!(
                "stale: current content no longer matches seq {seq} post-state"
            ));
        }
        let desired = if created {
            Vec::new()
        } else {
            let Some(bytes) = self.recovery.get_payload(&pre_ref) else {
                return undo0(format!("pre-content unrecoverable: {pre_ref}"));
            };
            bytes
        };

        // Prepare a FULL-synchronous intent before changing workspace bytes.
        // Reopen rolls a prepared undo back to `current`; evidence_ready means
        // the reverse history row and postimage committed together.
        let (undo_pre_mtime_ns, undo_pre_mode, undo_pre_xattrs) = file_meta_snapshot(&target);
        let rel = rel_path_for_log(root, &target);
        let root_id = root_path.to_string_lossy().into_owned();
        if self.codemode_edit_plan && !self.pending_edit_intents.is_empty() {
            return undo0(
                "conflict: multiple crash-atomic mutations in one CodeMode plan are unsupported",
            );
        }
        if self.codemode_edit_plan && self.recovery.exec_txn_active.get() {
            if let Err(error) = self.recovery.suspend_exec_txn() {
                return self.edit_outcome_error(
                    super::MutationState::MutationFree,
                    "intent_commit",
                    error,
                );
            }
        }
        if self.recovery.has_edit_intent(&root_id, &rel) {
            return self.edit_outcome_error(
                super::MutationState::Indeterminate,
                "reopen",
                format!("unresolved mutation intent blocks {rel}"),
            );
        }
        let intent_id = match self.recovery.create_edit_intent(
            &root_id,
            &rel,
            &current,
            &desired,
            "",
            "",
            undo_pre_mtime_ns,
            undo_pre_mode,
            &undo_pre_xattrs,
        ) {
            Ok(id) => id,
            Err(error) => {
                return self.edit_outcome_error(
                    super::MutationState::MutationFree,
                    "intent_commit",
                    format!("undo intent failed: {error}"),
                );
            }
        };
        if self.codemode_edit_plan {
            self.pending_edit_intents.push(intent_id);
        }
        // Write-time TOCTOU guard (V6-F1 / ZS-SEC-001): the target resolved
        // canonical earlier, but a parent swapped for a symlink since then
        // must not redirect the removal/restore outside the root.
        if let Err(e) = guard_write_target_parent(root_path, &target) {
            let _ = self.recovery.clear_edit_intent(intent_id);
            return undo0(super::op_result::bad_path(e));
        }

        if created {
            if let Err(error) = fs::remove_file(&target) {
                let _ = self.recovery.clear_edit_intent(intent_id);
                return undo0(format!("remove failed: {error}"));
            }
        } else if let Err(error) = atomic_write(&target, &desired) {
            if error.contains("after publication") {
                return self.finish_undo_rollback(
                    intent_id,
                    false,
                    &target,
                    &current,
                    undo_pre_mtime_ns,
                    undo_pre_mode,
                    &undo_pre_xattrs,
                    "publication",
                    format!("undo failed after publication: {error}"),
                );
            }
            let _ = self.recovery.clear_edit_intent(intent_id);
            self.set_mutation_outcome(super::MutationOutcome::new(
                super::MutationState::MutationFree,
                "undo",
                Some(target.display().to_string()),
            ));
            return undo0(format!("write failed: {error}"));
        }
        maybe_crash_mutation_at("undo_after_publish");

        let mut mtime_note = String::new();
        if !created {
            if let Err(e) = set_mode(&target, row.pre_mode) {
                mtime_note = format!(" mode=drifted({e})");
            }
            if let Err(e) = restore_xattrs(&target, &row.pre_xattrs) {
                mtime_note.push_str(&format!(" xattrs=drifted({e})"));
            }
            if let Err(e) = set_mtime_ns(&target, row.pre_mtime_ns) {
                mtime_note.push_str(&format!(" mtime=drifted({e})"));
            }
        }
        self.refresh_path_after_mutation(&target);

        let evidence_txn = if self.recovery.is_durable() {
            self.recovery.begin_exec_txn()
        } else {
            false
        };
        if self.recovery.is_durable() && !evidence_txn {
            return self.finish_undo_rollback(
                intent_id,
                false,
                &target,
                &current,
                undo_pre_mtime_ns,
                undo_pre_mode,
                &undo_pre_xattrs,
                "history",
                "undo evidence transaction failed to begin".into(),
            );
        }
        if evidence_txn {
            self.recovery.mark_exec_txn_full();
        }
        if let Err(error) = self
            .recovery
            .set_edit_intent_refs(intent_id, &post_ref, &pre_ref)
        {
            return self.finish_undo_rollback(
                intent_id,
                evidence_txn,
                &target,
                &current,
                undo_pre_mtime_ns,
                undo_pre_mode,
                &undo_pre_xattrs,
                "history",
                error,
            );
        }
        let undo_seq = match self.record_mutation(
            "undo",
            &rel,
            &post_ref,
            &pre_ref,
            false,
            undo_pre_mtime_ns,
            undo_pre_mode,
            &undo_pre_xattrs,
        ) {
            Ok(seq) => seq,
            Err(error) => {
                return self.finish_undo_rollback(
                    intent_id,
                    evidence_txn,
                    &target,
                    &current,
                    undo_pre_mtime_ns,
                    undo_pre_mode,
                    &undo_pre_xattrs,
                    "history",
                    super::op_result::journal_err(error),
                );
            }
        };
        maybe_crash_mutation_at("undo_before_commit");
        if !self.codemode_edit_plan {
            if let Err(error) = self.recovery.clear_edit_intent(intent_id) {
                return self.finish_undo_rollback(
                    intent_id,
                    evidence_txn,
                    &target,
                    &current,
                    undo_pre_mtime_ns,
                    undo_pre_mode,
                    &undo_pre_xattrs,
                    "finalize",
                    error,
                );
            }
            self.recovery.commit_exec_txn(evidence_txn);
            if let Some(error) = self.recovery.take_store_error() {
                return self.finish_undo_rollback(
                    intent_id,
                    false,
                    &target,
                    &current,
                    undo_pre_mtime_ns,
                    undo_pre_mode,
                    &undo_pre_xattrs,
                    "history",
                    error,
                );
            }
        }
        self.record_access("undo", &rel, &current_hash);
        // V6-F1 (ZS-STORE-004): seal the uniform effect record. An undo of a
        // created file removes it (Delete); an undo of a written file
        // restores the pre-image (Write).
        let effects = self.seal_effect_record(
            "undo",
            super::effect_capture::EffectScope::Session,
            vec![super::effect_capture::EffectPath {
                path: rel.clone(),
                action: if created {
                    super::effect_capture::EffectAction::Delete
                } else {
                    super::effect_capture::EffectAction::Write
                },
                seq: undo_seq,
                pre_ref: post_ref.clone(),
                post_ref: pre_ref.clone(),
            }],
            vec![],
        );
        let mut detail =
            format!("undo:1 {rel} reverted seq={seq} op={op} restored={pre_ref}{mtime_note}");
        Self::append_effect_token(&mut detail, &effects);
        detail
    }

    /// Journal one repo-file mutation (basis for fs.history / fs.undo).
    /// `pre_mtime_ns` (0 = unknown), `pre_mode` (-1 = unknown) and
    /// `pre_xattrs` ('' = unknown) capture the file's metadata immediately
    /// before this mutation wrote, so undo restores all of it exactly
    /// (fszero-md6 / fszero-7be / fszero-l4g). Returns the journal seq of
    /// the appended row (V6-F1: effect records cross-check against it).
    #[allow(clippy::too_many_arguments)]
    pub fn record_mutation(
        &mut self,
        op: &str,
        rel_path: &str,
        pre_ref: &str,
        post_ref: &str,
        created: bool,
        pre_mtime_ns: i64,
        pre_mode: i64,
        pre_xattrs: &str,
    ) -> Result<i64, String> {
        // Degraded sessions have no durable journal; skip without claiming a hole.
        if self.durable_degraded {
            return Ok(0);
        }
        let ts = super::recovery::unix_epoch_secs();
        let agent = std::env::var("FSZERO_AGENT_ID").unwrap_or_default();
        let window = self.access_session_window;
        self.recovery.append_mutation(
            ts,
            op,
            rel_path,
            pre_ref,
            post_ref,
            created,
            window,
            &agent,
            pre_mtime_ns,
            pre_mode,
            pre_xattrs,
        )
    }

    pub fn do_edit(&mut self, root: Option<&Path>, arg: Option<&str>) -> String {
        let Some(spec_str) = arg else {
            return edit0("missing spec");
        };
        let spec = match parse_edit_spec(spec_str) {
            Ok(spec) => spec,
            Err(e) => return edit0(e),
        };
        self.do_edit_spec(root, spec, None)
    }

    /// Structured entry that uses the edit spec directly, bypassing the
    /// `path:old|new` string grammar entirely. The object form avoids any
    /// escaping rules for `|` and is the recommended path for payloads
    /// containing closures, pattern alternation, or pipe characters
    /// (fszero-edit-spec-pipe-escape-beh).
    pub fn do_edit_parts(
        &mut self,
        root: Option<&Path>,
        path: &str,
        find: &str,
        replace: &str,
    ) -> String {
        self.do_edit_spec(
            root,
            EditSpec {
                target: EditTarget::Path(path.to_string()),
                old: find.to_string(),
                new: replace.to_string(),
            },
            None,
        )
    }

    /// Structured entry constrained to the exact canonical discovery window.
    pub fn do_edit_parts_window(
        &mut self,
        root: Option<&Path>,
        path: &str,
        find: &str,
        replace: &str,
        window: super::target_ref::LineWindow,
    ) -> String {
        self.do_edit_spec(
            root,
            EditSpec {
                target: EditTarget::Path(path.to_string()),
                old: find.to_string(),
                new: replace.to_string(),
            },
            Some(window),
        )
    }

    /// Resolve and root-confine `scope` before one capped literal scan,
    /// then edit the unique canonical workspace-relative target window.
    fn do_snap_edit(
        &mut self,
        root: Option<&Path>,
        query: &str,
        scope: &str,
        preimage: &str,
        replacement: &str,
    ) -> String {
        let Some(root) = root else {
            return edit0("bad root: fused snap mutation requires a workspace root");
        };
        let scope_path = Path::new(scope);
        if scope_path.is_absolute()
            || scope_path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return edit0("outside root: fused snap scope must be workspace-relative");
        }
        if !matches!(
            super::search::classify_search_query(Some(query), query),
            super::search::SearchRoute::Grep
        ) {
            return edit0(
                "invalid argument: fused snap mutation requires a literal discovery query",
            );
        }
        let resolved_scope = match self.resolve_existing_path_cached(Some(root), scope) {
            Ok(path) => path,
            Err(error) => return edit0(super::op_result::bad_path(error)),
        };
        let root_canon = match self.root_canon.clone() {
            Some(path) => path,
            None => match canonicalize_root(root) {
                Ok(path) => path,
                Err(error) => return edit0(super::op_result::bad_path(error)),
            },
        };
        let scope_relative = match resolved_scope.strip_prefix(root_canon.as_path()) {
            Ok(path) => path,
            Err(_) => {
                return edit0("outside root: fused snap scope resolved outside the workspace");
            }
        };
        let scope_key = scope_relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let file_scope = match fs::metadata(&resolved_scope) {
            Ok(metadata) if metadata.is_file() => true,
            Ok(metadata) if metadata.is_dir() => false,
            Ok(_) => {
                return edit0("invalid argument: fused snap scope must be a file or directory");
            }
            Err(error) => return edit0(super::op_result::read_failed(error)),
        };
        if let Err(error) = self.prepare_index_or_busy(Some(root)) {
            return edit0(error);
        }
        let use_prefilter = super::ast_sgrep::literal_prefilter_from_env()
            == super::ast_sgrep::LiteralPrefilter::BigramMemmem;
        let payload = super::search::build_scoped_literal_payload(
            root_canon.as_path(),
            &self.index,
            query,
            &scope_key,
            file_scope,
            use_prefilter.then_some(&mut self.lazy_bigrams),
        );
        if super::search::search_hit_count(&payload) >= super::search::GREP_HIT_LIMIT {
            return edit0(format!(
                "budget exceeded: fused snap uniqueness is uncertified because in-scope literal discovery reached its {}-hit payload cap",
                super::search::GREP_HIT_LIMIT
            ));
        }
        let mut targets = Vec::new();
        for line in payload.lines() {
            let Some(target) = line
                .strip_prefix("HIT ")
                .and_then(|line| line.split_whitespace().next())
            else {
                continue;
            };
            let Some((path, window)) = super::target_ref::parse_target_ref(target) else {
                continue;
            };
            if !targets
                .iter()
                .any(|(seen, seen_window)| seen == path && *seen_window == window)
            {
                targets.push((path.to_string(), window));
            }
        }
        match targets.as_slice() {
            [] => edit0("not found: fused snap query has no target in scope"),
            [(path, window)] => {
                self.do_edit_parts_window(Some(root), path, preimage, replacement, *window)
            }
            _ => edit0(format!(
                "conflict: fused snap query matched {} targets in scope",
                targets.len()
            )),
        }
    }

    /// Source text for a view-based edit target, or `"stale ref, no view"`.
    fn view_text_for_edit(&self, id: u32) -> Result<(PathBuf, String), &'static str> {
        self.resolve_view_for_edit(id)
            .map(|(p, b)| (p, String::from_utf8_lossy(&b).into_owned()))
            .ok_or("stale ref, no view")
    }

    /// Root-guard + re-read preimage check (TOCTOU-safe, used twice in edit).
    fn revalidate_edit_preimage(
        &self,
        root: Option<&Path>,
        path: &Path,
        source: &str,
    ) -> Result<(), String> {
        ensure_path_under_root(root, path).map_err(|e| super::op_result::bad_path(e))?;
        // Second TOCTOU read of the same user path; refuse FIFO/socket before open.
        crate::path::refuse_non_regular_file(path)?;
        let verify = fs::read_to_string(path).map_err(|e| super::op_result::read_failed(e))?;
        if verify != source {
            return Err("stale preimage".to_string());
        }
        Ok(())
    }

    fn edit_outcome_error(
        &mut self,
        state: super::MutationState,
        boundary: &str,
        message: impl Into<String>,
    ) -> String {
        self.set_mutation_outcome(super::MutationOutcome::new(state, boundary, None));
        edit0(message.into())
    }

    fn finish_edit_rollback(
        &mut self,
        intent_id: i64,
        edit_txn: bool,
        path: &Path,
        rollback: Result<super::MutationState, String>,
        boundary: &str,
        message: String,
    ) -> String {
        if edit_txn {
            self.recovery.rollback_exec_txn(true);
        }
        match rollback {
            Ok(state) => {
                self.refresh_path_after_mutation(path);
                match self.recovery.clear_edit_intent(intent_id) {
                    Ok(()) => self.edit_outcome_error(state, boundary, message),
                    Err(error) => self.edit_outcome_error(
                        super::MutationState::Indeterminate,
                        "finalize",
                        format!("{message}; intent clear failed: {error}"),
                    ),
                }
            }
            Err(error) => self.edit_outcome_error(
                super::MutationState::Indeterminate,
                "rollback",
                format!("{message}; {error}"),
            ),
        }
    }
    fn do_edit_spec(
        &mut self,
        root: Option<&Path>,
        spec: EditSpec,
        window: Option<super::target_ref::LineWindow>,
    ) -> String {
        self.last_mutation_outcome = None;
        let (target_path, source_text) = match spec.target {
            EditTarget::Path(path_arg) => {
                // Write/edit must not canonicalize through a tail symlink
                // (filesystem-v1 replace-link-entry). Reads still follow.
                let target_path = match root {
                    Some(root_path) => {
                        match validate_rollback_path(root_path, Path::new(&path_arg)) {
                            Ok(p) => p,
                            Err(e) => return edit0(super::op_result::bad_path(e)),
                        }
                    }
                    None => match self.resolve_existing_path_cached(None, &path_arg) {
                        Ok(p) => p,
                        Err(e) => return edit0(super::op_result::bad_path(e)),
                    },
                };
                if let Err(e) = crate::path::refuse_non_regular_file(&target_path) {
                    return edit0(e);
                }
                match fs::read_to_string(&target_path) {
                    Ok(text) => (target_path, text),
                    Err(e) => return edit0(super::op_result::read_failed(e)),
                }
            }
            EditTarget::ViewId(id) => match self.view_text_for_edit(id) {
                Ok(v) => v,
                Err(e) => return edit0(e),
            },
            EditTarget::LastView => {
                let id = match self.views.last_view_id {
                    0 => ((self.op_count.saturating_sub(1)) % 999) + 1,
                    id => id,
                };
                match self.view_text_for_edit(id) {
                    Ok(v) => v,
                    Err(e) => return edit0(e),
                }
            }
            EditTarget::ContentRef(_) => return edit0("content-ref edit unsupported"),
        };

        if let Err(e) = self.revalidate_edit_preimage(root, &target_path, &source_text) {
            return edit0(e);
        }

        if let Some(window) = window {
            let line_count = source_text.lines().count();
            if window.start > line_count || window.end > line_count {
                return edit0(format!(
                    "stale preimage: target line window L{}-L{} exceeds current file line count {line_count}",
                    window.start, window.end
                ));
            }
        }
        let byte_window = window.map(|window| {
            let (start, end) = super::target_ref::window_byte_range(&source_text, window);
            (start as usize, end as usize)
        });
        let replace_source = byte_window
            .map(|(start, end)| &source_text[start..end])
            .unwrap_or(&source_text);
        let replaced = match apply_unique_replace(replace_source, &spec.old, &spec.new) {
            Ok(text) => text,
            Err("no match") => {
                return edit0(if window.is_some() {
                    "stale preimage: no match in target line window"
                } else {
                    "no match"
                });
            }
            Err("ambiguous match") => {
                return edit0(if window.is_some() {
                    "ambiguous match in target line window"
                } else {
                    "ambiguous match, use longer unique old or ref"
                });
            }
            Err(e) => return edit0(e),
        };
        let updated = if let Some((start, end)) = byte_window {
            let mut text =
                String::with_capacity(source_text.len() - (end - start) + replaced.len());
            text.push_str(&source_text[..start]);
            text.push_str(&replaced);
            text.push_str(&source_text[end..]);
            text
        } else {
            replaced
        };

        if let Some(err) = self.store_error_suffix("edit") {
            return err;
        }
        if let Err(e) = self.revalidate_edit_preimage(root, &target_path, &source_text) {
            return edit0(e);
        }
        let (pre_mtime_ns, pre_mode, pre_xattrs) = file_meta_snapshot(&target_path);
        if self.codemode_edit_plan && !self.pending_edit_intents.is_empty() {
            return edit0(
                "conflict: multiple crash-atomic edits in one CodeMode plan are unsupported",
            );
        }
        let rel = rel_path_for_log(root, &target_path);
        let root_id = root
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if self.recovery.has_edit_intent(&root_id, &rel) {
            return self.edit_outcome_error(
                super::MutationState::Indeterminate,
                "reopen",
                format!("unresolved edit intent blocks {rel}"),
            );
        }
        if self.codemode_edit_plan && self.recovery.exec_txn_active.get() {
            if let Err(error) = self.recovery.suspend_exec_txn() {
                return self.edit_outcome_error(
                    super::MutationState::MutationFree,
                    "intent_commit",
                    error,
                );
            }
        }
        if let Err(error) = edit_fault("intent_commit") {
            return self.edit_outcome_error(
                super::MutationState::MutationFree,
                "intent_commit",
                error,
            );
        }
        // References are filled only after publication, inside the evidence transaction.
        let intent_id = match self.recovery.create_edit_intent(
            &root_id,
            &rel,
            source_text.as_bytes(),
            updated.as_bytes(),
            "",
            "",
            pre_mtime_ns,
            pre_mode,
            &pre_xattrs,
        ) {
            Ok(id) => id,
            Err(error) => {
                return self.edit_outcome_error(
                    super::MutationState::MutationFree,
                    "intent_commit",
                    format!("edit intent failed: {error}"),
                );
            }
        };
        if self.codemode_edit_plan {
            self.pending_edit_intents.push(intent_id);
        }
        // Write-time TOCTOU guard (V6-F1 / ZS-SEC-001): the target resolved
        // canonical earlier, but a parent swapped for a symlink since then
        // must not redirect the write outside the root.
        if let Some(root_path) = root {
            if let Err(e) = guard_write_target_parent(root_path, &target_path) {
                return edit0(super::op_result::bad_path(e));
            }
        }
        if let Err(e) = crate::path::refuse_non_regular_file(&target_path) {
            let _ = self.recovery.clear_edit_intent(intent_id);
            return edit0(e);
        }
        // Re-read immediately before publish. Intent commit and parent-swap
        // checks sit after the last preimage revalidate; without this check
        // a second session can clobber the first publisher
        // (fszero-ai-filesystem-excellence-jqf.6.2).
        #[cfg(test)]
        TEST_EDIT_BETWEEN_READ_AND_WRITE.with(|hook| {
            if let Some(interfere) = hook.get() {
                interfere(&target_path);
            }
        });
        if let Err(e) = self.revalidate_edit_preimage(root, &target_path, &source_text) {
            let _ = self.recovery.clear_edit_intent(intent_id);
            return edit0(e);
        }
        let mut edit_txn = false;
        if let Err(error) = super::path::atomic_write_with_outcome(&target_path, updated.as_bytes())
        {
            if error.published {
                return self.finish_edit_rollback(
                    intent_id,
                    edit_txn,
                    &target_path,
                    rollback_edit(
                        &target_path,
                        source_text.as_bytes(),
                        pre_mtime_ns,
                        pre_mode,
                        &pre_xattrs,
                    ),
                    "publication",
                    format!("edit failed after publication; rolled back: {error}"),
                );
            }
            let _ = self.recovery.clear_edit_intent(intent_id);
            return self.edit_outcome_error(
                super::MutationState::MutationFree,
                error.stage,
                error.to_string(),
            );
        }
        maybe_crash_mutation_at("edit_after_publish");
        if let Err(error) = edit_fault("verify") {
            return self.finish_edit_rollback(
                intent_id,
                edit_txn,
                &target_path,
                rollback_edit(
                    &target_path,
                    source_text.as_bytes(),
                    pre_mtime_ns,
                    pre_mode,
                    &pre_xattrs,
                ),
                "verify",
                error,
            );
        }
        match fs::read(&target_path) {
            Ok(materialized) if materialized == updated.as_bytes() => {}
            result => {
                let verification = match result {
                    Ok(_) => "postimage bytes differ from expected".to_string(),
                    Err(error) => format!("postimage read failed: {error}"),
                };
                return self.finish_edit_rollback(
                    intent_id,
                    edit_txn,
                    &target_path,
                    rollback_edit(
                        &target_path,
                        source_text.as_bytes(),
                        pre_mtime_ns,
                        pre_mode,
                        &pre_xattrs,
                    ),
                    "verify",
                    format!("materialization verification failed; rolled back: {verification}"),
                );
            }
        }
        edit_txn = if self.recovery.is_durable() {
            self.recovery.begin_exec_txn()
        } else {
            false
        };
        if edit_txn {
            self.recovery.mark_exec_txn_full();
        }
        if let Err(error) = edit_fault("post_ref") {
            return self.finish_edit_rollback(
                intent_id,
                edit_txn,
                &target_path,
                rollback_edit(
                    &target_path,
                    source_text.as_bytes(),
                    pre_mtime_ns,
                    pre_mode,
                    &pre_xattrs,
                ),
                "post_ref",
                error,
            );
        }
        let (pre_ref, post_ref) = self.put_pre_post(source_text.as_bytes(), updated.as_bytes());
        if let Err(error) = self
            .recovery
            .set_edit_intent_refs(intent_id, &pre_ref, &post_ref)
        {
            return self.finish_edit_rollback(
                intent_id,
                edit_txn,
                &target_path,
                rollback_edit(
                    &target_path,
                    source_text.as_bytes(),
                    pre_mtime_ns,
                    pre_mode,
                    &pre_xattrs,
                ),
                "post_ref",
                error,
            );
        }
        self.recovery.put("edit-post", updated.as_bytes());
        if let Err(error) = edit_fault("certificate") {
            return self.finish_edit_rollback(
                intent_id,
                edit_txn,
                &target_path,
                rollback_edit(
                    &target_path,
                    source_text.as_bytes(),
                    pre_mtime_ns,
                    pre_mode,
                    &pre_xattrs,
                ),
                "certificate",
                error,
            );
        }
        let cert_ref = self.store_edit_cert_with_metadata(
            &target_path,
            &pre_ref,
            &post_ref,
            &spec.old,
            &spec.new,
            pre_mtime_ns,
            pre_mode,
            &pre_xattrs,
        );
        if let Some(error) = self.store_error_suffix("edit") {
            return self.finish_edit_rollback(
                intent_id,
                edit_txn,
                &target_path,
                rollback_edit(
                    &target_path,
                    source_text.as_bytes(),
                    pre_mtime_ns,
                    pre_mode,
                    &pre_xattrs,
                ),
                "certificate",
                error,
            );
        }
        self.refresh_path_after_mutation(&target_path);
        self.record_access("edit", &rel, &content_hash_bytes(updated.as_bytes()));
        if let Err(error) = edit_fault("history") {
            return self.finish_edit_rollback(
                intent_id,
                edit_txn,
                &target_path,
                rollback_edit(
                    &target_path,
                    source_text.as_bytes(),
                    pre_mtime_ns,
                    pre_mode,
                    &pre_xattrs,
                ),
                "history",
                error,
            );
        }
        let edit_seq = match self.record_mutation(
            "edit",
            &rel,
            &pre_ref,
            &post_ref,
            false,
            pre_mtime_ns,
            pre_mode,
            &pre_xattrs,
        ) {
            Ok(seq) => seq,
            Err(error) => {
                return self.finish_edit_rollback(
                    intent_id,
                    edit_txn,
                    &target_path,
                    rollback_edit(
                        &target_path,
                        source_text.as_bytes(),
                        pre_mtime_ns,
                        pre_mode,
                        &pre_xattrs,
                    ),
                    "history",
                    super::op_result::journal_err(error),
                );
            }
        };
        if let Err(error) = edit_fault("finalize") {
            return self.finish_edit_rollback(
                intent_id,
                edit_txn,
                &target_path,
                rollback_edit(
                    &target_path,
                    source_text.as_bytes(),
                    pre_mtime_ns,
                    pre_mode,
                    &pre_xattrs,
                ),
                "finalize",
                error,
            );
        }
        if !self.codemode_edit_plan {
            if let Err(error) = self.recovery.clear_edit_intent(intent_id) {
                return self.finish_edit_rollback(
                    intent_id,
                    edit_txn,
                    &target_path,
                    rollback_edit(
                        &target_path,
                        source_text.as_bytes(),
                        pre_mtime_ns,
                        pre_mode,
                        &pre_xattrs,
                    ),
                    "finalize",
                    format!("edit finalization failed: {error}"),
                );
            }
            self.recovery.commit_exec_txn(edit_txn);
            if let Some(error) = self.store_error_suffix("edit") {
                return self.finish_edit_rollback(
                    intent_id,
                    false,
                    &target_path,
                    rollback_edit(
                        &target_path,
                        source_text.as_bytes(),
                        pre_mtime_ns,
                        pre_mode,
                        &pre_xattrs,
                    ),
                    "finalize",
                    error,
                );
            }
        }
        // V6-F1 (ZS-STORE-004): seal the uniform effect record (one Write on
        // the edited path) and bind it into the op receipt.
        let effects = self.seal_effect_record(
            "edit",
            super::effect_capture::EffectScope::Session,
            vec![super::effect_capture::EffectPath {
                path: rel.clone(),
                action: super::effect_capture::EffectAction::Write,
                seq: edit_seq,
                pre_ref: pre_ref.clone(),
                post_ref: post_ref.clone(),
            }],
            vec![],
        );
        let mut detail = format!("edit:1 (pre:{} bytes cert:{})", source_text.len(), cert_ref);
        Self::append_effect_token(&mut detail, &effects);
        detail
    }

    /// Remember last resolved path for warm-path reuse (path cache only).
    fn remember_resolved_path(&mut self, arg: &str, arc: &Arc<PathBuf>) -> PathBuf {
        self.caches.last_path_arg = Some(arg.to_string());
        self.caches.last_path = Some(Arc::clone(arc));
        (**arc).clone()
    }

    pub fn resolve_existing_path_cached(
        &mut self,
        root: Option<&Path>,
        arg: &str,
    ) -> Result<PathBuf, String> {
        // Session root is sticky and caches clear on root change — key by arg
        // alone when rooted (avoids format! + root.display every warm op).
        let key = if root.is_some() {
            arg.to_string()
        } else {
            format!("\0{arg}")
        };
        if let Some(path) = self.caches.paths.get(&key) {
            let cached = Arc::clone(path);
            if let Some(root) = root {
                match revalidate_path_under_root_canon(
                    self.root_canon.as_deref(),
                    root,
                    cached.as_path(),
                ) {
                    Ok(validated) => {
                        // Prefer Arc share when revalidate returns the same path.
                        if validated.as_os_str() == cached.as_os_str() {
                            return Ok(self.remember_resolved_path(arg, &cached));
                        }
                        let arc = Arc::new(validated);
                        self.caches.paths.insert(key, Arc::clone(&arc));
                        return Ok(self.remember_resolved_path(arg, &arc));
                    }
                    Err(e) => {
                        self.caches.paths.remove(&key);
                        self.caches.content.remove(cached.as_path());
                        return Err(e);
                    }
                }
            } else if fs::metadata(cached.as_path()).is_ok() {
                return Ok(self.remember_resolved_path(arg, &cached));
            } else {
                self.caches.paths.remove(&key);
            }
        }
        let path = resolve_existing_path(root, arg)?;
        let arc = Arc::new(path);
        self.caches.paths.insert(key, Arc::clone(&arc));
        Ok(self.remember_resolved_path(arg, &arc))
    }

    fn do_expand(&mut self, _root: Option<&Path>, arg: Option<&str>) -> String {
        let key = arg.unwrap_or("");
        if key.is_empty() {
            return expand0("no ref");
        }
        // Portable blob fragments go through the strict ZeroRef v1 parser and
        // shared selector. The split below is only the legacy named-key line
        // window compatibility path.
        let portable_blob = ["fz://blob/", "gz://blob/", "tz://blob/"]
            .iter()
            .any(|prefix| key.starts_with(prefix));
        let (key, window) = if portable_blob {
            (key, None)
        } else {
            match key.rsplit_once("#L") {
                Some((base, spec)) if !base.is_empty() => match spec.split_once('-') {
                    Some((s, e)) => match (s.parse::<usize>(), e.parse::<usize>()) {
                        (Ok(start), Ok(end)) if start >= 1 && end >= start => {
                            (base, Some((start, end)))
                        }
                        _ => return expand0(format!("bad window: {spec}")),
                    },
                    None => return expand0(format!("bad window: {spec}")),
                },
                _ => (key, None),
            }
        };
        match self.resolve_ref_payload_detailed(key) {
            Ok(bytes) => {
                // Explicit expand ALWAYS returns exact bytes: keep the payload
                // reachable for the wire layer (recovery.put keys are seq-
                // numbered, so a bare "expand" lookup cannot recover it).
                let out = if let Some((start, end)) = window {
                    let fragment = super::zeroref::ZeroFragment::Lines {
                        start: start as u64,
                        end: end as u64,
                    };
                    match super::zeroref::select_fragment(bytes.as_ref(), &fragment, key) {
                        Ok(slice) => slice.to_vec(),
                        Err(error) => return expand0(error),
                    }
                } else {
                    bytes
                };
                let _ = self.recovery.put("expand", &out);
                // Exempt these bytes from the visible-wire novelty collapse; an
                // earlier read of the same content would otherwise reduce this
                // exact serve to a one-line preview (b4yg).
                if let Ok(text) = std::str::from_utf8(&out) {
                    self.note_exact_served_content(text);
                }
                self.views.last_expand_payload = Some(out);
                "X:ok".to_string()
            }
            Err(e) => expand0(e),
        }
    }
}
