use super::access_log::{content_hash_bytes, rel_path_for_log};
use super::edit_spec::{apply_unique_replace, parse_path_edit_spec};
use super::external_edit::{ExternalEffectDisposition, ExternalEffectReceipt};
use super::*;
use std::process::Command;

/// World arg creates a new speculative world (fork / new: / newbatch:).
pub fn world_arg_creates(arg: &str) -> bool {
    arg == "fork" || arg.starts_with("new:") || arg.starts_with("newbatch:")
}

#[inline]
fn world0(detail: impl std::fmt::Display) -> String {
    super::op_result::op0("world", detail)
}

#[inline]
fn unknown_world() -> String {
    world0("unknown world")
}

#[inline]
fn unreadable_conflict(rel: impl AsRef<str>, e: impl std::fmt::Display) -> serde_json::Value {
    serde_json::json!({"file": rel.as_ref(), "reason": "unreadable", "detail": e.to_string()})
}

fn mark_world_dropped(sess: &mut FSZeroSession, wid: &str) -> bool {
    if sess.worlds.active.remove(wid).is_none() {
        return false;
    }
    if sess.recovery.is_durable() {
        let _ = sess.recovery.set_world_state(wid, "dropped");
    }
    true
}

#[inline]
fn world_persist0(e: impl std::fmt::Display) -> String {
    world0(format!("persist: {e}"))
}

/// Recovery-store key holding the commit-intent record of `wid` (fszero-k4ur.3).
#[inline]
fn commit_intent_key(wid: &str) -> String {
    format!("world/{wid}/commit_intent")
}

/// Register world from create path and format `world:1 {wid}{conflicts}`.
fn world1_created(session: &mut FSZeroSession, result: Result<String, String>) -> String {
    match result {
        Ok(wid) => format!("world:1 {wid}{}", session.conflict_suffix(&wid)),
        Err(e) => world0(e),
    }
}

#[inline]
fn with_rb(msg: String, rb: Option<String>) -> String {
    match rb {
        Some(r) => format!("{msg}; rollback failed for {r}"),
        None => msg,
    }
}

/// Test/oracle hook (fszero-k4ur.2): abort after `k` successful world-file
/// publishes when `FSZERO_CRASH_AFTER_WORLD_WRITES=k`. Simulates SIGKILL mid
/// multi-file `commit_world` — no compensating rollback runs.
fn maybe_crash_after_world_writes(writes_done: usize) {
    let Ok(raw) = std::env::var("FSZERO_CRASH_AFTER_WORLD_WRITES") else {
        return;
    };
    let Ok(k) = raw.parse::<usize>() else {
        return;
    };
    if writes_done == k && k > 0 {
        eprintln!("FSZERO_CRASH_AFTER_WORLD_WRITES={k}: aborting after {writes_done} publish(es)");
        std::process::abort();
    }
}

/// Stable v1 world tree envelope (preview/view). Optional counts for preview.
fn world_v1_payload(
    wid: &str,
    files: Vec<serde_json::Value>,
    counts: Option<(usize, usize)>,
) -> String {
    let mut v = serde_json::json!( { "version": 1, "world_ref": format!("fz://world/{wid}"), "world": wid, "files": files, });
    if let Some((n_files, n_changed)) = counts {
        v["counts"] = serde_json::json!({"files": n_files, "changed": n_changed});
    }
    v.to_string()
}

/// World arg stages an edit into a world.
#[inline]
pub fn world_arg_stages_edit(arg: &str) -> bool {
    arg.starts_with("edit:")
}

/// Create or stage-edit (parallel world-staging classification).
pub fn world_arg_is_staging(arg: &str) -> bool {
    world_arg_creates(arg) || world_arg_stages_edit(arg)
}

/// Staging plus commit (transaction journal mutation classification).
pub fn world_arg_mutates(arg: &str) -> bool {
    world_arg_is_staging(arg) || arg.starts_with("commit:")
}

#[derive(Debug, Clone)]
pub struct WorldEdit {
    pub edits: Vec<WorldFileEdit>,
    pub cert_ref: String,
}

#[derive(Debug, Clone)]
pub struct WorldFileEdit {
    pub path: PathBuf,
    pub pre: String,
    pub post: String,
    pub cert_ref: String,
    /// Original find/replace pair, kept for commit-time three-way re-apply
    /// (fszero-glg): a diverged base that still contains a unique `old`
    /// auto-merges; anything else is a structured conflict.
    pub old: String,
    pub new: String,
    /// 1-based inclusive line span of the replaced region within `pre`,
    /// the unit of cross-world conflict detection (fszero-4wp). A trailing
    /// newline in `old` extends the span one line (conservative).
    pub hunk: (u32, u32),
}

/// 1-based inclusive line span of the (unique) `old` occurrence in `pre`.
pub fn hunk_lines(pre: &str, old: &str) -> (u32, u32) {
    let off = pre.find(old).unwrap_or(0);
    let start = pre[..off].matches('\n').count() as u32 + 1;
    (start, start + old.matches('\n').count() as u32)
}

fn hunks_overlap(a: (u32, u32), b: (u32, u32)) -> bool {
    a.0 <= b.1 && b.0 <= a.1
}

/// Distinct file paths in edit order (rebase / preview / view share this).
fn unique_edit_paths(edits: &[WorldFileEdit]) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for e in edits {
        if !paths.contains(&e.path) {
            paths.push(e.path.clone());
        }
    }
    paths
}

/// One world's view of a file (fszero-1wm): every staged edit for `path`
/// replayed over `current`, exactly the sequence commit_world would apply.
/// Never touches disk.
///
/// Overlapping hunks on the same path are a structured conflict, even when
/// unique-replace would still apply (no last-write-wins). A moved base that
/// overlaps a previously committed world's hunk is the same conflict.
fn overlay_file_content(
    edits: &[WorldFileEdit],
    path: &Path,
    current: &str,
    committed: &[(PathBuf, (u32, u32))],
) -> Result<String, &'static str> {
    let path_edits: Vec<&WorldFileEdit> = edits.iter().filter(|e| e.path == path).collect();
    for (i, a) in path_edits.iter().enumerate() {
        for b in path_edits.iter().skip(i + 1) {
            if hunks_overlap(a.hunk, b.hunk) {
                return Err("overlapping conflict");
            }
        }
    }
    let mut cur = current.to_string();
    for e in &path_edits {
        cur = if cur == e.pre {
            e.post.clone()
        } else {
            apply_unique_replace(&cur, &e.old, &e.new)?
        };
    }
    if let Some(first) = path_edits.first() {
        if current != first.pre.as_str()
            && path_edits.iter().any(|e| {
                committed
                    .iter()
                    .any(|(p, h)| p == path && hunks_overlap(e.hunk, *h))
            })
        {
            return Err("overlapping conflict");
        }
    }
    Ok(cur)
}

impl FSZeroSession {
    /// Content-address pre and post bytes; return `(pre_ref, post_ref)`.
    #[inline]
    pub fn put_pre_post(&mut self, pre: &[u8], post: &[u8]) -> (String, String) {
        (
            self.recovery.put_content_ref(pre),
            self.recovery.put_content_ref(post),
        )
    }

    /// Next mutation-journal seq: the durable monotonic ordinal used as the
    /// detection-time stamp on external-effect receipts (V6-F4 / ZS-STORE-005).
    fn next_mutation_seq(&self) -> i64 {
        self.recovery
            .query_mutations("", None, 1)
            .first()
            .map(|row| row.seq + 1)
            .unwrap_or(1)
    }

