//! Plan-level transaction journal — snapshot before mutating ops, rollback on `X0`.

use super::program::{PlanStep, Program, TransactionMode};
use crate::core::FSZeroSession;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
struct FileSnapshot {
    path: PathBuf,
    bytes: Vec<u8>,
    existed: bool,
    mtime: Option<std::time::SystemTime>,
    perms: Option<std::fs::Permissions>,
    xattrs: Option<String>,
}

#[derive(Debug, Default)]
pub struct TransactionJournal {
    enabled: bool,
    files: HashMap<PathBuf, FileSnapshot>,
    created_dirs: HashSet<PathBuf>,
    memories: HashMap<String, Option<Vec<u8>>>,
    worlds_created: Vec<String>,
    worlds_committed: Vec<String>,
    rolled_back: bool,
}

impl TransactionJournal {
    pub fn for_program(program: &Program) -> Self {
        let enabled = match program.transaction {
            TransactionMode::On => true,
            TransactionMode::Off => false,
            TransactionMode::Auto => program_has_mutations(program),
        };
        Self {
            enabled,
            ..Self::default()
        }
    }

    pub fn always_on() -> Self {
        Self {
            enabled: true,
            ..Self::default()
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn rolled_back(&self) -> bool {
        self.rolled_back
    }

    pub fn before_edit(&mut self, session: &FSZeroSession, spec: &str) -> Result<(), String> {
        match edit_path_from_spec(spec) {
            Some(path) => self.before_edit_target(session, &path),
            None => Ok(()),
        }
    }

    pub fn before_edit_target(
        &mut self,
        session: &FSZeroSession,
        path: &str,
    ) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        let root = session
            .root
            .as_deref()
            .ok_or_else(|| "transaction mutation requires a rooted session".to_string())?;
        let resolved = crate::core::validate_rollback_path(root, Path::new(path))
            .map_err(|error| format!("snapshot path {path}: {error}"))?;
        self.snapshot_path(&resolved)?;
        self.record_missing_parent_dirs(root, &resolved);
        Ok(())
    }

    pub fn before_write(&mut self, session: &FSZeroSession, path: &str) -> Result<(), String> {
        self.before_edit_target(session, path)
    }

    /// Register an edit after dispatch from the kernel-owned certificate.
    /// Fused query edits do not know their path before dispatch, so this uses
    /// the certified path and preimage without a second filesystem operation.
    pub fn after_certified_edit(&mut self, session: &FSZeroSession) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        let cert = session
            .expand("last_cert")
            .ok_or_else(|| "transaction edit missing kernel certificate".to_string())?;
        let cert = String::from_utf8(cert)
            .map_err(|_| "transaction edit certificate is not UTF-8".to_string())?;
        let path = cert
            .lines()
            .find_map(|line| line.strip_prefix("path="))
            .ok_or_else(|| "transaction edit certificate missing path".to_string())?;
        let pre_ref = cert
            .lines()
            .find_map(|line| line.strip_prefix("pre="))
            .ok_or_else(|| "transaction edit certificate missing preimage ref".to_string())?;
        let pre_mtime_ns = certified_i64_field(&cert, "pre_mtime_ns")?;
        let pre_mode = certified_i64_field(&cert, "pre_mode")?;
        let pre_xattrs = cert
            .lines()
            .find_map(|line| line.strip_prefix("pre_xattrs="))
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let root = session
            .root
            .as_deref()
            .ok_or_else(|| "transaction mutation requires a rooted session".to_string())?;
        let resolved = crate::core::validate_rollback_path(root, Path::new(path))
            .map_err(|error| format!("certified rollback path {path}: {error}"))?;
        if self.files.contains_key(&resolved) {
            return Ok(());
        }
        let bytes = session
            .expand(pre_ref)
            .ok_or_else(|| format!("transaction edit missing certified preimage {pre_ref}"))?;
        self.files.insert(
            resolved.clone(),
            FileSnapshot {
                path: resolved,
                bytes,
                existed: true,
                mtime: pre_mtime_ns.and_then(system_time_from_ns),
                perms: pre_mode.and_then(permissions_from_mode),
                xattrs: pre_xattrs,
            },
        );
        Ok(())
    }

