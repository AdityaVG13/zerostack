//! Read-only kernel payloads for parallel CodeMode workers.
//!
//! # ParallelReadContext protocol (fszero-tucs / R-003)
//!
//! - Workers never touch `RecoveryStore` (no raw pointers, no shared Cells).
//! - `ParallelReadContext` holds only an owned `PathBuf` root — auto `Send`+`Sync`.
//! - `fs.search` (AST/structural routes need the store) runs on the **main**
//!   session thread via [`execute_search_branch`], and may overlap worker
//!   ls/read/stat inside the same `thread::scope`.
//! - Parent may mutate `session.recovery` while workers run; workers only hit
//!   the filesystem through the frozen root path.
//! - Sole product consumer: `execute_readonly_parallel` in `codemode/parallel.rs`
//!   (joins all scoped workers before returning).

use super::list_ops;
use super::op_result::classify_op_result;
use super::read_ops;
use super::search::{
    build_search_payload, classify_search_query, files_budget_message, search_hit_count,
};
use super::{FSZeroSession, resolve_existing_path};
use std::path::PathBuf;

/// Methods with a real parallel dispatch path (worker arms or main-thread search).
///
/// `is_parallel_branch_safe` must stay a subset of this set (plus gated `fs.world`).
pub const PARALLEL_IMPLEMENTED_METHODS: &[&str] = &["fs.ls", "fs.read", "fs.search", "fs.stat"];

/// Frozen session slice for parallel read-only branches.
///
/// Auto `Send` + `Sync` via ordinary owned fields only — no raw pointers, no
/// `unsafe impl`. Search that needs `RecoveryStore` stays on the main thread.
#[derive(Clone)]
pub struct ParallelReadContext {
    pub root: PathBuf,
}

impl ParallelReadContext {
    pub fn capture(session: &FSZeroSession) -> Option<Self> {
        Some(Self {
            root: session.root.clone()?,
        })
    }

    pub fn ls_manifest(&self, arg: Option<&str>) -> Result<String, String> {
        list_ops::collect_ls_manifest(&self.root, arg)
    }

    pub fn read_bytes(&self, path_arg: &str) -> Result<Vec<u8>, String> {
        let (path, byte_range) = read_ops::parse_read_arg(path_arg)?;
        let full_path = resolve_existing_path(Some(&self.root), path)?;
        if let Some(range) = byte_range {
            return read_ops::read_range_bytes(&full_path, range);
        }
        read_ops::read_stable_file_bytes(&full_path).map(|(bytes, _)| bytes)
    }

    pub fn stat_payload(&self, path_arg: Option<&str>) -> Result<String, String> {
        let path_arg = path_arg.unwrap_or(".");
        let full_path = resolve_existing_path(Some(&self.root), path_arg)?;
        let meta =
            std::fs::symlink_metadata(&full_path).map_err(super::op_result::metadata_failed)?;
        Ok(list_ops::format_stat_manifest(&full_path, &meta))
    }
}

/// Main-thread `fs.search` for a parallel group branch (needs `RecoveryStore`).
pub fn execute_search_branch(
    session: &FSZeroSession,
    group_index: usize,
    branch_id: &str,
    args: &serde_json::Value,
) -> ParallelBranchWork {
    let Some(query) = args
        .as_object()
        .and_then(|m| m.get("query"))
        .and_then(serde_json::Value::as_str)
    else {
        return branch_error(branch_id, "fs.search", "missing required arg: query");
    };
    let q = query;
    let root = session.root.as_deref();
    let indexed = session.index.indexed_file_keys.len();
    let payload = if let Some(msg) = files_budget_message(indexed) {
        msg
    } else {
        let route = classify_search_query(Some(q), q);
        build_search_payload(route, root, &session.index, &session.recovery, q, None)
    };
    let (op, kernel_message, kind) = if payload.starts_with("budget:0") {
        ('S', payload.clone(), "search")
    } else {
        let hits = search_hit_count(&payload);
        ('S', format!("search:{hits} hits"), "search")
    };
    let ok = classify_op_result(&kernel_message).ok;
    let payload_key = branch_recovery_key(group_index, branch_id, kind);
    ParallelBranchWork {
        branch_id: branch_id.to_string(),
        method: "fs.search".to_string(),
        op,
        kernel_message,
        ok,
        payload_key,
        payload: payload.into_bytes(),
    }
}

