//! Typed FSZero connector — thin CodeMode transport adapter (fszero-ncib.2).
//!
//! Canonical operations go through `dispatch_codemode_method` /
//! `dispatch_operation` only. This module owns:
//! - transaction journal hooks (pre/post mutation bookkeeping)
//! - CodeMode recipe helpers (`fs.compound` named forms, line windows)
//! - `FsStep` assembly from `DispatchOutcome`
//!
//! It must not reimplement domain validation, search retry, world-query
//! branching, or mutation semantics, and must not call FastMCP/MCP JSON-RPC.

use crate::core::dispatcher::InlineEvidence;
use crate::core::{DispatchOutcome, FSZeroSession, dispatch_codemode_method};
use serde_json::{Value, json};
use std::fs;

/// pn93: byte bound above which even a plan-produced file read keeps its
/// ref instead of inlining verbatim.
const PRODUCED_READ_MAX_BYTES: usize = 32 * 1024;
use std::path::{Path, PathBuf};

use super::transaction::world_arg_from_args;
use super::world_parse::world_id_from_kernel_message;

fn optional_line_arg(args: &Value, name: &str) -> Result<Option<usize>, String> {
    let Some(value) = args.get(name) else {
        return Ok(None);
    };
    let line = value
        .as_u64()
        .and_then(|line| usize::try_from(line).ok())
        .filter(|line| *line > 0)
        .ok_or_else(|| format!("{name} must be a positive integer"))?;
    Ok(Some(line))
}

/// First present spelling wins; agents reach for camelCase as often as
/// snake_case, so both are accepted at every line-window arg site (zerostack-eqyf).
fn optional_line_arg_aliased(args: &Value, names: &[&str]) -> Result<Option<usize>, String> {
    for name in names {
        if let Some(line) = optional_line_arg(args, name)? {
            return Ok(Some(line));
        }
    }
    Ok(None)
}

/// Parse `lines` selector: `"1-80"`, `"340-470"`, `"12"`, or a positive integer JSON number.
/// Returns (start, end) inclusive 1-based line numbers.
fn parse_lines_selector(args: &Value) -> Result<Option<(usize, usize)>, String> {
    let Some(value) = args.get("lines") else {
        return Ok(None);
    };
    if let Some(n) = value
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0)
    {
        return Ok(Some((n, n)));
    }
    let Some(s) = value.as_str() else {
        return Err(
            "lines must be a string range like \"1-80\" or \"340-470\", or a positive integer; got non-string/non-int (use start_line/end_line as alternative)".to_string(),
        );
    };
    let s = s.trim();
    if s.is_empty() {
        return Err("lines must not be empty; use \"1-N\" or omit for whole file".to_string());
    }
    if let Some((a, b)) = s.split_once('-') {
        let start = a.trim().parse::<usize>().map_err(|_| {
            format!("lines start must be a positive integer (got {a:?}); example: lines:\"1-80\"")
        })?;
        let end = b.trim().parse::<usize>().map_err(|_| {
            format!("lines end must be a positive integer (got {b:?}); example: lines:\"1-80\"")
        })?;
        if start == 0 || end == 0 {
            return Err(
                "lines range must be 1-based positive integers (e.g. \"1-80\")".to_string(),
            );
        }
        if end < start {
            return Err(format!(
                "lines range end ({end}) must not precede start ({start}); use ascending order like \"340-470\""
            ));
        }
        return Ok(Some((start, end)));
    }
    let n = s.parse::<usize>().map_err(|_| {
        format!(
            "lines must be \"N-M\" or a single line number (got {s:?}); example: lines:\"1-80\""
        )
    })?;
    if n == 0 {
        return Err("lines must be a 1-based positive integer".to_string());
    }
    Ok(Some((n, n)))
}

/// Parse a plain path or the canonical discovery target grammar.
fn canonical_line_target(path: &str) -> Result<(&str, Option<(usize, usize)>), String> {
    if let Some((bare, window)) = crate::core::target_ref::parse_target_ref(path) {
        return Ok((bare, Some((window.start, window.end))));
    }
    if path.contains('#') {
        return Err(format!(
            "invalid canonical target {path:?}; expected path#L<start>-L<end> with positive ascending bounds"
        ));
    }
    Ok((path, None))
}