    /// The session's last known state for a committed path (V6-F4 rescan
    /// gate baseline). The traced delta -- the mutation-journal postimage --
    /// wins over the world's staging read, unless the last staged edit
    /// DECLARED a new base via resolve:mine / resolve:merged / rebase (its
    /// cert header marks the declaration), in which case the declared base
    /// is the known state. Returns `(known_bytes, declared)`.
    fn session_known_state(
        &self,
        edits: &[WorldFileEdit],
        root: Option<&Path>,
        rel: &str,
    ) -> (Vec<u8>, bool) {
        let last = edits
            .iter()
            .rev()
            .find(|e| rel_path_for_log(root, &e.path) == rel);
        let Some(edit) = last else {
            return (Vec::new(), false);
        };
        let declared = self.recovery.expand(&edit.cert_ref).is_some_and(|cert| {
            let text = String::from_utf8_lossy(&cert);
            text.starts_with("world_resolve") || text.starts_with("world_rebase")
        });
        if declared {
            return (edit.pre.clone().into_bytes(), true);
        }
        if let Some(row) = self.recovery.query_mutations(rel, None, 1).first() {
            if row.post_ref.is_empty() {
                // The session's own last write deleted the file.
                return (Vec::new(), false);
            }
            if let Some(post) = self.recovery.expand(&row.post_ref) {
                return (post, false);
            }
        }
        (edit.pre.clone().into_bytes(), false)
    }

    /// Append external-effect receipts to the world's durable record
    /// (`world_{wid}/external_effects`, CAS ref returned). Refused commit
    /// gates and declared resolve/rebase absorbs both land here, so the
    /// record is the complete external-effect story of the world. Deterministic
    /// wire shape: `{"world", "receipts": [...]}` sorted by path.
    fn record_external_effect_receipts(
        &mut self,
        wid: &str,
        mut receipts: Vec<ExternalEffectReceipt>,
    ) -> String {
        receipts.sort_by(|a, b| a.path.cmp(&b.path));
        let key = Self::world_storage_key(wid, "external_effects");
        let mut all: Vec<ExternalEffectReceipt> = self
            .recovery
            .payload(&key)
            .and_then(|p| serde_json::from_slice::<serde_json::Value>(&p).ok())
            .and_then(|v| serde_json::from_value(v.get("receipts")?.clone()).ok())
            .unwrap_or_default();
        all.extend(receipts);
        let payload = serde_json::json!({ "world": wid, "receipts": all }).to_string();
        self.put_world_key(wid, "external_effects", payload.as_bytes())
    }

    /// One declared-absorb receipt for a re-baseline (resolve:mine /
    /// resolve:merged / rebase) whose base differs from the session's known
    /// state; `None` when the base is the session's own state (no absorb).
    fn declared_absorb_receipt(
        &self,
        edits: &[WorldFileEdit],
        root: Option<&Path>,
        rel: &str,
        declared_base: &str,
    ) -> Option<ExternalEffectReceipt> {
        let (known, _) = self.session_known_state(edits, root, rel);
        if known == declared_base.as_bytes() {
            return None;
        }
        Some(ExternalEffectReceipt::new(
            rel.to_string(),
            content_hash_bytes(&known),
            if declared_base.is_empty() {
                String::new()
            } else {
                content_hash_bytes(declared_base.as_bytes())
            },
            self.next_mutation_seq(),
            ExternalEffectDisposition::DeclaredAbsorb,
        ))
    }

    /// Durable commit-intent record for a multi-file world publish
    /// (fszero-k4ur.3). Written, with the world moved to `committing`, BEFORE
    /// the first workspace byte lands, so a kill anywhere inside the publish
    /// loop leaves enough evidence on reopen to collapse a Partial workspace
    /// back into legal set L. One tab-separated line per planned path:
    /// `rel<TAB>pre_ref<TAB>post_ref`.
    fn record_commit_intent(&mut self, wid: &str, planned: &[(PathBuf, String, String)]) {
        if !self.recovery.is_durable() {
            return;
        }
        let root = self.root.clone();
        let mut body = String::new();
        for (path, current, merged) in planned {
            let (pre_ref, post_ref) = self.put_pre_post(current.as_bytes(), merged.as_bytes());
            body.push_str(&format!(
                "{}\t{pre_ref}\t{post_ref}\n",
                rel_path_for_log(root.as_deref(), path)
            ));
        }
        self.recovery
            .put_key(&commit_intent_key(wid), body.as_bytes());
        let _ = self.recovery.set_world_state(wid, "committing");
    }

    /// Retire the intent record once the publish reached a terminal outcome
    /// (acked commit, or compensating rollback back to AllPre).
    fn clear_commit_intent(&mut self, wid: &str) {
        if !self.recovery.is_durable() {
            return;
        }
        let _ = self
            .recovery
            .delete_payload_and_lru(&commit_intent_key(wid));
    }

    /// Collapse worlds killed mid-publish back to AllPre and re-arm them for
    /// retry (fszero-k4ur.3). Runs on every durable open, before world
    /// rehydration, so no caller ever observes a stable Partial workspace.
    /// A path whose bytes are neither pre nor post was changed by someone else
    /// after the crash: it is left alone rather than clobbered.
    pub fn recover_committing_worlds(&mut self) {
        if !self.recovery.is_durable() {
            return;
        }
        let root = self.root.clone();
        for wid in self.recovery.list_committing_worlds() {
            let Some(intent) = self.recovery.payload(&commit_intent_key(&wid)) else {
                // No intent record: nothing to undo, but the world must not stay
                // stuck in the waypoint state.
                let _ = self.recovery.set_world_state(&wid, "active");
                continue;
            };
            for line in String::from_utf8_lossy(&intent).lines() {
                let mut cols = line.split('\t');
                let (Some(rel), Some(pre_ref), Some(post_ref)) =
                    (cols.next(), cols.next(), cols.next())
                else {
                    continue;
                };
                let path = match root.as_deref() {
                    Some(r) => r.join(rel),
                    None => PathBuf::from(rel),
                };
                let (Some(pre), Some(post)) = (
                    self.recovery.expand(pre_ref),
                    self.recovery.expand(post_ref),
                ) else {
                    continue;
                };
                if crate::path::refuse_non_regular_file(&path).is_err() {
                    continue;
                }
                let live = fs::read(&path).ok();
                if live.as_deref() == Some(pre.as_slice()) {
                    continue;
                }
                if live.as_deref() != Some(post.as_slice()) {
                    continue;
                }
                if pre.is_empty() && live.is_some() {
                    let _ = fs::remove_file(&path);
                } else {
                    let _ = atomic_write(&path, &pre);
                }
                self.refresh_path_after_mutation(&path);
                // V6-F4 (ZS-STORE-005): journal the recovery rollback so the
                // traced delta matches the workspace the session actually
                // reopens with. Without this row the crashed write would
                // stand as the last known state and trip the commit rescan
                // gate on the re-armed world's retry. Preimage of the
                // recovery = the crashed postimage; postimage = the restored
                // bytes (CAS ref already in the intent, or missing).
                let recovered_ref = if pre.is_empty() && live.is_some() {
                    String::new()
                } else {
                    pre_ref.to_string()
                };
                let ts = super::recovery::unix_epoch_secs();
                let agent = std::env::var("FSZERO_AGENT_ID").unwrap_or_default();
                if let Err(e) = self.recovery.append_mutation(
                    ts,
                    "recover",
                    rel,
                    post_ref,
                    &recovered_ref,
                    false,
                    self.access_session_window,
                    &agent,
                    0,
                    -1,
                    "",
                ) {
                    // Recovery stays best-effort; a missing row surfaces as a
                    // refused retry commit (fail loud), never a silent absorb.
                    eprintln!("recover_committing_worlds: journal rollback failed: {e}");
                }
            }
            let _ = self.recovery.set_world_state(&wid, "active");
            let _ = self
                .recovery
                .delete_payload_and_lru(&commit_intent_key(&wid));
        }
    }

    /// Mint pre/post refs and a cert body `{header}pre=…\npost=…\n`.
    pub fn mint_pre_post_cert(
        &mut self,
        header: &str,
        pre: &[u8],
        post: &[u8],
    ) -> (String, String, String) {
        let (pre_ref, post_ref) = self.put_pre_post(pre, post);
        let cert = format!("{header}pre={pre_ref}\npost={post_ref}\n");
        let cert_ref = self.recovery.put_content_ref(cert.as_bytes());
        (pre_ref, post_ref, cert_ref)
    }