#[derive(Clone)]
pub struct ParallelBranchWork {
    pub branch_id: String,
    pub method: String,
    pub op: char,
    pub kernel_message: String,
    pub ok: bool,
    pub payload_key: String,
    pub payload: Vec<u8>,
}

pub fn branch_recovery_key(group_index: usize, branch_id: &str, kind: &str) -> String {
    format!("codemode/p/{group_index}/{branch_id}/{kind}")
}

pub fn execute_parallel_branch(
    ctx: &ParallelReadContext,
    group_index: usize,
    branch_id: &str,
    method: &str,
    args: &serde_json::Value,
) -> ParallelBranchWork {
    let map = args.as_object();
    let (op, kernel_message, kind, payload) = match method {
        "fs.ls" => {
            let arg = map
                .and_then(|m| m.get("arg"))
                .and_then(serde_json::Value::as_str);
            match ctx.ls_manifest(arg) {
                Ok(body) => (
                    'L',
                    format!("ls:{} entries", body.lines().count()),
                    "ls",
                    body.into_bytes(),
                ),
                Err(e) => ('L', format!("bad ls: {e}"), "error", e.into_bytes()),
            }
        }
        "fs.read" => {
            let Some(path) = map
                .and_then(|m| m.get("path"))
                .and_then(serde_json::Value::as_str)
            else {
                return branch_error(branch_id, method, "missing required arg: path");
            };
            match ctx.read_bytes(path) {
                Ok(bytes) => ('R', format!("read:{} bytes", bytes.len()), "read", bytes),
                Err(e) => (
                    'R',
                    super::op_result::op0("read", &e),
                    "error",
                    e.into_bytes(),
                ),
            }
        }
        "fs.search" => {
            // Search needs RecoveryStore; workers must not call this path.
            return branch_error(
                branch_id,
                method,
                "fs.search must run on the main session thread",
            );
        }
        "fs.stat" => {
            let path = map
                .and_then(|m| m.get("path"))
                .and_then(serde_json::Value::as_str);
            match ctx.stat_payload(path) {
                Ok(body) => (
                    'T',
                    format!("stat:{} bytes", body.len()),
                    "stat",
                    body.into_bytes(),
                ),
                Err(e) => (
                    'T',
                    super::op_result::op0("stat", super::op_result::bad_path(&e)),
                    "error",
                    e.into_bytes(),
                ),
            }
        }
        _ => (
            'X',
            format!("not parallel-safe: {method}"),
            "error",
            format!("not parallel-safe: {method}").into_bytes(),
        ),
    };

    let ok = classify_op_result(&kernel_message).ok;
    let payload_key = branch_recovery_key(group_index, branch_id, kind);
    ParallelBranchWork {
        branch_id: branch_id.to_string(),
        method: method.to_string(),
        op,
        kernel_message,
        ok,
        payload_key,
        payload,
    }
}

fn branch_error(branch_id: &str, method: &str, message: &str) -> ParallelBranchWork {
    ParallelBranchWork {
        branch_id: branch_id.to_string(),
        method: method.to_string(),
        op: 'X',
        kernel_message: message.to_string(),
        ok: false,
        payload_key: "codemode/error".to_string(),
        payload: message.as_bytes().to_vec(),
    }
}

pub fn is_parallel_branch_safe(method: &str, args: &serde_json::Value) -> bool {
    if PARALLEL_IMPLEMENTED_METHODS.contains(&method) {
        return true;
    }
    if method == "fs.world" {
        if let Some(arg) = args.get("arg").and_then(serde_json::Value::as_str) {
            return super::world_arg_is_staging(arg)
                || arg.starts_with("view:")
                || arg.starts_with("conflicts:");
        }
    }
    false
}