fn line_window(payload: &[u8], start_line: usize, end_line: usize) -> Vec<u8> {
    let text = String::from_utf8_lossy(payload);
    crate::core::page_lines(&text, start_line, end_line).bytes
}

#[derive(Debug, Clone)]
pub struct FsStep {
    pub op: char,
    pub method: String,
    pub ack: String,
    pub ok: bool,
    pub recovery_key: String,
    pub detail: Option<String>,
    pub payload: Vec<u8>,
    pub evidence: Option<InlineEvidence>,
}

pub struct FsConnector<'a> {
    session: &'a mut FSZeroSession,
    journal: Option<&'a mut super::transaction::TransactionJournal>,
}

impl<'a> FsConnector<'a> {
    pub fn new(session: &'a mut FSZeroSession) -> Self {
        Self {
            session,
            journal: None,
        }
    }

    pub fn with_journal(
        session: &'a mut FSZeroSession,
        journal: &'a mut super::transaction::TransactionJournal,
    ) -> Self {
        Self {
            session,
            journal: Some(journal),
        }
    }

    /// Invoke a native `fs.*` method via the typed domain dispatcher.
    ///
    /// Argument validation, root policy, search retry, world-query branching,
    /// mutation effects, refs, and telemetry are domain-owned. This adapter
    /// only journals CodeMode transactions and assembles `FsStep`.
    pub fn invoke(&mut self, method: &str, args: &Value) -> FsStep {
        // Named compound recipes are CodeMode program sugar (not single ABI
        // ops). They expand into one or more canonical dispatches below.
        if method == "fs.compound" {
            if let Some(name) = args.get("name").and_then(Value::as_str) {
                if name == "verifiedEdit" {
                    return self.verified_edit(args);
                }
                return self.named_compound(name, args);
            }
        }

        if let Err(error) = self.journal_before(method, args) {
            return self.error_step(method, error);
        }
        match dispatch_codemode_method(self.session, method, args) {
            Ok(outcome) => match self.journal_after(method, args, &outcome) {
                Ok(()) => self.finalize_outcome(method, outcome),
                Err(error) => self.error_step(method, error),
            },
            Err(e) => self.error_step(method, e.message),
        }
    }