    /// Expand cert pre/post refs + payloads (shared by verify_cert + rehydrate).
    pub fn expand_cert_pre_post_payloads<'a>(
        &mut self,
        cert: &'a str,
    ) -> Result<(&'a str, Vec<u8>, &'a str, Vec<u8>), String> {
        let need = |field: &str, msg: &str| cert_field(cert, field).ok_or_else(|| msg.to_string());
        let pre_ref = need("pre", "missing pre ref")?;
        let post_ref = need("post", "missing post ref")?;
        let expand = |r: &str, msg: &str| self.recovery.expand(r).ok_or_else(|| msg.to_string());
        Ok((
            pre_ref,
            expand(pre_ref, "missing pre payload")?,
            post_ref,
            expand(post_ref, "missing post payload")?,
        ))
    }

    pub fn verify_cert(&mut self, cert_ref: &str) -> Result<String, String> {
        let cert_bytes = self
            .recovery
            .expand(cert_ref)
            .or_else(|| self.recovery.expand("last_cert"))
            .ok_or_else(|| "missing cert".to_string())?;
        let cert = String::from_utf8_lossy(&cert_bytes);
        let (pre_ref, pre, post_ref, post) = self.expand_cert_pre_post_payloads(&cert)?;
        if !content_ref_matches(pre_ref, &pre) {
            return Err("pre hash mismatch".to_string());
        }
        if !content_ref_matches(post_ref, &post) {
            return Err("post hash mismatch".to_string());
        }
        let report = format!(
            "cert:ok\ncert={cert_ref}\npre_bytes={}\npost_bytes={}\n",
            pre.len(),
            post.len()
        );
        self.recovery.put_key("last_cert_verify", report.as_bytes());
        Ok(report)
    }

    pub fn create_world_from_edit(&mut self, spec: &str) -> Result<String, String> {
        self.create_world_from_edits(std::slice::from_ref(&spec))
    }

    pub fn create_world_from_batch(&mut self, spec: &str) -> Result<String, String> {
        let edit_specs: Vec<&str> = spec.split(";;").filter(|s| !s.trim().is_empty()).collect();
        if edit_specs.is_empty() {
            return Err("empty batch".to_string());
        }
        self.create_world_from_edits(&edit_specs)
    }

    fn create_world_from_edits(&mut self, specs: &[&str]) -> Result<String, String> {
        let mut edits = Vec::with_capacity(specs.len());
        for spec in specs {
            edits.push(self.prepare_world_file_edit(spec)?);
        }
        let cert_refs: Vec<String> = edits.iter().map(|edit| edit.cert_ref.clone()).collect();
        let batch_cert = format!(
            "world_batch\nedits={}\ncerts={}\n",
            edits.len(),
            cert_refs.join(",")
        );
        let cert_ref = self.recovery.put_content_ref(batch_cert.as_bytes());
        if cert_ref == "fz://blob/error" {
            return Err(self
                .recovery
                .take_store_error()
                .unwrap_or_else(|| "world cert store failed".to_string()));
        }
        let wid = self.register_active_world(WorldEdit {
            edits,
            cert_ref: cert_ref.clone(),
        });
        self.persist_active_world(&wid)?;
        self.put_world_manifest(&wid, &cert_ref, specs.len());
        Ok(wid)
    }

    /// Allocate `W{n}` and insert into the active world map.
    fn register_active_world(&mut self, world: WorldEdit) -> String {
        let wid = format!("W{}", self.worlds.next_id);
        self.worlds.next_id += 1;
        self.worlds.active.insert(wid.clone(), world);
        wid
    }

    fn world_storage_key(wid: &str, key: &str) -> String {
        format!("world_{wid}/{key}")
    }

    fn put_world_manifest(&mut self, wid: &str, cert_ref: &str, edits_len: usize) {
        let manifest = format!("{wid}\ncert={cert_ref}\nedits={edits_len}\n");
        self.recovery.put_key(
            &Self::world_storage_key(wid, "manifest"),
            manifest.as_bytes(),
        );
    }

    /// Named recovery payload under `world_{wid}/{key}`.
    fn put_world_key(&mut self, wid: &str, key: &str, data: &[u8]) -> String {
        self.recovery
            .put_payload_at_key(&Self::world_storage_key(wid, key), data)
    }

    /// Persist `{"world", "conflicts"}` under `world_{wid}/{key}` (`conflict` / `conflicts`).
    fn put_world_conflict_payload(
        &mut self,
        wid: &str,
        key: &str,
        conflicts: &[serde_json::Value],
    ) -> String {
        let report = serde_json::json!({"world": wid, "conflicts": conflicts}).to_string();
        self.put_world_key(wid, key, report.as_bytes())
    }

    /// `files={n} ref={cref}` for merge/rebase conflict failures.
    fn world_conflict_note(&mut self, wid: &str, conflicts: &[serde_json::Value]) -> String {
        let n = conflicts.len();
        let cref = self.put_world_conflict_payload(wid, "conflict", conflicts);
        format!("files={n} ref={cref}")
    }

    fn prepare_world_file_edit(&mut self, spec: &str) -> Result<WorldFileEdit, String> {
        let (path_arg, old, new) = parse_path_edit_spec(spec).map_err(|error| {
            format!("invalid world edit spec ({error}); expected <path>:<find>|<replace>")
        })?;
        let root = self.root.clone();
        let target_path = self
            .resolve_existing_path_cached(root.as_deref(), &path_arg)
            .map_err(|e| e.to_string())?;
        crate::path::refuse_non_regular_file(&target_path)?;
        let source = fs::read_to_string(&target_path).map_err(|e| e.to_string())?;
        let updated = apply_unique_replace(&source, &old, &new).map_err(|e| e.to_string())?;
        let (pre_ref, post_ref) = self.put_pre_post(source.as_bytes(), updated.as_bytes());
        let cert_ref = self.store_edit_cert(&target_path, &pre_ref, &post_ref, &old, &new);
        let hunk = hunk_lines(&source, &old);
        Ok(WorldFileEdit {
            path: target_path,
            pre: source,
            post: updated,
            cert_ref,
            old: old.to_string(),
            new: new.to_string(),
            hunk,
        })
    }

    /// O(1) world fork (fszero-ap9): registers an empty world with no tree
    /// scan and no file reads — cost independent of repo size. Durable
    /// sessions still upsert the empty active row so restart rehydrates the
    /// fork id (fszero-w2g.46 / INV-W1).
    pub fn fork_world(&mut self) -> String {
        let wid = self.register_active_world(WorldEdit {
            edits: Vec::new(),
            cert_ref: String::new(),
        });
        let _ = self.persist_active_world(&wid);
        wid
    }

    /// Persist the in-memory world row into SQLite when the session is durable.
    /// Shared by create/stage/fork/resolve/rebase (fszero-w2g.10 / .18 / .46).
    fn persist_active_world(&mut self, wid: &str) -> Result<(), String> {
        if !self.recovery.is_durable() {
            return Ok(());
        }
        let Some(world) = self.worlds.active.get(wid) else {
            return Err("unknown world".to_string());
        };
        let persist_edits: Vec<(String, String)> = world
            .edits
            .iter()
            .map(|e| (e.path.to_string_lossy().into_owned(), e.cert_ref.clone()))
            .collect();
        let cert_ref = world.cert_ref.clone();
        self.recovery.upsert_active_world(
            wid,
            &cert_ref,
            &persist_edits,
            self.access_session_window,
        )
    }

    /// Stage one more edit into an existing world (journal append).
    pub fn stage_world_edit(&mut self, wid: &str, spec: &str) -> Result<(u32, u32), String> {
        if !self.worlds.active.contains_key(wid) {
            return Err("unknown world".to_string());
        }
        let edit = self.prepare_world_file_edit(spec)?;
        let hunk = edit.hunk;
        let world = self.worlds.active.get_mut(wid).expect("checked above");
        world.edits.push(edit);
        let cert_ref = world.cert_ref.clone();
        let n = world.edits.len();
        self.put_world_manifest(wid, &cert_ref, n);
        self.persist_active_world(wid)?;
        Ok(hunk)
    }

    /// Live hunk-level cross-world overlap (fszero-4wp): every edit of `wid`
    /// against every other open world's edits on the same file. Runs on each
    /// journal append (cheap: worlds and per-world edits are small) so agents
    /// learn about collisions at edit time, not at commit.
    pub fn world_conflicts(&self, wid: &str) -> Vec<(String, PathBuf, (u32, u32), (u32, u32))> {
        let Some(world) = self.worlds.active.get(wid) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (other_id, other) in &self.worlds.active {
            if other_id == wid {
                continue;
            }
            for e in &world.edits {
                for oe in &other.edits {
                    if e.path == oe.path && hunks_overlap(e.hunk, oe.hunk) {
                        out.push((other_id.clone(), e.path.clone(), e.hunk, oe.hunk));
                    }
                }
            }
        }
        out.sort();
        out
    }

    fn conflict_suffix(&self, wid: &str) -> String {
        let conflicts = self.world_conflicts(wid);
        if conflicts.is_empty() {
            return String::new();
        }
        let root = self.root.clone();
        let items: Vec<String> = conflicts
            .iter()
            .map(|(other, path, ours, theirs)| {
                format!(
                    "{other}:{}:{}-{}&{}-{}",
                    rel_path_for_log(root.as_deref(), path),
                    ours.0,
                    ours.1,
                    theirs.0,
                    theirs.1
                )
            })
            .collect();
        format!(" conflicts={}", items.join(","))
    }

    /// Commit with three-way merge (fszero-glg): (base at stage, world edits,
    /// base now). Fast path — file unchanged since staging — writes the
    /// precomputed post, byte-identical to the legacy single-track commit.
    /// A diverged base re-applies the unique find/replace (clean hunks
    /// auto-merge); a hunk whose needle vanished or duplicated becomes a
    /// structured conflict report and NOTHING is written (never a silent
    /// clobber). Overlapping hunks (intra-world diverging overlays, or a
    /// moved base that overlaps a previously committed world's hunk) also
    /// conflict — never last-write-wins. The world stays active on conflict
    /// so it can be resolved.
    pub fn commit_world(&mut self, wid: &str) -> Result<(String, Vec<String>, String), String> {
        let world = self
            .worlds
            .active
            .remove(wid)
            .ok_or_else(|| "unknown world".to_string())?;
        let root = self.root.clone();

        // Plan phase: resolve every touched path to (current, merged) without
        // writing a byte. Paths in first-touch order, each replayed through
        // all its staged edits exactly like overlay reads (fszero-1wm).
        let paths = unique_edit_paths(&world.edits);

        // RESCAN GATE (V6-F4 / ZS-STORE-005): before planning any merge,
        // verify each committed path's on-disk content equals the session's
        // last known state (full content hash, not mtime/len). A divergence
        // is an undeclared (external) mutation: the commit is REFUSED with a
        // durable external-effect receipt bound into the world's record --
        // never a silent absorb into a clean auto-merge. A deleted file under
        // a nonempty known state is an external delete (same refusal); an
        // expected-missing world (staged pre == "") keeps an empty known
        // state and passes. Nothing is written on refusal; the world stays
        // active for resolve/rebase or retry.
        let mut external: Vec<ExternalEffectReceipt> = Vec::new();
        for path in &paths {
            let rel = rel_path_for_log(root.as_deref(), path);
            let (known, _) = self.session_known_state(&world.edits, root.as_deref(), &rel);
            let detected_seq = self.next_mutation_seq();
            if crate::path::refuse_non_regular_file(path).is_err() {
                // FIFO/socket/device: metadata is not a hang, content open is.
                // A special node is never the session's known regular bytes.
                if !known.is_empty() {
                    external.push(ExternalEffectReceipt::new(
                        rel,
                        content_hash_bytes(&known),
                        String::new(),
                        detected_seq,
                        ExternalEffectDisposition::Refused,
                    ));
                }
                continue;
            }
            match fs::read(path) {
                Ok(bytes) => {
                    if bytes != known {
                        external.push(ExternalEffectReceipt::new(
                            rel,
                            content_hash_bytes(&known),
                            content_hash_bytes(&bytes),
                            detected_seq,
                            ExternalEffectDisposition::Refused,
                        ));
                    }
                }
                Err(e) => {
                    // External delete: only a genuine NotFound is treated as
                    // a missing file under the world (permission errors stay
                    // on the existing unreadable-conflict path). An empty
                    // known state (expected-missing world) passes.
                    if !known.is_empty() && e.kind() == std::io::ErrorKind::NotFound {
                        external.push(ExternalEffectReceipt::new(
                            rel,
                            content_hash_bytes(&known),
                            String::new(),
                            detected_seq,
                            ExternalEffectDisposition::Refused,
                        ));
                    }
                }
            }
        }
        if !external.is_empty() {
            let n = external.len();
            let cref = self.record_external_effect_receipts(wid, external);
            self.worlds.active.insert(wid.to_string(), world);
            return Err(format!(
                "external edit on committed path(s): files={n} ref={cref}"
            ));
        }

        let mut planned: Vec<(PathBuf, String, String)> = Vec::with_capacity(paths.len());
        let mut conflicts: Vec<serde_json::Value> = Vec::new();
        for path in &paths {
            let rel = rel_path_for_log(root.as_deref(), path);
            let mut missing_ok = false;
            if let Err(e) = crate::path::refuse_non_regular_file(path) {
                conflicts.push(unreadable_conflict(rel, &e));
                continue;
            }
            let current = match fs::read_to_string(path) {
                Ok(c) => c,
                Err(e) => {
                    // Deleted/renamed under the world (delete-vs-edit,
                    // rename-vs-edit): a structured conflict, not a hard
                    // error — nothing gets written, nothing resurrected.
                    // Exception: a resolution (fszero-e8s accept-mine /
                    // supply-merged on a deleted file) staged pre == "",
                    // an explicit expect-missing marker — recreate it.
                    let expects_missing = world
                        .edits
                        .iter()
                        .find(|ed| &ed.path == path)
                        .is_some_and(|ed| ed.pre.is_empty());
                    if expects_missing {
                        missing_ok = true;
                        String::new()
                    } else {
                        conflicts.push(unreadable_conflict(rel, &e));
                        continue;
                    }
                }
            };
            // Path guard: a target must live under the root before we plan
            // a write to it. Missing-but-expected targets use the same
            // create-tolerant validator as fs.write.
            if let Some(root) = root.as_deref() {
                let guard = if missing_ok {
                    validate_rollback_path(root, path).map(|_| ())
                } else {
                    ensure_path_under_root(Some(root), path)
                };
                if let Err(e) = guard {
                    self.worlds.active.insert(wid.to_string(), world);
                    return Err(format!("world path guard: {e}"));
                }
            }
            match overlay_file_content(&world.edits, path, &current, &self.worlds.committed_hunks) {
                Ok(merged) => planned.push((path.clone(), current, merged)),
                Err(reason) => {
                    let first = world
                        .edits
                        .iter()
                        .find(|e| &e.path == path)
                        .expect("path came from edits");
                    let (theirs_ref, base_ref) =
                        self.put_pre_post(current.as_bytes(), first.pre.as_bytes());
                    let ours_ref = self.recovery.put_content_ref(first.post.as_bytes());
                    conflicts.push(serde_json::json!({"file": rel, "reason": reason, "hunk": [first.hunk.0, first.hunk.1], "base_ref": base_ref, "ours_ref": ours_ref, "theirs_ref": theirs_ref}));
                }
            }
        }
        if !conflicts.is_empty() {
            let note = self.world_conflict_note(wid, &conflicts);
            self.worlds.active.insert(wid.to_string(), world);
            return Err(format!("merge conflict {note}"));
        }

        // Write phase: re-check preimage per file (fszero-w2g.11), publish via
        // atomic_write (same as edit), and journal fail-closed. Any failure
        // rolls every earlier path back to its pre-commit bytes (CE-W3).
        self.record_commit_intent(wid, &planned);
        let mut effect_paths: Vec<super::effect_capture::EffectPath> =
            Vec::with_capacity(planned.len());
        for (idx, (path, current, merged)) in planned.iter().enumerate() {
            let pre_meta = file_meta_snapshot(path);
            // Write-phase preimage: refuse to clobber if disk moved since plan.
            if let Err(e) = crate::path::refuse_non_regular_file(path) {
                let rb = self.rollback_world_writes(&planned[..idx], &world, wid);
                return Err(with_rb(format!("stale preimage (unreadable {e})"), rb));
            }
            let live = match fs::read_to_string(path) {
                Ok(c) => c,
                Err(e) if current.is_empty() => {
                    let _ = e;
                    String::new()
                }
                Err(e) => {
                    let rb = self.rollback_world_writes(&planned[..idx], &world, wid);
                    return Err(with_rb(format!("stale preimage (unreadable {e})"), rb));
                }
            };
            if live != *current {
                // V6-F4: receipt the write-phase external edit too (TOCTOU
                // between plan and write) -- the refusal is never silent.
                let rel = rel_path_for_log(root.as_deref(), path);
                let receipt = ExternalEffectReceipt::new(
                    rel.clone(),
                    content_hash_bytes(current.as_bytes()),
                    content_hash_bytes(live.as_bytes()),
                    self.next_mutation_seq(),
                    ExternalEffectDisposition::Refused,
                );
                self.record_external_effect_receipts(wid, vec![receipt]);
                let rb = self.rollback_world_writes(&planned[..idx], &world, wid);
                let msg = format!("stale preimage: {rel} changed between plan and write");
                return Err(with_rb(msg, rb));
            }
            // Write-time TOCTOU guard (V6-F1 / ZS-SEC-001): a parent swapped
            // for a symlink since plan-time must not redirect the publish
            // outside the root.
            if let Some(root_path) = root.as_deref() {
                if let Err(e) = super::path::guard_write_target_parent(root_path, path) {
                    let rb = self.rollback_world_writes(&planned[..idx], &world, wid);
                    return Err(with_rb(format!("world write guard: {e}"), rb));
                }
            }
            if let Err(e) = atomic_write(path, merged.as_bytes()) {
                let rb = self.rollback_world_writes(&planned[..idx], &world, wid);
                return Err(with_rb(e, rb));
            }
            self.refresh_path_after_mutation(path);
            // World commits are repo mutations like edit/write: journal them
            // (pre = the ACTUAL pre-write bytes, so fs.undo restores the
            // merged-over base, not the stale staging preimage).
            let (pre_ref, post_ref) = self.put_pre_post(current.as_bytes(), merged.as_bytes());
            let rel = rel_path_for_log(root.as_deref(), path);
            let world_seq = match self.record_mutation(
                "world",
                &rel,
                &pre_ref,
                &post_ref,
                false,
                pre_meta.0,
                pre_meta.1,
                &pre_meta.2,
            ) {
                Ok(seq) => seq,
                Err(e) => {
                    // Journal failed after publish: roll this file and all earlier
                    // back so we never ack a hole (fszero-w2g.12 / .46).
                    let _ = atomic_write(path, current.as_bytes());
                    self.refresh_path_after_mutation(path);
                    let rb = self.rollback_world_writes(&planned[..idx], &world, wid);
                    return Err(with_rb(super::op_result::journal_err(e), rb));
                }
            };
            // V6-F1 (ZS-STORE-004): collect the uniform per-path effects.
            let file_created = !path.exists();
            effect_paths.push(super::effect_capture::EffectPath {
                path: rel,
                action: if file_created {
                    super::effect_capture::EffectAction::Create
                } else {
                    super::effect_capture::EffectAction::Write
                },
                seq: world_seq,
                pre_ref: pre_ref.clone(),
                post_ref: post_ref.clone(),
            });
            // fszero-k4ur.2: simulated SIGKILL after k successful publishes.
            // No rollback, no registry commit — leaves Partial workspace state.
            maybe_crash_after_world_writes(idx + 1);
        }
        let cert_ref = if world.cert_ref.is_empty() {
            // Forked world (fszero-ap9): cert deferred from fork time.
            let cert = format!("world_fork\nedits={}\n", world.edits.len());
            self.recovery.put_content_ref(cert.as_bytes())
        } else {
            world.cert_ref.clone()
        };
        self.recovery.put_key("last_cert/ref", cert_ref.as_bytes());
        let rel_paths: Vec<String> = planned
            .iter()
            .map(|(path, _, _)| rel_path_for_log(root.as_deref(), path))
            .collect();
        // RACC-R exact-bytes authority on the production commit path (fszero-ip9y /
        // d90z / 09kg): snapshot + AtomicPublication + safepoint + EvidencePage +
        // full SuccessorMap wire. Fail-closed: try_put_key errors propagate.
        if !planned.is_empty() {
            if let Err(e) = self.bind_racc_world_commit(wid, &planned, root.as_deref()) {
                return Err(format!("racc world commit bind: {e}"));
            }
        }
        if self.recovery.is_durable() {
            let _ = self.recovery.set_world_state(wid, "committed");
        }
        self.clear_commit_intent(wid);
        // V6-F1 (ZS-STORE-004): seal the world commit's uniform effect
        // record (scope = this world) and bind it into the op receipt.
        let effects = self.seal_effect_record(
            "world",
            super::effect_capture::EffectScope::World {
                wid: wid.to_string(),
            },
            effect_paths,
            vec![],
        );
        self.worlds
            .committed_hunks
            .extend(world.edits.iter().map(|e| (e.path.clone(), e.hunk)));
        Ok((cert_ref, rel_paths, effects))
    }

    /// Bind RACC-R identity artifacts after multi-file world writes succeed.
    /// Uses content refs (pre/post CAS) as successor keys, publishes candidate
    /// root via AtomicPublication, extracts an evidence page per modified path,
    /// and persists full successor-map wire JSON (not just len).
    fn bind_racc_world_commit(
        &mut self,
        wid: &str,
        planned: &[(PathBuf, String, String)],
        root: Option<&std::path::Path>,
    ) -> Result<(), String> {
        use super::racc::{
            AtomicPublication, CrashPoint, EvidencePage, ExactRange, RefFate, SuccessorMap,
        };

        let base_files: Vec<(String, Vec<u8>)> = planned
            .iter()
            .map(|(path, current, _)| {
                let rel = rel_path_for_log(root, path);
                (rel, current.as_bytes().to_vec())
            })
            .collect();
        let cand_files: Vec<(String, Vec<u8>)> = planned
            .iter()
            .map(|(path, _, merged)| {
                let rel = rel_path_for_log(root, path);
                (rel, merged.as_bytes().to_vec())
            })
            .collect();

        let base_snap = super::racc::snapshot_from_files(base_files)
            .map_err(|e| format!("base snapshot: {e}"))?;
        let cand_snap = super::racc::snapshot_from_files(cand_files)
            .map_err(|e| format!("candidate snapshot: {e}"))?;

        let mut pubn = AtomicPublication::new();
        let published = pubn
            .publish_with_fault(
                base_snap.root_digest_hex(),
                cand_snap.root_digest_hex(),
                CrashPoint::None,
            )
            .map_err(|e| format!("atomic publication: {e}"))?;
        let sp = super::racc::safepoint_for_snapshot(&cand_snap, Some(wid));

        let mut smap = SuccessorMap::new();
        let mut evidence_digests: Vec<String> = Vec::new();
        for (path, current, merged) in planned {
            let rel = rel_path_for_log(root, path);
            if current == merged {
                let same_ref = self.recovery.put_content_ref(current.as_bytes());
                smap.record(RefFate::Unchanged { ref_id: same_ref })
                    .map_err(|e| format!("successor map: {e}"))?;
                continue;
            }
            let (pre_ref, post_ref) = self.put_pre_post(current.as_bytes(), merged.as_bytes());
            smap.record(RefFate::Modified {
                from: pre_ref,
                to: post_ref,
            })
            .map_err(|e| format!("successor map: {e}"))?;
            if !merged.is_empty() {
                let range = ExactRange::new(0, merged.len() as u64)
                    .map_err(|e| format!("evidence range: {e}"))?;
                let page = EvidencePage::extract(
                    cand_snap.root_digest_hex(),
                    &rel,
                    merged.as_bytes(),
                    range,
                )
                .map_err(|e| format!("evidence page: {e}"))?;
                page.verify_against_source(cand_snap.root_digest_hex(), merged.as_bytes())
                    .map_err(|e| format!("evidence verify: {e}"))?;
                evidence_digests.push(format!("{rel}:{}", page.range_digest_hex));
            }
        }

        self.recovery
            .try_put_key(
                "last_world_snapshot/digest",
                cand_snap.root_digest_hex().as_bytes(),
            )
            .map_err(|e| format!("snapshot digest: {e}"))?;
        self.recovery
            .try_put_key("last_world_safepoint/id", sp.safepoint_id.as_bytes())
            .map_err(|e| format!("safepoint id: {e}"))?;
        self.recovery
            .try_put_key("last_world_successor_map", smap.to_wire_json().as_bytes())
            .map_err(|e| format!("successor map wire: {e}"))?;
        self.recovery
            .try_put_key("last_world_publication/root", published.as_bytes())
            .map_err(|e| format!("publication root: {e}"))?;
        if !evidence_digests.is_empty() {
            self.recovery
                .try_put_key(
                    "last_world_evidence/range_digests",
                    evidence_digests.join("\n").as_bytes(),
                )
                .map_err(|e| format!("evidence digests: {e}"))?;
        }
        Ok(())
    }

    /// Compensating multi-file rollback: restore pre-commit bytes for paths
    /// already published in this commit attempt and re-insert the world so
    /// agents can retry (fszero-w2g.22 / CE-W3). Returns rollback error detail
    /// when any restore fails (never silent).
    fn rollback_world_writes(
        &mut self,
        applied: &[(PathBuf, String, String)],
        world: &WorldEdit,
        wid: &str,
    ) -> Option<String> {
        let mut rollback_errors = Vec::new();
        for (applied_path, applied_current, _) in applied.iter().rev() {
            if applied_current.is_empty() {
                if applied_path.exists() {
                    if let Err(e) = fs::remove_file(applied_path) {
                        rollback_errors.push(format!("{}: {e}", applied_path.display()));
                    }
                }
            } else if let Err(e) = atomic_write(applied_path, applied_current.as_bytes()) {
                rollback_errors.push(format!("{}: {e}", applied_path.display()));
            }
            self.refresh_path_after_mutation(applied_path);
        }
        self.worlds.active.insert(wid.to_string(), world.clone());
        let _ = self.persist_active_world(wid);
        self.clear_commit_intent(wid);
        if rollback_errors.is_empty() {
            None
        } else {
            Some(rollback_errors.join(", "))
        }
    }

    /// Export a just-committed world as a real git commit (take from the
    /// artifact-fs/Mesa/TigerFS review: "snapshotting the filesystem becomes
    /// a commit in the branch"). Stages exactly the world's files on the
    /// CURRENT branch. Never touches branch state beyond one commit.
    fn git_commit_world(&self, wid: &str, rel_paths: &[String]) -> Result<String, String> {
        let root = self.root.as_deref().ok_or("no root")?;
        let run = |args: &[&str]| -> Result<String, String> {
            crate::runtime_metrics::record_process_start();
            let out = Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr)
                    .trim()
                    .chars()
                    .take(200)
                    .collect());
            }
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        };
        let mut add_args = vec!["add", "--"];
        add_args.extend(rel_paths.iter().map(String::as_str));
        run(&add_args)?;
        let msg = format!("fszero: world {wid} commit ({} file(s))", rel_paths.len());
        // Pathspec-scoped commit: commits ONLY the world's files even when
        // the user has other work staged.
        let mut commit_args = vec!["commit", "-m", msg.as_str(), "--"];
        commit_args.extend(rel_paths.iter().map(String::as_str));
        run(&commit_args)?;
        run(&["rev-parse", "--short", "HEAD"])
    }

    pub fn do_world(&mut self, arg: Option<&str>) -> String {
        let spec = arg.unwrap_or("");
        if world_arg_creates(spec)
            && env_usize("FSZERO_BUDGET_WORLDS").is_some_and(|cap| self.worlds.active.len() >= cap)
        {
            let cap = env_usize("FSZERO_BUDGET_WORLDS").unwrap_or(0);
            self.store_budget_evidence("W", "worlds", cap, self.worlds.active.len() + 1);
            return format!(
                "budget:0 worlds cap={cap} attempted={}",
                self.worlds.active.len() + 1
            );
        }
        if spec == "fork" {
            let wid = self.fork_world();
            return format!("world:1 {wid}");
        }
        if let Some(rest) = spec.strip_prefix("edit:") {
            let Some((wid, edit_spec)) = rest.split_once(':') else {
                return world0("invalid edit; expected edit:<world>:<path>:<find>|<replace>");
            };
            let wid = wid.to_string();
            return match self.stage_world_edit(&wid, edit_spec) {
                Ok(hunk) => format!(
                    "world:1 edit:{wid} hunk={}-{}{}",
                    hunk.0,
                    hunk.1,
                    self.conflict_suffix(&wid)
                ),
                Err(e) => world0(e),
            };
        }
        if let Some(wid) = spec.strip_prefix("conflicts:") {
            if !self.worlds.active.contains_key(wid) {
                return unknown_world();
            }
            let root = self.root.clone();
            let conflicts = self.world_conflicts(wid);
            let rows: Vec<serde_json::Value> = conflicts
                .iter()
                .map(|(other, path, ours, theirs)| {
                    serde_json::json!({
                        "with": other, "file": rel_path_for_log(root.as_deref(), path),
                        "ours": [ours.0, ours.1], "theirs": [theirs.0, theirs.1],
                    })
                })
                .collect();
            let cref = self.put_world_conflict_payload(wid, "conflicts", &rows);
            return format!("world:1 conflicts:{wid} n={} ref={cref}", conflicts.len());
        }
        if let Some(rest) = spec.strip_prefix("view:") {
            return self.do_world_view(rest);
        }
        if let Some(edit_spec) = spec.strip_prefix("newbatch:") {
            let r = self.create_world_from_batch(edit_spec);
            return world1_created(self, r);
        }
        if let Some(edit_spec) = spec.strip_prefix("new:") {
            let r = self.create_world_from_edit(edit_spec);
            return world1_created(self, r);
        }
        if let Some(rest) = spec.strip_prefix("commit:") {
            let (wid, git_export) = match rest.strip_suffix(":git") {
                Some(w) => (w, true),
                None => (rest, false),
            };
            return match self.commit_world(wid) {
                Ok((cert, rel_paths, effects)) => {
                    let mut e_tail = String::new();
                    if !effects.is_empty() {
                        e_tail.push(' ');
                        e_tail.push_str(&effects);
                    }
                    if git_export {
                        // Filesystem commit already applied; the git step is
                        // additive and fail-open (reported, never rolls back).
                        match self.git_commit_world(wid, &rel_paths) {
                            Ok(sha) => {
                                format!("world:1 commit:{wid} cert:{cert} git:{sha}{e_tail}")
                            }
                            Err(e) => {
                                format!("world:1 commit:{wid} cert:{cert} git:0 ({e}){e_tail}")
                            }
                        }
                    } else {
                        format!("world:1 commit:{wid} cert:{cert}{e_tail}")
                    }
                }
                Err(e) => world0(e),
            };
        }
        if let Some(rest) = spec.strip_prefix("resolve:") {
            return self.do_world_resolve(rest);
        }
        if let Some(wid) = spec.strip_prefix("preview:") {
            return self.do_world_preview(wid);
        }
        if let Some(wid) = spec.strip_prefix("rebase:") {
            return self.do_world_rebase(wid);
        }
        if let Some(wid) = spec.strip_prefix("drop:") {
            if mark_world_dropped(self, wid) {
                return format!("world:1 drop:{wid}");
            }
            return unknown_world();
        }
        world0(
            "unknown action; expected fork, new:, newbatch:, edit:, conflicts:, view:, commit:, resolve:, preview:, rebase:, or drop:",
        )
    }

    /// Conflict resolution API (fszero-e8s), the agent-facing response to a
    /// commit_world conflict report. Contract (docs/design/world-ref.md
    /// sibling; stable):
    ///   resolve:<wid>:abort                  — drop the world (alias of drop:)
    ///   resolve:<wid>:<path>:mine            — my staged content wins: the
    ///       file's edits collapse to one whose preimage is the CURRENT base,
    ///       so the next commit fast-paths my intended bytes over it.
    ///   resolve:<wid>:<path>:theirs          — the moved base wins: my edits
    ///       for that file are withdrawn from the world.
    ///   resolve:<wid>:<path>:merged:<text>   — supply the merged content
    ///       verbatim (everything after `merged:` — no grammar, may contain
    ///       any bytes); preimage set to the current base.
    /// Re-commit after resolving each conflicted file; unresolved files
    /// still conflict (nothing is ever silently clobbered).
    fn do_world_resolve(&mut self, rest: &str) -> String {
        let Some((wid, action)) = rest.split_once(':') else {
            return world0(
                "invalid resolve; expected resolve:<world>:<path>:mine|theirs|merged:<text>",
            );
        };
        if action == "abort" {
            if mark_world_dropped(self, wid) {
                return format!("world:1 resolve:{wid} abort");
            }
            return unknown_world();
        }
        let Some((path_arg, mode)) = action.split_once(':') else {
            return world0(
                "invalid resolve; expected resolve:<world>:<path>:mine|theirs|merged:<text>",
            );
        };
        if !self.worlds.active.contains_key(wid) {
            return unknown_world();
        }
        let root = self.root.clone();
        let target = match self.resolve_existing_path_cached(root.as_deref(), path_arg) {
            Ok(p) => p,
            Err(e) => {
                // Deleted-under-world targets (delete-vs-edit) can't resolve
                // via the existing-path cache; fall back to a root-joined
                // path so `mine`/`merged` can recreate the file.
                let Some(root) = root.as_deref() else {
                    return world0(e);
                };
                // Canonical base so the recreated path passes the same
                // root guard the staged (canonical) paths pass.
                fs::canonicalize(root)
                    .unwrap_or_else(|_| root.to_path_buf())
                    .join(path_arg.trim_start_matches('/'))
            }
        };
        if let Err(e) = crate::path::refuse_non_regular_file(&target) {
            return world0(e);
        }
        let current = fs::read_to_string(&target).unwrap_or_default();
        let rel = rel_path_for_log(root.as_deref(), &target);
        // V6-F4 (ZS-STORE-005): resolve:mine/merged declares the current disk
        // base as the world's base. If that base differs from the session's
        // known state, the absorb is receipted (declared_absorb) -- never a
        // silent absorb. Evaluated against the PRE-resolve edits (theirs
        // withdraws edits and absorbs nothing).
        let absorb_receipt = if mode == "theirs" {
            None
        } else {
            let world = self.worlds.active.get(wid).expect("checked above");
            self.declared_absorb_receipt(&world.edits, root.as_deref(), &rel, &current)
        };
        // Match staged edits by repo-relative key, not raw PathBuf equality:
        // a deleted target resolves through the uncanonicalized fallback
        // while staging stored the canonical path.
        let root_for_match = root.clone();
        let rel_key = rel.clone();
        let matches_target =
            move |p: &Path| rel_path_for_log(root_for_match.as_deref(), p) == rel_key;
        let world = self.worlds.active.get_mut(wid).expect("checked above");
        let had_edits = world.edits.iter().any(|e| matches_target(&e.path));
        if !had_edits {
            return world0(format!("no staged edits for {rel}"));
        }
        if mode == "theirs" {
            world.edits.retain(|e| !matches_target(&e.path));
            if let Err(e) = self.persist_active_world(wid) {
                return world_persist0(e);
            }
            return format!("world:1 resolve:{wid} {rel} theirs");
        }
        let resolved_post = if mode == "mine" {
            // Replay my staged intent over ITS OWN preimage — the world's
            // final would-be content for this file, taken wholesale.
            let mut edits = world.edits.clone();
            // Overlay matches by exact path: rebase matched edits onto the
            // resolved target before replaying.
            for e in &mut edits {
                if matches_target(&e.path) {
                    e.path = target.clone();
                }
            }
            let mine = edits.iter().find(|e| e.path == target).expect("had_edits");
            let pre = mine.pre.clone();
            match overlay_file_content(&edits, &target, &pre, &self.worlds.committed_hunks) {
                Ok(post) => post,
                Err(e) => return world0(format!("overlay: {e}")),
            }
        } else if let Some(text) = mode.strip_prefix("merged:") {
            text.to_string()
        } else {
            return world0("invalid resolve mode; expected mine, theirs, or merged:<text>");
        };
        let (_pre_ref, _post_ref, cert_ref) = self.mint_pre_post_cert(
            &format!("world_resolve\nmode={mode}\n"),
            current.as_bytes(),
            resolved_post.as_bytes(),
        );
        let world = self.worlds.active.get_mut(wid).expect("checked above");
        world.edits.retain(|e| !matches_target(&e.path));
        world.edits.push(WorldFileEdit {
            path: target,
            hunk: (1, resolved_post.matches('\n').count().max(1) as u32),
            pre: current,
            post: resolved_post,
            cert_ref,
            old: String::new(),
            new: String::new(),
        });
        if let Some(receipt) = absorb_receipt {
            self.record_external_effect_receipts(wid, vec![receipt]);
        }
        if let Err(e) = self.persist_active_world(wid) {
            return world_persist0(e);
        }
        format!(
            "world:1 resolve:{wid} {rel} {}",
            if mode == "mine" { "mine" } else { "merged" }
        )
    }

    /// Explicit rebase (fszero-wk9, design section 4.2): re-baseline every
    /// staged edit against the CURRENT tree without writing a byte — the
    /// commit-time three-way logic run early, its result adopted as the new
    /// staging state. Clean files collapse to (pre = current base, post =
    /// merged would-be content); any conflicting file leaves the WHOLE world
    /// unchanged and returns the structured conflict report instead (rebase
    /// is atomic: partial rebases would silently mix baselines).
    fn do_world_rebase(&mut self, wid: &str) -> String {
        let root = self.root.clone();
        let Some(world) = self.worlds.active.get(wid) else {
            return unknown_world();
        };
        let edits = world.edits.clone();
        let paths = unique_edit_paths(&edits);
        let mut rebased: Vec<WorldFileEdit> = Vec::new();
        let mut conflicts: Vec<serde_json::Value> = Vec::new();
        let mut moved = 0usize;
        // V6-F4 (ZS-STORE-005): rebase re-baselines the world onto the
        // current tree; a base that differs from the session's known state is
        // a declared external absorb -- receipted. Recorded only if the
        // rebase is adopted (atomic: a conflict aborts the whole rebase).
        let mut absorb_receipts: Vec<ExternalEffectReceipt> = Vec::new();
        for path in &paths {
            let rel = rel_path_for_log(root.as_deref(), path);
            if let Err(e) = crate::path::refuse_non_regular_file(path) {
                conflicts.push(unreadable_conflict(rel, &e));
                continue;
            }
            let current = match fs::read_to_string(path) {
                Ok(c) => c,
                Err(e) => {
                    conflicts.push(unreadable_conflict(rel, &e));
                    continue;
                }
            };
            let first = edits.iter().find(|e| &e.path == path).expect("from edits");
            if let Some(receipt) =
                self.declared_absorb_receipt(&edits, root.as_deref(), &rel, &current)
            {
                absorb_receipts.push(receipt);
            }
            match overlay_file_content(&edits, path, &current, &self.worlds.committed_hunks) {
                Ok(merged) => {
                    if current != first.pre {
                        moved += 1;
                    }
                    let hunk = if merged == current {
                        first.hunk
                    } else {
                        (1, merged.matches('\n').count().max(1) as u32)
                    };
                    let (_pre_ref, _post_ref, cert_ref) = self.mint_pre_post_cert(
                        "world_rebase\n",
                        current.as_bytes(),
                        merged.as_bytes(),
                    );
                    rebased.push(WorldFileEdit {
                        path: path.clone(),
                        pre: current,
                        post: merged,
                        cert_ref,
                        old: first.old.clone(),
                        new: first.new.clone(),
                        hunk,
                    });
                }
                Err(reason) => {
                    conflicts.push(serde_json::json!({"file": rel, "reason": reason, "hunk": [first.hunk.0, first.hunk.1]}));
                }
            }
        }
        if !conflicts.is_empty() {
            let note = self.world_conflict_note(wid, &conflicts);
            return world0(format!("rebase conflict {note}"));
        }
        let world = self.worlds.active.get_mut(wid).expect("checked above");
        world.edits = rebased;
        if let Err(e) = self.persist_active_world(wid) {
            return world_persist0(e);
        }
        if !absorb_receipts.is_empty() {
            self.record_external_effect_receipts(wid, absorb_receipts);
        }
        format!("world:1 rebase:{wid} files={} moved={moved}", paths.len())
    }

    /// Full-tree preview (fszero-otm): the complete would-be tree exactly
    /// as the FS would look post-commit, before any byte lands on disk —
    /// every tracked path with metadata, plus post hash/ref for files the
    /// world changes. Unchanged entries carry base metadata only (hashing
    /// the whole base per preview would defeat zero-materialization); the
    /// changed set is byte-exact via the same overlay commit uses. Walk
    /// scope and caps match the index walker, so the preview is the tree
    /// the index (and graphzero) reasons about.
    fn do_world_preview(&mut self, wid: &str) -> String {
        let root = self.root.clone();
        let Some(root_path) = root.as_deref() else {
            return world0("no root");
        };
        let Some(world) = self.worlds.active.get(wid) else {
            return unknown_world();
        };
        let edits = world.edits.clone();
        let mut changed: std::collections::HashMap<String, serde_json::Value> =
            std::collections::HashMap::new();
        let paths = unique_edit_paths(&edits);
        for path in &paths {
            let rel = rel_path_for_log(root.as_deref(), path);
            if let Err(e) = crate::path::refuse_non_regular_file(path) {
                changed.insert(
                    rel,
                    serde_json::json!({"status": "unreadable", "detail": e}),
                );
                continue;
            }
            let entry = match fs::read_to_string(path) {
                Err(e) => serde_json::json!({"status": "unreadable", "detail": e.to_string()}),
                Ok(current) => {
                    match overlay_file_content(&edits, path, &current, &self.worlds.committed_hunks)
                    {
                        Ok(post) => {
                            let post_ref = self.recovery.put_content_ref(post.as_bytes());
                            serde_json::json!({"status": "changed", "post_size": post.len(), "post_hash": content_hash_bytes(post.as_bytes()), "post_ref": post_ref})
                        }
                        Err(reason) => {
                            serde_json::json!({"status": "conflict", "detail": reason})
                        }
                    }
                }
            };
            changed.insert(rel, entry);
        }
        let mut files: Vec<serde_json::Value> = Vec::new();
        let mut n_changed = 0usize;
        for (path, meta) in super::ast::walk::walk_rs_files(root_path) {
            let rel = rel_path_for_log(root.as_deref(), &path);
            let mtime_ns = mtime_ns_of(&path);
            let mut entry =
                serde_json::json!({"file": rel, "size": meta.len(), "mtime_ns": mtime_ns});
            if let Some(delta) = changed.remove(&rel) {
                n_changed += 1;
                for (k, v) in delta.as_object().into_iter().flatten() {
                    entry[k] = v.clone();
                }
            }
            files.push(entry);
        }
        // Changed paths the walker missed (deleted under the world).
        for (rel, delta) in changed {
            n_changed += 1;
            let mut entry = serde_json::json!({"file": rel});
            for (k, v) in delta.as_object().into_iter().flatten() {
                entry[k] = v.clone();
            }
            files.push(entry);
        }
        let n_files = files.len();
        let payload = world_v1_payload(wid, files, Some((n_files, n_changed)));
        let cref = self.put_world_key(wid, "preview", payload.as_bytes());
        format!("world:1 preview:{wid} v=1 files={n_files} changed={n_changed} ref={cref}")
    }

    /// Overlay reads (fszero-1wm): `view:<wid>` enumerates the world's
    /// changed files; `view:<wid>:<path>` serves the file exactly as the
    /// tree would look post-commit — journal replayed over the current
    /// base, zero disk writes. Byte-identical to what commit_world would
    /// materialize (same overlay_file_content).
    fn do_world_view(&mut self, rest: &str) -> String {
        let root = self.root.clone();
        let (wid, path_arg) = match rest.split_once(':') {
            Some((wid, path_arg)) => (wid, Some(path_arg)),
            None => (rest, None),
        };
        let Some(world) = self.worlds.active.get(wid) else {
            return unknown_world();
        };
        let Some(path_arg) = path_arg else {
            // World-ref overlay enumeration v1 (fszero-cbt): the stable,
            // versioned contract graphzero's speculative blast consumes —
            // every changed file with hunk spans, base (current disk) and
            // would-be post content hashes/refs, computed with ZERO
            // materialization. Post blobs are persisted content-addressed
            // so a consumer can fetch would-be bytes by fz://blob ref.
            // Documented in docs/design/world-ref.md; bump `version` on any
            // breaking shape change.
            let edits = world.edits.clone();
            let paths = unique_edit_paths(&edits);
            let mut files: Vec<serde_json::Value> = Vec::new();
            for path in &paths {
                let hunks: Vec<[u32; 2]> = edits
                    .iter()
                    .filter(|x| &x.path == path)
                    .map(|x| [x.hunk.0, x.hunk.1])
                    .collect();
                let rel = rel_path_for_log(root.as_deref(), path);
                let entry = match crate::path::refuse_non_regular_file(path)
                    .and_then(|_| fs::read_to_string(path).map_err(|e| e.to_string()))
                {
                    Err(e) => {
                        serde_json::json!({"file": rel, "hunks": hunks, "status": "unreadable", "detail": e})
                    }
                    Ok(current) => match overlay_file_content(
                        &edits,
                        path,
                        &current,
                        &self.worlds.committed_hunks,
                    ) {
                        Ok(post) => {
                            let (base_ref, post_ref) =
                                self.put_pre_post(current.as_bytes(), post.as_bytes());
                            serde_json::json!({"file": rel, "hunks": hunks, "status": "clean", "base_hash": content_hash_bytes(current.as_bytes()), "post_hash": content_hash_bytes(post.as_bytes()), "base_ref": base_ref, "post_ref": post_ref})
                        }
                        Err(reason) => {
                            serde_json::json!({"file": rel, "hunks": hunks, "status": "conflict", "detail": reason})
                        }
                    },
                };
                files.push(entry);
            }
            let n_files = files.len();
            let payload = world_v1_payload(wid, files, None);
            let cref = self.put_world_key(wid, "view", payload.as_bytes());
            return format!("world:1 view:{wid} v=1 files={n_files} ref={cref}");
        };
        let edits = world.edits.clone();
        let target = match self.resolve_existing_path_cached(root.as_deref(), path_arg) {
            Ok(p) => p,
            Err(e) => return world0(e),
        };
        if let Err(e) = crate::path::refuse_non_regular_file(&target) {
            return world0(e);
        }
        let current = match fs::read_to_string(&target) {
            Ok(c) => c,
            Err(e) => return world0(e),
        };
        match overlay_file_content(&edits, &target, &current, &self.worlds.committed_hunks) {
            Ok(view) => {
                let cref = self.put_world_key(wid, "view", view.as_bytes());
                format!("world:1 view:{wid} ref={cref} bytes={}", view.len())
            }
            Err(e) => world0(format!("overlay conflict: {e}")),
        }
    }
}

pub fn cert_field<'a>(cert: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    cert.lines()
        .find_map(|line| line.strip_prefix(&prefix).map(str::trim))
}

fn content_ref_matches(r: &str, data: &[u8]) -> bool {
    r.rsplit('/')
        .next()
        .is_some_and(|expected| content_hash_bytes(data) == expected)
}
