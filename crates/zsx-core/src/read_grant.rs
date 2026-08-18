//! Bounded one-file read grants for explicit absolute reads outside the
//! session root (papercut `pc_c60a17138873`, bead `zerostack-k91e`).
//!
//! The session root is the approved workspace root: relative reads stay
//! there, and mutation/search/list operations stay root-confined. An
//! explicit absolute `fs.read` / `fs.multiRead` path is a *granted read*:
//! the plan first mints a [`SessionReadGrant`] for exactly one canonical
//! file via `fs.readGrant` (lowered from `zero.fs.read_grant` /
//! `zero.fs.compound("readGrant", ...)`), and every dispatch that reads
//! that file must match the grant's canonical identity again, fresh, at
//! dispatch time and after the engine read. Any symlink or path
//! substitution changes the canonical target and fails closed.
//!
//! The capability mirrors the session approval-grant pattern (typed,
//! bounded, single-use, expiring, recorded in the receipt) but for reads:
//! - one grant authorizes one exact canonical file, read only;
//! - the grant ledger is bounded (64) and grants expire (300s);
//! - a grant is consumed once; the consumed binding rides the dispatch and
//!   the adapter re-roots only for the granted file, never a directory
//!   wider than the granted file's own parent;
//! - the read receipt (`read_grant` / `read_grants` on the result value)
//!   records every consumed grant;
//! - fail-closed checks run at mint (canonicalize + regular file + outside
//!   the session root), at take (fresh canonicalize == granted path), at
//!   adapter planning (fresh canonicalize == binding, expiry), and after
//!   the engine read (the file still resolves to the granted path).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zero_abi::raw_worker::EngineIdentity;

/// Schema tag of one bounded read grant.
pub const SESSION_READ_GRANT_SCHEMA: &str = "zerostack.session.read_grant";

/// Upper bound on concurrently active read grants, mirroring the approval
/// grant ledger bound (`MAX_SESSION_APPROVAL_GRANTS`).
pub const MAX_SESSION_READ_GRANTS: usize = 64;

/// Read grant lifetime, mirroring the approval grant lifetime
/// (`MAX_SESSION_APPROVAL_LIFETIME_MS`).
pub const MAX_SESSION_READ_GRANT_LIFETIME_MS: u64 = 300_000;

/// One bounded read grant minted by `fs.readGrant` and consumed by a
/// matching `fs.read` / `fs.multiRead` dispatch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionReadGrant {
    pub schema: String,
    pub grant_id: String,
    pub engine: EngineIdentity,
    /// Canonical primary workspace root the grant was minted under.
    pub root: String,
    pub generation: u64,
    pub request_id: u64,
    /// `"fs.read"` (also covers `fs.multiRead`).
    pub operation: String,
    /// Canonical absolute path of the ONE granted file, resolved at mint
    /// time (symlinks already followed). This is the identity every later
    /// check must match byte-for-byte.
    pub canonical_path: String,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}

/// The consumed binding of one read grant, riding an in-flight dispatch
/// from the connector to the FSZero adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrantedReadFile {
    pub grant_id: String,
    pub canonical_path: PathBuf,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}

/// One absolute path of a read dispatch, classified against the session
/// root and (for external files) the consumed grant that authorizes it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadPathGrant {
    pub requested_path: String,
    pub canonical_path: PathBuf,
    /// `Some` for an externally granted file, `None` for a path inside the
    /// session root (no grant needed; root confinement already covers it).
    pub grant: Option<GrantedReadFile>,
}

/// One rewritten path of a granted external read plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadRewrite {
    pub requested_path: String,
    pub relative_path: String,
    pub grant: Option<GrantedReadFile>,
}

/// The temporary session re-root selected for one external read dispatch.
///
/// The root is exactly one granted file's own parent directory (a granted
/// `fs.multiRead` may only span files sharing that one parent) and the
/// rewritten request names exactly the granted files. The primary root is
/// restored after the call, and the session thread serializes dispatches,
/// so no other operation ever observes the widened root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadPlan {
    pub root: PathBuf,
    pub rewrites: Vec<ReadRewrite>,
}

/// Wall-clock milliseconds, shared by mint/take/plan so one time base
/// drives every grant check.
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or(u64::MAX)
}