    pub fn before_undo(&mut self, session: &FSZeroSession, path: &str) -> Result<(), String> {
        let path = path
            .rsplit_once('|')
            .and_then(|(candidate, seq)| seq.parse::<i64>().ok().map(|_| candidate))
            .unwrap_or(path);
        self.before_edit_target(session, path)
    }

    pub fn before_memory(
        &mut self,
        session: &FSZeroSession,
        method: &str,
        args: &serde_json::Value,
    ) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        match method {
            "fs.memory.put" | "fs.memory.delete" => {
                self.snapshot_memory(
                    session,
                    args.get("path").and_then(serde_json::Value::as_str),
                )?;
            }
            "fs.memory.rename" => {
                self.snapshot_memory(
                    session,
                    args.get("from").and_then(serde_json::Value::as_str),
                )?;
                self.snapshot_memory(session, args.get("to").and_then(serde_json::Value::as_str))?;
            }
            "fs.memory" => match args.get("op").and_then(serde_json::Value::as_str) {
                Some("put" | "delete") => {
                    self.snapshot_memory(
                        session,
                        args.get("path").and_then(serde_json::Value::as_str),
                    )?;
                }
                Some("rename") => {
                    self.snapshot_memory(
                        session,
                        args.get("from").and_then(serde_json::Value::as_str),
                    )?;
                    self.snapshot_memory(
                        session,
                        args.get("to").and_then(serde_json::Value::as_str),
                    )?;
                }
                _ => {}
            },
            _ => {}
        }
        Ok(())
    }

    /// `fs.compound` with a `mem:` intent reaches the same durable-memory
    /// engine as `fs.memory.*`, so it needs the same preimage snapshot.
    pub fn before_compound_intent(
        &mut self,
        session: &FSZeroSession,
        intent: &str,
    ) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        let Some(spec) = intent.strip_prefix("mem:") else {
            return Ok(());
        };
        if let Some(rest) = spec.strip_prefix("put:") {
            let path = crate::core::decode_wire_path(
                rest.split_once('|').map_or(rest, |(path, _)| path).trim(),
            );
            self.snapshot_memory(session, Some(&path))?;
        } else if let Some(path) = spec.strip_prefix("delete:") {
            let path = crate::core::decode_wire_path(path.trim());
            self.snapshot_memory(session, Some(&path))?;
        } else if let Some(rest) = spec.strip_prefix("rename:")
            && let Some((from, to)) = rest.split_once('|')
        {
            let from = crate::core::decode_wire_path(from.trim());
            let to = crate::core::decode_wire_path(to.trim());
            self.snapshot_memory(session, Some(&from))?;
            self.snapshot_memory(session, Some(&to))?;
        }
        Ok(())
    }

    fn snapshot_memory(
        &mut self,
        session: &FSZeroSession,
        path: Option<&str>,
    ) -> Result<(), String> {
        let Some(path) = path else {
            return Ok(());
        };
        if self.memories.contains_key(path) {
            return Ok(());
        }
        let snapshot = match crate::core::get_memory(&session.recovery, path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.starts_with("memory miss:") => None,
            Err(error) => return Err(format!("memory snapshot {path}: {error}")),
        };
        self.memories.insert(path.to_string(), snapshot);
        Ok(())
    }

    pub fn before_world(&mut self, session: &FSZeroSession, arg: &str) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        if let Some(edit_spec) = arg.strip_prefix("newbatch:") {
            for spec in edit_spec.split(";;").filter(|s| !s.trim().is_empty()) {
                self.before_edit(session, spec)?;
            }
            return Ok(());
        }
        if let Some(edit_spec) = arg.strip_prefix("new:") {
            self.before_edit(session, edit_spec)?;
        } else if let Some(rest) = arg.strip_prefix("edit:") {
            if let Some((_wid, edit_spec)) = rest.split_once(':') {
                self.before_edit(session, edit_spec)?;
            }
        } else if let Some(wid) = arg.strip_prefix("commit:") {
            if wid.strip_suffix(":git").is_some() {
                return Err("transaction cannot roll back fs.world commit:...:git; set transaction off explicitly to allow git export".to_string());
            }
            self.snapshot_world_commit(session, wid)?;
        }
        Ok(())
    }

    pub fn before_verified_edit(
        &mut self,
        session: &FSZeroSession,
        args: &serde_json::Value,
    ) -> Result<(), String> {
        match args.get("path").and_then(serde_json::Value::as_str) {
            Some(path) => self.before_edit_target(session, path),
            None => Ok(()),
        }
    }
    pub fn record_world_created(&mut self, wid: &str) {
        if self.enabled {
            self.worlds_created.push(wid.to_string());
        }
    }

    pub fn record_world_committed(&mut self, wid: &str) {
        if self.enabled {
            self.worlds_committed.push(wid.to_string());
        }
    }

    pub fn rollback(&mut self, session: &mut FSZeroSession) -> Result<(), String> {
        if !self.enabled || self.rolled_back {
            return Ok(());
        }
        self.rolled_back = true;
        let mut errors = Vec::new();
        for snapshot in self.files.values() {
            if snapshot.existed {
                if let Err(e) = session.restore_file_for_rollback(
                    &snapshot.path,
                    &snapshot.bytes,
                    snapshot.mtime,
                    snapshot.perms.clone(),
                    snapshot.xattrs.clone(),
                ) {
                    errors.push(format!("{}: {e}", snapshot.path.display()));
                }
            } else if snapshot.path.exists() {
                if let Err(e) = session.remove_file_for_rollback(&snapshot.path) {
                    errors.push(format!("remove {}: {e}", snapshot.path.display()));
                }
            }
        }
        let mut created_dirs: Vec<_> = self.created_dirs.iter().collect();
        created_dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        for path in created_dirs {
            if let Err(error) = std::fs::remove_dir(path) {
                if error.kind() != std::io::ErrorKind::NotFound
                    && error.kind() != std::io::ErrorKind::DirectoryNotEmpty
                {
                    errors.push(format!("remove directory {}: {error}", path.display()));
                }
            }
        }
        for (path, bytes) in &self.memories {
            let result = match bytes {
                Some(bytes) => {
                    crate::core::put_memory(&mut session.recovery, path, bytes).map(|_| ())
                }
                None if crate::core::get_memory(&session.recovery, path).is_ok() => {
                    crate::core::delete_memory(&mut session.recovery, path)
                }
                None => Ok(()),
            };
            if let Err(error) = result {
                errors.push(format!("memory {path}: {error}"));
            }
        }
        for wid in self
            .worlds_created
            .iter()
            .chain(self.worlds_committed.iter())
        {
            session.drop_active_world(wid);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    fn snapshot_path(&mut self, path: &PathBuf) -> Result<(), String> {
        if self.files.contains_key(path) {
            return Ok(());
        }
        let (existed, bytes) = match std::fs::read(path) {
            Ok(bytes) => (true, bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (false, Vec::new()),
            Err(error) => return Err(format!("read snapshot {}: {error}", path.display())),
        };
        let meta = if existed {
            Some(
                std::fs::metadata(path)
                    .map_err(|error| format!("metadata snapshot {}: {error}", path.display()))?,
            )
        } else {
            None
        };
        let mtime = meta.as_ref().and_then(|metadata| metadata.modified().ok());
        let perms = meta.map(|metadata| metadata.permissions());
        let xattrs = existed.then(|| crate::core::xattrs_of(path)).flatten();
        self.files.insert(
            path.clone(),
            FileSnapshot {
                path: path.clone(),
                bytes,
                existed,
                mtime,
                perms,
                xattrs,
            },
        );
        Ok(())
    }

    fn record_missing_parent_dirs(&mut self, root: &Path, path: &Path) {
        let mut parent = path.parent();
        while let Some(directory) = parent {
            if directory == root || directory.exists() {
                break;
            }
            self.created_dirs.insert(directory.to_path_buf());
            parent = directory.parent();
        }
    }

    fn snapshot_world_commit(&mut self, session: &FSZeroSession, wid: &str) -> Result<(), String> {
        for path in session.world_edit_paths(wid) {
            self.snapshot_path(&path)?;
        }
        Ok(())
    }
}

fn certified_i64_field(cert: &str, field: &str) -> Result<Option<i64>, String> {
    let prefix = format!("{field}=");
    let Some(value) = cert.lines().find_map(|line| line.strip_prefix(&prefix)) else {
        return Ok(None);
    };
    value
        .parse::<i64>()
        .map(Some)
        .map_err(|_| format!("transaction edit certificate field {field} is not an integer"))
}

fn system_time_from_ns(ns: i64) -> Option<std::time::SystemTime> {
    (ns > 0).then(|| std::time::UNIX_EPOCH + std::time::Duration::from_nanos(ns as u64))
}

fn permissions_from_mode(mode: i64) -> Option<std::fs::Permissions> {
    if mode < 0 {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(std::fs::Permissions::from_mode(mode as u32))
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        None
    }
}

fn edit_path_from_spec(spec: &str) -> Option<String> {
    let (path, _) = spec.split_once(':')?;
    Some(path.trim().to_string())
}

pub(crate) fn world_arg_from_args(args: &serde_json::Value) -> Result<Option<String>, String> {
    if let Some(arg) = args.get("arg").and_then(serde_json::Value::as_str) {
        return Ok(Some(arg.to_string()));
    }
    crate::core::structured_world_arg(args).map_err(|error| error.message)
}

pub fn program_has_mutations(program: &Program) -> bool {
    program.steps.iter().any(step_is_mutating)
}

fn step_is_mutating(step: &PlanStep) -> bool {
    match step {
        PlanStep::Call { call, args, .. } => call_is_mutating(call, args),
        PlanStep::Parallel { branches, .. } => {
            branches.iter().any(|b| call_is_mutating(&b.call, &b.args))
        }
    }
}

/// A `mem:` compound intent writes durable memory even though `fs.compound`
/// carries no `name`; without this the plan is classified read-only and Auto
/// mode never arms the journal.
fn compound_intent_mutates(args: &serde_json::Value) -> bool {
    args.get("intent")
        .and_then(serde_json::Value::as_str)
        .and_then(|intent| intent.strip_prefix("mem:"))
        .is_some_and(|spec| {
            spec.starts_with("put:") || spec.starts_with("delete:") || spec.starts_with("rename:")
        })
}

fn call_is_mutating(call: &str, args: &serde_json::Value) -> bool {
    match call {
        "fs.write" | "fs.edit" | "fs.undo" | "fs.memory.put" | "fs.memory.delete"
        | "fs.memory.rename" => true,
        "fs.memory" => matches!(
            args.get("op").and_then(serde_json::Value::as_str),
            Some("put" | "delete" | "rename")
        ),
        "fs.compound" => {
            compound_intent_mutates(args)
                || matches!(
                    args.get("name").and_then(serde_json::Value::as_str),
                    Some("write" | "mutate" | "edit" | "verifiedEdit" | "undo")
                )
                || args.get("name").and_then(serde_json::Value::as_str) == Some("memory")
                    && matches!(
                        args.get("op").and_then(serde_json::Value::as_str),
                        Some("put" | "delete" | "rename")
                    )
                || args.get("name").and_then(serde_json::Value::as_str) == Some("world")
                    && world_arg_from_args(args)
                        .ok()
                        .flatten()
                        .is_some_and(|arg| crate::core::world_arg_mutates(&arg))
        }
        "fs.world" => world_arg_from_args(args)
            .ok()
            .flatten()
            .is_some_and(|arg| crate::core::world_arg_mutates(&arg)),
        _ => false,
    }
}

#[cfg(test)]
#[path = "../../../../../tests/fszero/unit/fs-zero/transaction_tests.rs"]
mod tests;