    /// Pre-mutation journal capture — transport-only bookkeeping, not a
    /// second engine path.
    fn journal_before(&mut self, method: &str, args: &Value) -> Result<(), String> {
        let Some(journal) = self.journal.as_deref_mut() else {
            return Ok(());
        };
        match method {
            "fs.write" => {
                if let Some(path) = args.get("path").and_then(Value::as_str) {
                    journal.before_write(self.session, path)?;
                }
            }
            "fs.edit" => {
                if let Some(spec) = args.get("spec").and_then(Value::as_str) {
                    journal.before_edit(self.session, spec)?;
                } else if let Some(path) = args.get("path").and_then(Value::as_str) {
                    journal.before_edit_target(self.session, path)?;
                }
            }
            "fs.undo" => {
                if let Some(path) = args
                    .get("arg")
                    .or_else(|| args.get("path"))
                    .and_then(Value::as_str)
                {
                    journal.before_undo(self.session, path)?;
                }
            }
            "fs.memory" | "fs.memory.put" | "fs.memory.delete" | "fs.memory.rename" => {
                journal.before_memory(self.session, method, args)?
            }
            "fs.world" => {
                if let Some(arg) = world_arg_from_args(args)? {
                    journal.before_world(self.session, &arg)?;
                }
            }
            "fs.compound" => {
                if let Some(intent) = args.get("intent").and_then(Value::as_str) {
                    journal.before_compound_intent(self.session, intent)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn journal_after(
        &mut self,
        method: &str,
        args: &Value,
        outcome: &DispatchOutcome,
    ) -> Result<(), String> {
        if !outcome.result.ok {
            return Ok(());
        }
        let Some(journal) = self.journal.as_deref_mut() else {
            return Ok(());
        };
        if method == "fs.edit" {
            journal.after_certified_edit(self.session)?;
        }
        if method == "fs.world"
            && let Ok(Some(spec)) = world_arg_from_args(args)
        {
            if let Some(wid) = spec.strip_prefix("commit:") {
                journal.record_world_committed(wid.strip_suffix(":git").unwrap_or(wid));
            } else if crate::core::world_arg_creates(&spec) {
                if let Some(d) = outcome.detail.as_deref() {
                    if let Some(wid) = world_id_from_kernel_message(d) {
                        journal.record_world_created(&wid);
                    }
                }
            }
        }
        Ok(())
    }

    fn missing_arg(&mut self, method: &str, field: &str) -> FsStep {
        self.error_step(method, format!("missing required arg: {field}"))
    }

    fn error_step(&mut self, method: &str, detail: String) -> FsStep {
        self.session
            .recovery
            .put_key(super::runtime::ERROR_REF, detail.as_bytes());
        self.session
            .record_codemode_materialization(detail.as_bytes());
        FsStep {
            op: 'X',
            method: method.to_string(),
            ack: "X0".to_string(),
            ok: false,
            recovery_key: super::runtime::ERROR_REF.to_string(),
            detail: Some(detail.clone()),
            payload: detail.into_bytes(),
            evidence: None,
        }
    }

    fn finalize_step(
        &mut self,
        op: char,
        method: &str,
        ack: String,
        ok: bool,
        recovery_key: &str,
        detail: Option<String>,
        evidence: Option<InlineEvidence>,
    ) -> FsStep {
        let recovered = self.session.expand(recovery_key);
        if recovered.is_none() {
            self.session.record_codemode_measurement_miss();
        }
        let payload = recovered
            .or_else(|| detail.as_ref().map(|d| d.as_bytes().to_vec()))
            .unwrap_or_default();
        self.session.record_codemode_materialization(&payload);
        FsStep {
            op,
            method: method.to_string(),
            ack,
            ok,
            recovery_key: recovery_key.to_string(),
            detail,
            payload,
            evidence,
        }
    }

    fn finalize_outcome(&mut self, method: &str, outcome: DispatchOutcome) -> FsStep {
        let op = outcome.opcode.unwrap_or('X');
        let ack = outcome.result.ack.clone().unwrap_or_else(|| {
            if outcome.result.ok {
                "ok".into()
            } else {
                "X0".into()
            }
        });
        let key = outcome
            .recovery_key
            .as_deref()
            .unwrap_or(super::runtime::ERROR_REF);
        // Park the human-readable failure under ERROR_REF so finish()/CLI can
        // expand it after the plan rolls back (fszero-quer). Kernel failures
        // previously left only step.detail in memory while recovery_key pointed
        // at an empty opcode key (e.g. "read").
        if !outcome.result.ok {
            if let Some(detail) = outcome.detail.as_ref().filter(|d| !d.is_empty()) {
                self.session
                    .recovery
                    .put_key(super::runtime::ERROR_REF, detail.as_bytes());
            }
        }
        self.finalize_step(
            op,
            method,
            ack,
            outcome.result.ok,
            key,
            outcome.detail,
            outcome.inline_evidence,
        )
    }

    /// Required string field → invoke; missing → typed missing_arg.
    fn invoke_required(&mut self, method: &str, field: &str, value: Option<&str>) -> FsStep {
        match value {
            Some(v) => self.invoke(method, &json!({ field: v })),
            None => self.missing_arg(method, field),
        }
    }

    /// Optional string field on an otherwise empty args object.
    fn invoke_optional(&mut self, method: &str, field: &str, value: Option<&str>) -> FsStep {
        let mut args = json!({});
        if let Some(a) = value {
            args[field] = json!(a);
        }
        self.invoke(method, &args)
    }

    /// Convenience wrappers — pure invoke shims (no adapter-local semantics).
    pub fn ls(&mut self, arg: Option<&str>) -> FsStep {
        self.invoke_optional("fs.ls", "arg", arg)
    }

    pub fn read(&mut self, arg: Option<&str>) -> FsStep {
        self.invoke_required("fs.read", "path", arg)
    }

    pub fn search(&mut self, arg: Option<&str>) -> FsStep {
        self.invoke_required("fs.search", "query", arg)
    }

    pub fn edit(&mut self, arg: Option<&str>) -> FsStep {
        self.invoke_required("fs.edit", "spec", arg)
    }

    /// Object-form edit: same domain path as invoke("fs.edit", {path,find,replace}).
    pub fn edit_parts(&mut self, path: &str, find: &str, replace: &str) -> FsStep {
        self.edit_parts_in_window(path, find, replace, None)
    }

    /// Object-form edit constrained to one canonical 1-based line window.
    fn edit_parts_in_window(
        &mut self,
        path: &str,
        find: &str,
        replace: &str,
        window: Option<(usize, usize)>,
    ) -> FsStep {
        let mut args = json!({"path": path, "find": find, "replace": replace});
        if let Some((start, end)) = window {
            args["start_line"] = json!(start);
            args["end_line"] = json!(end);
        }
        let step = self.invoke("fs.edit", &args);
        // pn93: a successful mutation makes the path plan-produced, so a
        // later full-file read inlines instead of forcing a re-fetch.
        if step.ok {
            self.session.note_produced_path(path);
        }
        step
    }

    pub fn write(&mut self, path: &str, content: &str) -> FsStep {
        let step = self.invoke("fs.write", &json!({"path": path, "content": content}));
        // pn93: the plan produced these bytes; a terminal read of the path
        // must come back inline, not hidden behind a ref.
        if step.ok {
            self.session.note_produced_path(path);
        }
        step
    }

    pub fn compound(&mut self, intent: &str) -> FsStep {
        self.invoke("fs.compound", &json!({"intent": intent}))
    }

    /// Compound mutate apply/status contract. Exact postimage verification is
    /// kernel-owned; a successful fs.edit already proves materialized bytes.
    fn mutate_checked(
        &mut self,
        path: &str,
        old: &str,
        new: &str,
        window: Option<(usize, usize)>,
    ) -> FsStep {
        let mut step = self.edit_parts_in_window(path, old, new, window);
        let status = if step.ok { "applied" } else { "rejected" };
        let contract = format!("apply={} dry_run=false status={status}", step.ok);
        step.detail = Some(match step.detail.take() {
            Some(detail) => format!("{contract}; {detail}"),
            None => contract,
        });
        step
    }

    /// Planner entry with terminal-action validation (call-issues-0729 item 3).
    /// A mutation goal must reach a mutating action; listing is not success.
    fn compound_plan(&mut self, args: &Value) -> FsStep {
        let goal = args
            .get("goal")
            .or_else(|| args.get("intent"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if is_mutation_goal(goal) {
            let path = args.get("path").and_then(Value::as_str);
            let old = args.get("old").and_then(Value::as_str);
            let new = args.get("new").and_then(Value::as_str);
            if let (Some(path), Some(old), Some(new)) = (path, old, new) {
                return self.mutate_checked(path, old, new, None);
            }
            return self.error_step(
                "fs.compound",
                format!(
                    "plan goal {goal:?} requests a mutation but the plan has no terminal mutating action; pass zero.fs.plan(goal,{{path,old,new}}) or call zero.fs.compound('mutate',{{path,old,new}}) — a listing/read-only plan is not success for a mutation goal"
                ),
            );
        }
        match args.get("path").and_then(Value::as_str) {
            Some(path) => self.named_compound("inventory", &json!({"path": path})),
            None => self.compound(goal),
        }
    }

    fn named_compound(&mut self, name: &str, args: &Value) -> FsStep {
        match name {
            "read" => self.compound_read(args),
            "inventory" | "list" if args.get("paths").is_some() => {
                self.invoke("fs.multiList", args)
            }
            "inventory" | "list" => {
                // Build ls arg string so depth/budget reach parse_ls_spec (fszero-n1qc).
                let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
                if Path::new(path).is_absolute() {
                    let root = self.session.root.as_deref();
                    let root_display = root
                        .map_or_else(|| "(none)".to_string(), |root| root.display().to_string());
                    let corrected = root
                        .and_then(|root| Path::new(path).strip_prefix(root).ok())
                        .filter(|relative| !relative.as_os_str().is_empty())
                        .map_or_else(
                            || ".".to_string(),
                            |relative| relative.display().to_string(),
                        );
                    return self.error_step(
                        "fs.compound",
                        format!(
                            "list path must be workspace-relative to active root {root_display:?}; received absolute path {path:?}; use path:{corrected:?}"
                        ),
                    );
                }
                let path = if crate::core::path::is_session_root_arg(path) {
                    "."
                } else {
                    path
                };
                let pattern = args.get("pattern").and_then(Value::as_str);
                let depth = args.get("depth").and_then(Value::as_u64).or_else(|| {
                    args.get("depth")
                        .and_then(Value::as_str)
                        .and_then(|s| s.parse().ok())
                });
                let budget = args.get("budget").and_then(Value::as_u64).or_else(|| {
                    args.get("budget")
                        .and_then(Value::as_str)
                        .and_then(|s| s.parse().ok())
                });
                let mut parts = Vec::new();
                if let Some(d) = depth {
                    parts.push(format!("--depth={d}"));
                }
                if let Some(b) = budget.filter(|b| *b > 0) {
                    parts.push(format!("--budget={b}"));
                }
                let path_or_glob = pattern
                    .map(|pattern| {
                        if path == "." {
                            pattern.to_string()
                        } else {
                            Path::new(path).join(pattern).to_string_lossy().into_owned()
                        }
                    })
                    .unwrap_or_else(|| path.to_string());
                parts.push(path_or_glob);
                let arg = parts.join(" ");
                self.ls(Some(&arg))
            }
            "write" => {
                let Some(path) = args.get("path").and_then(Value::as_str) else {
                    return self.missing_arg("fs.compound", "path");
                };
                let content = args.get("content").and_then(Value::as_str).unwrap_or("");
                let path = match workspace_relative_path(self.session.root.as_deref(), path) {
                    Ok(path) => path,
                    Err(detail) => return self.error_step("fs.compound", detail),
                };
                self.write(&path, content)
            }
            "mutate" | "edit" => {
                // Reject unsupported delete semantics with a corrective shape (fszero-oppj).
                if args.get("delete").is_some()
                    || args.get("op").and_then(Value::as_str) == Some("delete")
                    || args.get("action").and_then(Value::as_str) == Some("delete")
                {
                    return self.error_step(
                        "fs.compound",
                        "compound mutate/edit does not support delete; use zero.fs.compound('mutate',{path,old,new}) or zero.fs.write({path,content}) to replace file contents. There is no compound delete API — remove via write-empty only if intentional.".to_string(),
                    );
                }
                if let Some(spec) = args.get("spec").and_then(Value::as_str) {
                    return self.edit(Some(spec));
                }
                // Fused snap mutation is one canonical fs.edit dispatch. Search
                // selection and the journaled edit both stay inside the kernel.
                if args.get("query").is_some() {
                    return self.invoke("fs.edit", args);
                }
                let Some(target) = args.get("path").and_then(Value::as_str) else {
                    return self.missing_arg("fs.compound", "path, query, or spec");
                };
                let Some(old) = args.get("old").and_then(Value::as_str) else {
                    return self
                        .missing_arg("fs.compound", "old or spec (mutate needs path+old+new)");
                };
                let Some(new) = args.get("new").and_then(Value::as_str) else {
                    return self
                        .missing_arg("fs.compound", "new or spec (mutate needs path+old+new)");
                };
                // Accept canonical discovery targets verbatim. The kernel receives
                // the bare path plus the exact line window, so uniqueness and
                // replacement are evaluated only inside that target.
                let (path, window) = match canonical_line_target(target) {
                    Ok(parts) => parts,
                    Err(detail) => return self.error_step("fs.compound", detail),
                };
                // Structured arg passing: never marshal the object form
                // through the `path:old|new` grammar, so `|` and `:` in
                // find/replace are preserved byte-exact (fszero-edit-spec-pipe-escape-beh).
                let path = match workspace_relative_path(self.session.root.as_deref(), path) {
                    Ok(path) => path,
                    Err(detail) => return self.error_step("fs.compound", detail),
                };
                self.mutate_checked(&path, old, new, window)
            }
            "plan" => self.compound_plan(args),
            "resolve" => self.resolve(args.get("intent").and_then(Value::as_str)),
            "history" => self.history(
                args.get("path")
                    .or_else(|| args.get("arg"))
                    .and_then(Value::as_str),
            ),
            "undo" => self.undo(
                args.get("path")
                    .or_else(|| args.get("arg"))
                    .and_then(Value::as_str),
            ),
            "world" => match world_arg_from_args(args) {
                Ok(Some(arg)) => self.world(Some(&arg)),
                Ok(None) => self.missing_arg("fs.compound", "arg or action"),
                Err(error) => self.error_step("fs.compound", error),
            },
            "memory" => {
                let path = args.get("path").and_then(Value::as_str).unwrap_or("");
                let content = args.get("content").and_then(Value::as_str).unwrap_or("");
                let op = args.get("op").and_then(Value::as_str).unwrap_or("get");
                match op {
                    "put" => self.memory_put(path, content),
                    "ls" => {
                        self.memory_ls(args.get("prefix").and_then(Value::as_str).unwrap_or(path))
                    }
                    _ => self.memory_get(path),
                }
            }
            other => {
                let candidates = closest_valid_names(other, VALID_COMPOUND_NAMES);
                let best = candidates.first().copied().unwrap_or("read");
                self.error_step(
                    "fs.compound",
                    format!("unknown compound '{other}'; closest valid names: {}; try zero_describe('{best}')", candidates.join(", ")),)
            }
        }
    }

    fn compound_read(&mut self, args: &Value) -> FsStep {
        if args.get("paths").is_some() {
            if args.get("path").is_some() {
                return self.error_step(
                    "fs.compound",
                    "pass path or paths, not both".to_string(),
                );
            }
            return self.invoke("fs.multiRead", args);
        }
        let Some(path) = args.get("path").and_then(Value::as_str) else {
            return self.missing_arg("fs.compound", "path");
        };
        // Canonical `#L<start>-L<end>` discovery target selects a window
        // (fszero-codemode-read-no-line-window-7y71). Without this the fragment
        // reached path resolution and surfaced as a bogus "not found".
        let (path, from_fragment) = match canonical_line_target(path) {
            Ok(parts) => parts,
            Err(d) => return self.error_step("fs.compound", d),
        };
        if from_fragment.is_some() && args.get("lines").is_some() {
            return self.error_step(
                "fs.compound",
                "canonical target #L<start>-L<end> conflicts with the lines argument; pass exactly one".to_string(),
            );
        }
        // Prefer explicit `lines` ("1-80") when present; else start_line/end_line (fszero-r70i).
        let from_lines = match parse_lines_selector(args) {
            Ok(v) => v,
            Err(d) => return self.error_step("fs.compound", d),
        };
        let start_line = match optional_line_arg_aliased(args, &["start_line", "startLine"]) {
            Ok(v) => v,
            Err(d) => return self.error_step("fs.compound", d),
        };
        let end_line = match optional_line_arg_aliased(args, &["end_line", "endLine"]) {
            Ok(v) => v,
            Err(d) => return self.error_step("fs.compound", d),
        };
        let (start, end, windowed) = if let Some((s, e)) = from_lines.or(from_fragment) {
            (s, e, true)
        } else {
            let s = start_line.unwrap_or(1);
            let e = end_line.unwrap_or(usize::MAX);
            let windowed = start_line.is_some() || end_line.is_some();
            (s, e, windowed)
        };
        if end < start {
            return self.error_step(
                "fs.compound",
                format!("line window end ({end}) must not precede start ({start}); use ascending lines:\"N-M\" or start_line/end_line"),
            );
        }

        match classify_read_path(self.session.root.as_deref(), path) {
            Ok(ReadPathKind::File) => {
                let mut step = self.read(Some(path));
                // pn93: a full read of a file this plan already wrote or
                // mutated returns content the session produced; mark it exact
                // so the visible-wire novelty pass inlines it verbatim
                // (bounded: produced reads larger than 32KB keep the ref).
                if step.ok
                    && !windowed
                    && step.payload.len() <= PRODUCED_READ_MAX_BYTES
                    && self.session.is_produced_path(path)
                {
                    self.session
                        .note_exact_served_content(&String::from_utf8_lossy(&step.payload));
                }
                if step.ok && windowed {
                    // Window first, then mint recovery ref on the *windowed* bytes
                    // (never store full-file then return a windowed payload under that ref).
                    // cd0v: range/next_offset/remaining/total_lines in detail so the
                    // agent can resume without guessing the next start_line.
                    let text = String::from_utf8_lossy(&step.payload);
                    let page = crate::core::page_lines(&text, start, end);
                    // Keep line_window as the byte selector (delegates to page_lines).
                    step.payload = line_window(text.as_bytes(), start, end);
                    step.recovery_key = self.session.recovery.put_content_ref(&step.payload);
                    let resume = match page.next_offset {
                        Some(n) => format!(
                            " next_offset={n} remaining={} total_lines={}",
                            page.remaining, page.total
                        ),
                        None => format!(" remaining=0 total_lines={}", page.total),
                    };
                    step.detail = Some(format!(
                        "read:{} bytes L{}-{} range=[{},{}]{}",
                        step.payload.len(),
                        page.start,
                        page.end,
                        page.start,
                        page.end,
                        resume
                    ));
                }
                step
            }
            Ok(ReadPathKind::Directory) => self.error_step(
                "fs.compound",
                "path is a directory; use zero.fs.compound('inventory',{path})".to_string(),
            ),
            Err(detail) => self.error_step("fs.compound", detail),
        }
    }

    pub fn stat(&mut self, arg: Option<&str>) -> FsStep {
        self.invoke_required("fs.stat", "path", arg)
    }

    pub fn expand(&mut self, arg: Option<&str>) -> FsStep {
        self.invoke_required("fs.expand", "ref", arg)
    }

    pub fn world(&mut self, arg: Option<&str>) -> FsStep {
        match arg {
            Some(a) => self.invoke("fs.world", &json!({"arg": a})),
            None => self.missing_arg("fs.world", "arg or query"),
        }
    }

    pub fn memory_put(&mut self, path: &str, content: &str) -> FsStep {
        self.invoke("fs.memory.put", &json!({"path": path, "content": content}))
    }

    pub fn memory_get(&mut self, path: &str) -> FsStep {
        self.invoke_required("fs.memory.get", "path", Some(path))
    }

    pub fn memory_ls(&mut self, prefix: &str) -> FsStep {
        self.invoke_required("fs.memory.ls", "prefix", Some(prefix))
    }

    pub fn memory_delete(&mut self, path: &str) -> FsStep {
        self.invoke_required("fs.memory.delete", "path", Some(path))
    }

    pub fn memory_rename(&mut self, from: &str, to: &str) -> FsStep {
        self.invoke("fs.memory.rename", &json!({"from": from, "to": to}))
    }

    pub fn history(&mut self, arg: Option<&str>) -> FsStep {
        self.invoke_optional("fs.history", "path", arg)
    }

    pub fn undo(&mut self, arg: Option<&str>) -> FsStep {
        match arg {
            Some(a) => self.invoke_required("fs.undo", "path", Some(a)),
            None => self.missing_arg("fs.undo", "arg or path"),
        }
    }

    pub fn world_access_query(&mut self, args: &Value) -> FsStep {
        self.invoke("fs.world", args)
    }

    pub fn verified_edit(&mut self, args: &Value) -> FsStep {
        if let Some(journal) = self.journal.as_deref_mut()
            && let Err(error) = journal.before_verified_edit(self.session, args)
        {
            return self.error_step("fs.compound", error);
        }
        // verifiedEdit is a CodeMode compound recipe; kernel path is shared.
        let detail = self.session.do_verified_edit(args);
        let ok = detail.starts_with("verifiedEdit:1");
        let key = if ok {
            "verifiedEdit/ok"
        } else {
            "verifiedEdit/err"
        };
        let ack = if ok {
            "E1".to_string()
        } else {
            "X0".to_string()
        };
        self.finalize_step('E', "fs.compound", ack, ok, key, Some(detail), None)
    }

    pub fn resolve(&mut self, arg: Option<&str>) -> FsStep {
        self.invoke_required("fs.resolve", "intent", arg)
    }

    pub fn session_mut(&mut self) -> &mut FSZeroSession {
        self.session
    }

    pub fn file_for_symbol(&self, sym: &str) -> Option<String> {
        super::recipes::file_for_symbol(self.session, sym)
    }
}

/// Goal verbs that require a terminal mutating action (call-issues-0729 item 3).
const MUTATION_GOAL_VERBS: &[&str] = &[
    "mutate", "edit", "replace", "rewrite", "write", "patch", "update", "fix", "insert", "remove",
    "delete", "rename", "refactor", "apply",
];

/// Accept absolute paths that resolve inside the workspace root by rewriting
/// them workspace-relative; reject anything that escapes the jail (fszero-xs0x).
fn workspace_relative_path(root: Option<&Path>, path: &str) -> Result<String, String> {
    let candidate = Path::new(path);
    if !candidate.is_absolute() {
        return Ok(path.to_string());
    }
    let Some(root) = root else {
        return Err(format!(
            "path must be workspace-relative; received absolute path {path:?} with no active root"
        ));
    };
    let canon_root = fs::canonicalize(root).ok();
    let canon_candidate = fs::canonicalize(candidate).ok().or_else(|| {
        let parent = fs::canonicalize(candidate.parent()?).ok()?;
        Some(parent.join(candidate.file_name()?))
    });
    let mut bases: Vec<&Path> = vec![root];
    if let Some(canon) = canon_root.as_deref() {
        bases.push(canon);
    }
    let mut targets: Vec<&Path> = vec![candidate];
    if let Some(canon) = canon_candidate.as_deref() {
        targets.push(canon);
    }
    for target in &targets {
        for base in &bases {
            let Ok(relative) = target.strip_prefix(base) else {
                continue;
            };
            if relative.as_os_str().is_empty() {
                continue;
            }
            if relative
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                continue;
            }
            return Ok(relative.to_string_lossy().into_owned());
        }
    }
    Err(format!(
        "path must be workspace-relative to active root {:?}; received absolute path {path:?} that does not resolve inside the root; pass a root-relative path instead",
        root.display()
    ))
}

fn is_mutation_goal(goal: &str) -> bool {
    let lowered = goal.to_ascii_lowercase();
    MUTATION_GOAL_VERBS.iter().any(|verb| {
        lowered
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word == *verb)
    })
}

const VALID_COMPOUND_NAMES: &[&str] = &[
    "read",
    "inventory",
    "list",
    "plan",
    "write",
    "mutate",
    "verifiedEdit",
    "resolve",
    "history",
    "undo",
    "world",
    "memory",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadPathKind {
    File,
    Directory,
}

fn classify_read_path(root: Option<&Path>, arg: &str) -> Result<ReadPathKind, String> {
    let target = match crate::core::path::resolve_existing_path(root, arg) {
        Ok(target) => target,
        Err(detail) if detail.starts_with("ambiguous unicode path") => return Err(detail),
        Err(_) => return Err(missing_path_message(root, arg)),
    };
    let meta = fs::metadata(&target).map_err(|_| missing_path_message(root, arg))?;
    if meta.is_dir() {
        Ok(ReadPathKind::Directory)
    } else {
        Ok(ReadPathKind::File)
    }
}

fn missing_path_message(root: Option<&Path>, arg: &str) -> String {
    let suggestions = nearest_path_suggestions(root, arg);
    if suggestions.is_empty() {
        format!("path not found: {arg}")
    } else {
        format!("path not found: {arg}; nearest: {}", suggestions.join(", "))
    }
}

fn nearest_path_suggestions(root: Option<&Path>, arg: &str) -> Vec<String> {
    let Ok(rel) = crate::core::path::sanitize_relative_arg(arg) else {
        return Vec::new();
    };
    let base = root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut parent_rel = rel.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
    let mut needle = rel
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut dir = base.join(&parent_rel);
    if !dir.is_dir() {
        let mut components = rel.components();
        if let Some(std::path::Component::Normal(first)) = components.next() {
            needle = first.to_string_lossy().into_owned();
            parent_rel = PathBuf::new();
            dir = base.clone();
        }
    }
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut ranked = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel_name = if parent_rel.as_os_str().is_empty() {
            name.clone()
        } else {
            parent_rel.join(&name).to_string_lossy().replace('\\', "/")
        };
        let score = super::name_rank::name_score(&needle, &name);
        ranked.push((score, rel_name));
    }
    super::name_rank::take_top_ranked(ranked, 3)
}

fn closest_valid_names<'a>(needle: &str, candidates: &'a [&'a str]) -> Vec<&'a str> {
    let ranked = candidates
        .iter()
        .map(|candidate| (super::name_rank::name_score(needle, candidate), *candidate))
        .collect::<Vec<_>>();
    super::name_rank::take_top_ranked(ranked, 3)
}