/// Extract the absolute paths of a read dispatch. Returns an error when a
/// call mixes project-relative and absolute paths (the raw-worker rule:
/// one `fs.multiRead` call cannot mix them). Non-read operations yield no
/// paths.
pub fn absolute_read_paths(op: &str, args: &Value) -> Result<Vec<String>, String> {
    let paths: Vec<&str> = match op {
        "fs.read" => args
            .get("path")
            .or_else(|| args.get("arg"))
            .and_then(Value::as_str)
            .into_iter()
            .collect(),
        "fs.multiRead" => match args.get("paths").and_then(Value::as_array) {
            Some(values) if values.iter().all(Value::is_string) => {
                values.iter().filter_map(Value::as_str).collect()
            }
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    if paths.is_empty() || paths.iter().all(|path| !Path::new(path).is_absolute()) {
        return Ok(Vec::new());
    }
    if paths.iter().any(|path| !Path::new(path).is_absolute()) {
        return Err(
            "fs.multiRead cannot mix project-relative and absolute paths".to_string()
        );
    }
    Ok(paths.into_iter().map(str::to_owned).collect())
}

/// Mint one bounded read grant for exactly one canonical file outside the
/// session root.
///
/// Fail-closed checks, all before the grant enters the ledger:
/// - the path is absolute;
/// - it resolves (`canonicalize`) and is a regular file;
/// - the canonical target is NOT inside the session root (in-root files are
///   already readable under root confinement and need no grant);
/// - the ledger has room after pruning expired grants.
///
/// The returned grant's `canonical_path` is the fully resolved identity
/// (symlinks followed at mint time); every later consumption must match it.
pub fn mint_read_grant(
    active: &mut Vec<SessionReadGrant>,
    workspace_root: &Path,
    session_id: &str,
    generation: u64,
    request_id: u64,
    sequence: u64,
    path: &str,
    now: u64,
) -> Result<SessionReadGrant, String> {
    if !Path::new(path).is_absolute() {
        return Err(format!(
            "fs.readGrant requires an absolute path, got '{path}'"
        ));
    }
    let canonical = std::fs::canonicalize(Path::new(path)).map_err(|error| {
        format!(
            "fs.readGrant target '{path}' does not resolve: {error}"
        )
    })?;
    let metadata = std::fs::metadata(&canonical).map_err(|error| {
        format!(
            "fs.readGrant target '{}' cannot be inspected: {error}",
            canonical.display()
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "fs.readGrant target '{}' is not a regular file",
            canonical.display()
        ));
    }
    if canonical.starts_with(workspace_root) {
        return Err(format!(
            "fs.readGrant target '{}' is inside the session root '{}'; only files outside the approved root need a read grant",
            canonical.display(),
            workspace_root.display()
        ));
    }
    active.retain(|grant| now < grant.expires_at_unix_ms && now >= grant.issued_at_unix_ms);
    if active.len() >= MAX_SESSION_READ_GRANTS {
        return Err(format!(
            "fs.readGrant ledger is full (max {MAX_SESSION_READ_GRANTS} active grants)"
        ));
    }
    let grant = SessionReadGrant {
        schema: SESSION_READ_GRANT_SCHEMA.to_string(),
        grant_id: format!("read-grant-{session_id}-r{request_id}-{sequence}"),
        engine: EngineIdentity::FsZero,
        root: workspace_root.to_string_lossy().into_owned(),
        generation,
        request_id,
        operation: "fs.read".to_string(),
        canonical_path: canonical.to_string_lossy().into_owned(),
        issued_at_unix_ms: now,
        expires_at_unix_ms: now.saturating_add(MAX_SESSION_READ_GRANT_LIFETIME_MS),
    };
    active.push(grant.clone());
    Ok(grant)
}

/// Consume the read grants authorizing a set of absolute read paths.
///
/// Each absolute path is canonicalized fresh. Paths inside the session root
/// need no grant; every path outside it must match an active grant's
/// canonical identity exactly (a symlink or substitution that resolves
/// anywhere else fails closed). All-or-nothing: when any path lacks a
/// matching grant, no grant is consumed and the whole call is rejected.
pub fn take_read_grants(
    active: &mut Vec<SessionReadGrant>,
    workspace_root: &Path,
    op: &str,
    paths: &[String],
    now: u64,
) -> Result<Vec<ReadPathGrant>, String> {
    active.retain(|grant| now < grant.expires_at_unix_ms && now >= grant.issued_at_unix_ms);
    let mut entries: Vec<ReadPathGrant> = Vec::with_capacity(paths.len());
    // (active ledger index, entry index) pairs for externally granted paths.
    let mut matched: Vec<(usize, usize)> = Vec::with_capacity(paths.len());
    for (entry_index, path) in paths.iter().enumerate() {
        let target = match std::fs::canonicalize(Path::new(path)) {
            Ok(target) => target,
            // An unresolvable path inside the session root keeps the
            // engine's own not-found semantics (no grant involved); an
            // unresolvable external path fails closed.
            Err(error) => {
                let raw = PathBuf::from(path);
                if raw.starts_with(workspace_root) {
                    entries.push(ReadPathGrant {
                        requested_path: path.clone(),
                        canonical_path: raw,
                        grant: None,
                    });
                    continue;
                }
                return Err(format!(
                    "explicit read path '{path}' does not resolve: {error}"
                ));
            }
        };
        if target.starts_with(workspace_root) {
            entries.push(ReadPathGrant {
                requested_path: path.clone(),
                canonical_path: target,
                grant: None,
            });
            continue;
        }
        let Some(active_index) = active
            .iter()
            .position(|grant| Path::new(&grant.canonical_path) == target)
        else {
            return Err(format!(
                "explicit read outside the session root requires a read grant; no active grant matches canonical path '{}'; mint one with fs.readGrant({{ path }}) first",
                target.display()
            ));
        };
        if matched
            .iter()
            .any(|(prior_index, _)| *prior_index == active_index)
        {
            return Err(format!(
                "explicit read path '{}' repeats a granted file; mint one read grant per occurrence",
                path
            ));
        }
        matched.push((active_index, entry_index));
        entries.push(ReadPathGrant {
            requested_path: path.clone(),
            canonical_path: target,
            grant: None, // bound below, once every path validated
        });
    }
    // All-or-nothing: only now that every path validated, remove the matched
    // grants (descending ledger indices so earlier indices stay valid) and
    // bind each consumed grant to its entry. A `fs.multiRead` that mixes
    // in-root and externally granted paths is rejected BEFORE any grant is
    // consumed (the adapter re-checks the same rule as defense in depth).
    if op == "fs.multiRead" && !matched.is_empty() && entries.len() != matched.len() {
        return Err(
            "fs.multiRead cannot mix session-root and externally granted read paths".to_string(),
        );
    }
    matched.sort_by_key(|(active_index, _)| *active_index);
    for (active_index, entry_index) in matched.into_iter().rev() {
        let grant = active.remove(active_index);
        entries[entry_index].grant = Some(GrantedReadFile {
            grant_id: grant.grant_id,
            canonical_path: PathBuf::from(grant.canonical_path),
            issued_at_unix_ms: grant.issued_at_unix_ms,
            expires_at_unix_ms: grant.expires_at_unix_ms,
        });
    }
    Ok(entries)
}

/// Adapter-side verification and temporary re-root plan for one read
/// dispatch with absolute paths.
///
/// Runs again on the session thread, fresh: every absolute path is
/// canonicalized and must either lie inside the session root (no grant) or
/// match a consumed [`GrantedReadFile`] byte-for-byte (grant still valid).
/// A symlink or path substitution between the connector take and this check
/// changes the canonical target and fails closed.
///
/// Returns `None` when the dispatch has no absolute paths (plain relative
/// read, no re-root). The planned root is the primary root when every
/// absolute path is in-root, otherwise the granted file's own parent
/// directory. `fs.multiRead` may not mix in-root and externally granted
/// paths, and its granted files must share one parent directory.
pub fn plan_granted_read(
    workspace_root: Option<&Path>,
    op: &str,
    paths: &[String],
    bindings: &[GrantedReadFile],
    now: u64,
) -> Result<Option<ReadPlan>, String> {
    if paths.is_empty() {
        return Ok(None);
    }
    let Some(workspace_root) = workspace_root else {
        return Err("explicit absolute read requires an active primary root".to_string());
    };
    let mut rewrites: Vec<ReadRewrite> = Vec::with_capacity(paths.len());
    for path in paths {
        let grant = match std::fs::canonicalize(Path::new(path)) {
            Ok(target) => {
                if target.starts_with(workspace_root) {
                    None
                } else {
                    let binding = bindings
                        .iter()
                        .find(|binding| binding.canonical_path == target)
                        .cloned();
                    let Some(binding) = binding else {
                        return Err(format!(
                            "explicit read '{}' has no matching read grant; path substituted, symlink changed, or grant already consumed",
                            path
                        ));
                    };
                    if now >= binding.expires_at_unix_ms {
                        return Err(format!(
                            "read grant '{}' for '{}' expired",
                            binding.grant_id,
                            binding.canonical_path.display()
                        ));
                    }
                    Some(binding)
                }
            }
            // An unresolvable in-root path keeps the engine's own
            // not-found semantics; an unresolvable external path fails
            // closed (the grant names a real file).
            Err(error) => {
                let raw = Path::new(path);
                if raw.starts_with(workspace_root) {
                    None
                } else {
                    return Err(format!(
                        "explicit read path '{path}' does not resolve: {error}"
                    ));
                }
            }
        };
        rewrites.push(ReadRewrite {
            requested_path: path.clone(),
            relative_path: String::new(),
            grant,
        });
    }

    let external: Vec<&ReadRewrite> = rewrites
        .iter()
        .filter(|rewrite| rewrite.grant.is_some())
        .collect();
    let root = if external.is_empty() {
        workspace_root.to_path_buf()
    } else {
        if op == "fs.multiRead" && external.len() != rewrites.len() {
            return Err(
                "fs.multiRead cannot mix session-root and externally granted read paths"
                    .to_string(),
            );
        }
        // The temporary root is never wider than one granted file's own
        // parent directory, so a granted multiRead may only span files that
        // share one parent; files in different directories are read with
        // separate fs.read calls.
        let mut parents: Vec<PathBuf> = Vec::new();
        for rewrite in &external {
            let parent = rewrite
                .grant
                .as_ref()
                .expect("external rewrites always carry a grant")
                .canonical_path
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| "granted read path has no parent directory".to_string())?;
            if !parents.iter().any(|prior| prior == &parent) {
                parents.push(parent);
            }
        }
        if parents.len() != 1 {
            return Err(
                "granted read files must share one parent directory; read files in different directories with separate fs.read calls"
                    .to_string(),
            );
        }
        parents.pop().expect("granted read files share one parent")
    };

    for rewrite in &mut rewrites {
        let canonical = if let Some(grant) = &rewrite.grant {
            grant.canonical_path.clone()
        } else {
            std::fs::canonicalize(Path::new(&rewrite.requested_path))
                .unwrap_or_else(|_| PathBuf::from(&rewrite.requested_path))
        };
        let relative = canonical.strip_prefix(&root).map_err(|_| {
            format!(
                "explicit read path '{}' is outside selected root '{}'",
                rewrite.requested_path,
                root.display()
            )
        })?;
        if relative.as_os_str().is_empty() {
            return Err(
                "explicit read path must name a file below its selected root".to_string()
            );
        }
        rewrite.relative_path = relative.to_string_lossy().into_owned();
    }
    Ok(Some(ReadPlan { root, rewrites }))
}

/// Post-read fail-closed verification: every externally granted file must
/// still resolve to the granted canonical path after the engine read.
/// Catches a file swapped (or re-symlinked) between the adapter's planning
/// check and the engine's open; the read result is discarded on mismatch.
pub fn post_verify_read(plan: &ReadPlan) -> Result<(), String> {
    for rewrite in &plan.rewrites {
        let Some(grant) = &rewrite.grant else {
            continue;
        };
        let resolved = std::fs::canonicalize(plan.root.join(&rewrite.relative_path))
            .map_err(|error| {
                format!(
                    "granted read target '{}' vanished during read: {error}",
                    grant.canonical_path.display()
                )
            })?;
        if resolved != grant.canonical_path {
            return Err(format!(
                "granted read target substituted during read: expected '{}', resolved '{}'",
                grant.canonical_path.display(),
                resolved.display()
            ));
        }
    }
    Ok(())
}
