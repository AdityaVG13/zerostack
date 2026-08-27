use crate::*;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct PersistResult {
    pub(crate) refs_complete: bool,
    pub(crate) error: Option<String>,
}

pub(crate) enum RecoveryStoreLease<'a> {
    Shared {
        store: RecoveryStore,
        slot: &'a Mutex<Option<RecoveryStore>>,
    },
    Owned(RecoveryStore),
}

impl std::ops::Deref for RecoveryStoreLease<'_> {
    type Target = RecoveryStore;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Shared { store, .. } | Self::Owned(store) => store,
        }
    }
}

impl std::ops::DerefMut for RecoveryStoreLease<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Shared { store, .. } | Self::Owned(store) => store,
        }
    }
}

impl Drop for RecoveryStoreLease<'_> {
    fn drop(&mut self) {
        let Self::Shared { store, slot } = self else {
            return;
        };
        let mut available = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if available.is_none() {
            let placeholder = RecoveryStore::new(None);
            *available = Some(std::mem::replace(store, placeholder));
        }
    }
}

impl TokenZeroEngine {
    /// Check out the reusable long-lived store, or construct a temporary store
    /// when another request already has it. One-shot CLI commands own their store.
    pub(crate) fn recovery_store(&self) -> RecoveryStoreLease<'_> {
        match &self.recovery_store {
            Some(slot) => {
                let taken = {
                    let mut available =
                        slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    available.take()
                };
                // SAFETY: `recovery_store` is a checkout occupancy lock, not
                // the persist gate. `RecoveryStore::new(Some(path))` reads
                // snapshot+journal. The previous `lock().take().unwrap_or_else(new)`
                // kept the guard alive for the whole statement, so a miss-path
                // load stalled lease Drop (put-back) and other checkouts.
                // Sibling of session cold-load-off-mutex: copy the occupancy
                // result out, drop, then construct.
                let store = taken
                    .unwrap_or_else(|| RecoveryStore::new(Some(self.config.cache_path.clone())));
                RecoveryStoreLease::Shared { store, slot }
            }
            None => {
                RecoveryStoreLease::Owned(RecoveryStore::new(Some(self.config.cache_path.clone())))
            }
        }
    }

    pub(crate) fn shell_output_policy(&self) -> RunOutputPolicy {
        RunOutputPolicy {
            per_stream_capture_bytes: self.config.shell_capture_bytes,
            spill_threshold_bytes: self.config.shell_spill_bytes,
            spill_dir: Some(shell_spill_dir(&self.config.cache_path)),
        }
        .normalized()
    }
}

pub fn inner_env() -> BTreeMap<String, String> {
    BTreeMap::from([("TOKENZERO_INNER".to_string(), "1".to_string())])
}

pub fn persist_refs(
    store: &mut RecoveryStore,
    refs: &mut Vec<tokenzero_core::RefRecord>,
) -> PersistResult {
    let error = (!refs.is_empty())
        .then(|| store.persist_pending())
        .transpose()
        .err()
        .map(|err| err.to_string());
    if error.is_some() {
        refs.clear();
    }
    PersistResult {
        refs_complete: error.is_none() && prune_dead_refs(store, refs),
        error,
    }
}

pub fn push_payload_refs(
    refs: &mut Vec<tokenzero_core::RefRecord>,
    stored: &StoredPayload,
    bytes: usize,
) {
    refs.push(ref_record("blob", stored.blob_ref.clone(), bytes));
    refs.push(ref_record("file", stored.file_ref.clone(), bytes));
}

impl TokenZeroEngine {
    /// Rewrite full-hash blob refs in a tool response to durable ordinal
    /// aliases and persist the ordinal-to-full mapping before emission.
    pub fn apply_session_visible_ref_aliases(&self, response: &mut ToolResponse) {
        self.apply_session_visible_ref_aliases_with_meter(response, count_tokens);
    }

    /// Same as [`Self::apply_session_visible_ref_aliases`], but the complete
    /// full-ref rewrite is published only when it is strictly cheaper under
    /// `meter`; production passes the real rendering gauge (`count_tokens`).
    /// Tests inject a deterministic meter so the accepted path is reachable
    /// without depending on the default lexical gauge (1glt). Accounting and
    /// repeated path/symbol token counts always use the real `count_tokens`.
    fn apply_session_visible_ref_aliases_with_meter(
        &self,
        response: &mut ToolResponse,
        meter: impl Fn(&str) -> usize,
    ) {
        let full_refs = response
            .refs
            .iter()
            .filter(|record| {
                tokenzero_recovery::session_visible_blob_alias(&record.ref_id).is_some()
            })
            .map(|record| record.ref_id.clone())
            .collect::<Vec<_>>();
        if full_refs.is_empty() {
            // Scan before leasing. Most responses have no repeated path/symbol
            // atom worth aliasing, and for those the old code still took the
            // recovery-store lease and re-counted tokens over the entire visible
            // text -- pure overhead on every warm read, which measured as a
            // ~30-60% p50 regression on the warm MCP read workload.
            let Some(visible) = response.visible.as_mut() else {
                return;
            };
            if !crate::text_aliases::has_alias_candidates(&visible.text) {
                return;
            }
            let mut store = self.recovery_store();
            let Some(rewritten) = crate::text_aliases::alias_repeated_paths_and_symbols_if_changed(
                &mut store,
                &visible.text,
            ) else {
                return;
            };
            visible.text = rewritten;
            let visible_tokens = count_tokens(&visible.text);
            if let Some(accounting) = response.accounting.as_mut() {
                accounting.visible_tokens = visible_tokens;
            }
            return;
        }
        let mut store = self.recovery_store();
        let Ok(range) = store.reserve_ordinal_range(full_refs.len() as u64) else {
            return;
        };
        let mut aliases = Vec::with_capacity(full_refs.len());
        for (offset, full_ref) in full_refs.iter().enumerate() {
            let Ok(alias) = store.store_ordinal_alias_deferred(range, offset as u64, full_ref)
            else {
                return;
            };
            aliases.push((full_ref.clone(), alias));
        }
        if store.persist_pending().is_err() {
            return;
        }
        // Rewrite the complete response, not only refs/visible/telemetry.
        // `detail_ref`, safety, channels, diagnostics, and future string fields
        // must never retain a duplicate full ref after the public refs changed.
        // Publish the rewrite only when the complete serialized response is
        // strictly cheaper under the same token gauge used for rendering: BPE
        // cost is contextual, so an ordinal that is shorter in isolation can
        // cost more once the surrounding JSON tokens merge (1glt). When the
        // rewrite is not strictly cheaper the response keeps its byte/field
        // semantics; the lossless ordinal mapping stays persisted but is not
        // exposed in any field.
        let Ok(encoded) = serde_json::to_string(response) else {
            return;
        };
        let Some(rewritten_encoded) =
            rewrite_full_refs_if_strictly_cheaper(&encoded, &aliases, meter)
        else {
            return;
        };
        let Ok(rewritten) = serde_json::from_str::<ToolResponse>(&rewritten_encoded) else {
            return;
        };
        *response = rewritten;
        if let Some(visible) = response.visible.as_mut() {
            // Same prefilter as the no-refs branch above: only pay for the
            // path/symbol scan when the text can actually contain an atom.
            if crate::text_aliases::has_alias_candidates(&visible.text)
                && let Some(rewritten) =
                    crate::text_aliases::alias_repeated_paths_and_symbols_if_changed(
                        &mut store,
                        &visible.text,
                    )
            {
                visible.text = rewritten;
            }
        }
        if let Some(accounting) = response.accounting.as_mut() {
            if let Some(visible) = response.visible.as_ref() {
                accounting.visible_tokens = count_tokens(&visible.text);
            }
            accounting.exact_ref_tokens = Some(
                response
                    .refs
                    .iter()
                    .map(|record| count_tokens(&record.ref_id))
                    .sum(),
            );
        }
    }
}

/// Rewrite every `full_ref` occurrence in the complete serialized response
/// only when the rewritten serialization is strictly cheaper under `meter`
/// than the original. BPE token cost is contextual, so candidate-local or
/// lexical length comparisons are not a sound proof of a win: the decision
/// uses the whole serialized form with the same gauge used for rendering.
fn rewrite_full_refs_if_strictly_cheaper(
    encoded: &str,
    aliases: &[(String, String)],
    meter: impl Fn(&str) -> usize,
) -> Option<String> {
    let mut rewritten = encoded.to_owned();
    for (full_ref, alias) in aliases {
        rewritten = rewritten.replace(full_ref, alias);
    }
    (meter(&rewritten) < meter(encoded)).then_some(rewritten)
}

pub fn served_record(content: &str, stored: &StoredPayload) -> ServedRecord {
    served_record_with_metadata(
        sha256_hex(content),
        content.len(),
        content.lines().count(),
        stored,
    )
}

pub(crate) fn served_record_with_metadata(
    content_sha256: String,
    byte_len: usize,
    line_count: usize,
    stored: &StoredPayload,
) -> ServedRecord {
    ServedRecord {
        content_sha256,
        blob_ref: stored.blob_ref.clone(),
        file_ref: stored.file_ref.clone(),
        raw_tokens: stored.raw_tokens,
        line_count,
        byte_len,
        served_at: SystemTime::now(),
        serve_count: 1,
    }
}

pub fn success_response(
    tool: &str,
    mode: Mode,
    text: String,
    refs: Vec<tokenzero_core::RefRecord>,
    accounting: (usize, usize, usize, Option<usize>),
) -> ToolResponse {
    ToolResponse::ok(
        tool,
        mode,
        text,
        refs,
        Accounting::measured(
            accounting.0,
            accounting.1,
            accounting.2,
            accounting.1,
            0,
            accounting.3,
        ),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalPayloadPolicy {
    Inline,
    ExactRef,
}

/// Auto mode keeps local payloads inline through the configured boundary and
/// prefers an exact selector only above it. Explicit modes always win.
pub fn local_payload_policy(
    payload_bytes: usize,
    exact_ref_threshold_bytes: usize,
    mode: Mode,
    exact_ref_available: bool,
) -> LocalPayloadPolicy {
    if mode == Mode::Auto && exact_ref_available && payload_bytes > exact_ref_threshold_bytes {
        LocalPayloadPolicy::ExactRef
    } else {
        LocalPayloadPolicy::Inline
    }
}

/// Auto-mode admission via the horizon-cost estimator (ZS-VIEW-006).
/// Explicit modes and missing exact refs always stay inline; the estimator
/// decides only Auto-mode payloads. Callers must pass labeled per-call or
/// replay-derived expansion probability and horizon -- this function never
/// substitutes `AdmissionEstimator` defaults. The `ByteThreshold` policy
/// never reaches this function -- it routes through `local_payload_policy`
/// so the legacy rule stays byte-identical on the default path.
pub fn local_payload_policy_estimated(
    payload_bytes: usize,
    mode: Mode,
    exact_ref_available: bool,
    estimator: &crate::admission::AdmissionEstimator,
    expansion_probability_milli: u32,
    horizon: u64,
    handling_cost_tokens: u64,
) -> LocalPayloadPolicy {
    if mode != Mode::Auto || !exact_ref_available {
        return LocalPayloadPolicy::Inline;
    }
    let decision = estimator.decide_horizon_cost(
        payload_bytes,
        Some(expansion_probability_milli),
        Some(horizon),
        handling_cost_tokens,
    );
    if decision.admit_exact_ref {
        LocalPayloadPolicy::ExactRef
    } else {
        LocalPayloadPolicy::Inline
    }
}

// Public API (module `render` is exported): the eight arguments map 1:1 to
// distinct capsule/rendering semantics, so a parameter struct would churn the
// published surface without a lint gain. Targeted allow, not a blanket one.
#[allow(clippy::too_many_arguments)]
pub fn recoverable_capsule(
    rendered: &str,
    fallback: &str,
    raw_tokens: usize,
    mode: Mode,
    max_visible_tokens: usize,
    label: &str,
    recovery_ref: Option<&str>,
    refs_complete: bool,
) -> Result<tokenzero_core::Capsule, String> {
    if refs_complete {
        let capsule = tokenzero_core::make_capsule_with_recovery_ref(
            rendered,
            raw_tokens,
            mode,
            max_visible_tokens,
            Some(label),
            recovery_ref,
        )?;
        crate::perf_profile::note_hot_path_capsule();
        Ok(capsule)
    } else {
        Ok(tokenzero_core::Capsule {
            text: fallback.trim_end().to_string(),
            raw_tokens,
            visible_tokens: raw_tokens,
            omitted_lines: 0,
            mode,
            protected_anchors: Vec::new(),
            exact_refs: Vec::new(),
            lossy_spans: Vec::new(),
            lossy_policy_id: None,
        })
    }
}

pub fn cache_write_diagnostic(message: impl Into<String>) -> tokenzero_core::Diagnostic {
    tokenzero_core::Diagnostic {
        code: "cache_write_failed".to_string(),
        message: message.into(),
        repair: Some("fix recovery cache permissions or pass --cache-path".to_string()),
    }
}

pub fn session_persist_diagnostic(err: &std::io::Error) -> tokenzero_core::Diagnostic {
    tokenzero_core::Diagnostic {
        code: "session_persist_failed".to_string(),
        message: format!("session memory persist failed: {err}"),
        repair: Some(
            "fix session-memory directory permissions or TOKENZERO_REF_INDEX_PATH".to_string(),
        ),
    }
}

pub fn session_persist_failure(tool: &str, err: &std::io::Error) -> ToolResponse {
    let diagnostic = session_persist_diagnostic(err);
    failure_response(
        tool,
        &diagnostic.code,
        diagnostic.message.clone(),
        diagnostic.repair.as_deref(),
    )
}

pub fn failure_response(
    tool: &str,
    code: &str,
    message: impl Into<String>,
    repair: Option<&str>,
) -> ToolResponse {
    ToolResponse::error(tool, code, message.into(), repair.map(str::to_string))
}

fn format_allowed_roots(allowed_roots: &[PathBuf]) -> String {
    if allowed_roots.is_empty() {
        return "<none>".to_string();
    }
    allowed_roots
        .iter()
        .map(|root| root.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn suggested_relative_path(path: &Path, allowed_roots: &[PathBuf]) -> String {
    for root in allowed_roots {
        if let Ok(rel) = path.strip_prefix(root) {
            let text = rel.to_string_lossy();
            if text.is_empty() {
                return ".".to_string();
            }
            return text.into_owned();
        }
    }
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty() && name != "." && name != "..")
        .unwrap_or_else(|| ".".to_string())
}

pub(crate) fn path_outside_roots_repair(path: &Path, allowed_roots: &[PathBuf]) -> String {
    let relative = suggested_relative_path(path, allowed_roots);
    format!(
        "use a path relative to the active root (e.g. '{relative}'), or re-root via allowed_roots / CodeMode root param"
    )
}

pub fn path_not_allowed(tool: &str, path: &Path, allowed_roots: &[PathBuf]) -> ToolResponse {
    let roots = format_allowed_roots(allowed_roots);
    let repair = path_outside_roots_repair(path, allowed_roots);
    failure_response(
        tool,
        "path_not_allowed",
        format!(
            "path is outside allowed roots: {}. active root(s): {roots}. {repair}",
            path.display()
        ),
        Some(&repair),
    )
}

pub fn expansion_response(result: ExpansionResult, recovery_tokens: usize) -> ToolResponse {
    if result.found {
        let mut response = success_response(
            "expand",
            Mode::Exact,
            result.content,
            Vec::new(),
            (
                result.tokens,
                result.tokens,
                recovery_tokens,
                Some(count_tokens(&result.ref_id)),
            ),
        );
        if let (Some(start_line), Some(end_line), Some(line_count)) = (
            result.returned_start_line,
            result.returned_end_line,
            result.line_count,
        ) {
            response.telemetry = Some(serde_json::json!({
                "window": {
                    "clamped": result.clamped,
                    "start_line": start_line,
                    "end_line": end_line,
                    "line_count": line_count,
                }
            }));
        }
        return response;
    }
    let full_ref = &result.ref_id;
    let reason = result.reason.as_str();
    let is_window_oob = reason.starts_with("window-out-of-range");
    let exact = [
        (
            "shared-cas-missing",
            "shared_cas_missing",
            "shared CAS object missing",
        ),
        (
            "shared-cas-corruption",
            "shared_cas_corruption",
            "shared CAS object corrupted",
        ),
        (
            "shared-cas-policy",
            "shared_cas_policy",
            "shared CAS policy denied expansion",
        ),
        ("shared-cas-io", "shared_cas_io", "shared CAS I/O failure"),
        (
            "shared-cas-non-utf8",
            "shared_cas_non_utf8",
            "shared CAS object is not UTF-8 text",
        ),
        (
            "unsupported-ref-kind",
            "unsupported_ref_kind",
            "foreign non-blob ref requires its owning engine",
        ),
        ("stale-ref", "ref_stale", "ref is no longer recoverable"),
        (
            "invalid-ref",
            "invalid_ref",
            "ref is not a valid tz://, fz://, or gz:// recovery handle",
        ),
        (
            "decode-failed",
            "expand_failed",
            "ref was found but could not be decoded",
        ),
    ];
    let (code, message) = if reason == "dangling-ref" {
        ("dangling_ref", format!("{reason} (ref: {full_ref})"))
    } else if reason.starts_with("ref-not-found") {
        ("ref_not_found", format!("{reason} (ref: {full_ref})"))
    } else if is_window_oob {
        ("window_out_of_range", format!("{reason} (ref: {full_ref})"))
    } else if reason.starts_with("zeroref-") {
        ("zeroref_malformed", format!("{reason}: {full_ref}"))
    } else if let Some(code) = fragment_error_code(reason) {
        // yevj: invalid fragments fail typed ONCE — the code names the
        // fragment defect so adapters stop instead of retrying a ref that can
        // never resolve. The reason carries the parsed bounds detail.
        (code, format!("{reason} (ref: {full_ref})"))
    } else if let Some((_, code, message)) = exact.iter().find(|entry| entry.0 == reason) {
        (*code, format!("{message}: {full_ref}"))
    } else {
        ("expand_failed", format!("ref expansion failed: {full_ref}"))
    };
    let repair = if is_window_oob {
        "choose start_line/end_line within the stored payload line count (1-based inclusive)"
    } else if fragment_error_code(reason).is_some() {
        "drop the fragment suffix to expand the whole payload, or re-issue it within the stored extents"
    } else if reason == "unsupported-ref-kind" {
        "route the ref to the engine named by its scheme"
    } else {
        "align the producer and consumer shared store root, then retry with the exact ref"
    };
    let mut response = ToolResponse::error("expand", code, message, Some(repair.to_string()));
    response.telemetry = Some(serde_json::json!({
        "expand": {
            "fail_count": 1,
            "dangling_ref_count": u64::from(reason == "dangling-ref"),
            "miss_kind": code,
        }
    }));
    response
}

/// Map a recovery-store fragment failure reason to a stable typed error
/// code. Reasons may carry `; key=value` bounds detail after the kind tag.
fn fragment_error_code(reason: &str) -> Option<&'static str> {
    const FRAGMENT_REASONS: &[(&str, &str)] = &[
        ("fragment-malformed", "fragment_malformed"),
        ("fragment-reversed", "fragment_reversed"),
        ("fragment-out-of-range", "fragment_out_of_range"),
        ("fragment-not-utf8-boundary", "fragment_not_utf8_boundary"),
        ("non_utf8_line_fragment", "fragment_not_utf8_boundary"),
        ("fragment-unknown-kind", "fragment_unknown_kind"),
        ("fragment-duplicate", "fragment_duplicate"),
    ];
    FRAGMENT_REASONS
        .iter()
        .find(|(tag, _)| reason == *tag || reason.starts_with(&format!("{tag};")))
        .map(|(_, code)| *code)
}

pub fn unchanged_since_expand_ack(since_ref: &str) -> String {
    format!("unchanged since {since_ref}")
}

pub fn expand_since_diff_text(since_ref: &str, target_ref: &str, diff_body: &str) -> String {
    format!(
        "# expand {target_ref} — diff since {since_ref}
{diff_body}"
    )
}

pub fn common_content_type(content_types: &[ContentType]) -> ContentType {
    let Some(first) = content_types.first().copied() else {
        return ContentType::Unknown;
    };
    if content_types
        .iter()
        .all(|content_type| *content_type == first)
    {
        first
    } else {
        ContentType::Unknown
    }
}

pub fn exact_ref_token_count(refs: &[tokenzero_core::RefRecord]) -> usize {
    refs.iter().map(|record| count_tokens(&record.ref_id)).sum()
}

/// Re-verify advertised refs after a persist: the persist's cache merge can
/// evict entries under byte/count pressure (including refs stored earlier in
/// the same call), and a response must never advertise a ref that can no
/// longer be expanded. Returns true when every ref survived.
pub fn prune_dead_refs(store: &RecoveryStore, refs: &mut Vec<tokenzero_core::RefRecord>) -> bool {
    let before = refs.len();
    refs.retain(|record| store.has_ref(&record.ref_id));
    refs.len() == before
}

pub struct AppliedEdits {
    pub(crate) text: String,
    pub(crate) diff: String,
    pub(crate) lines_added: usize,
    pub(crate) lines_removed: usize,
}

pub struct EditFailure {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) repair: Option<String>,
}

fn edit_failure(
    code: &'static str,
    message: impl Into<String>,
    repair: &'static str,
) -> Result<AppliedEdits, EditFailure> {
    Err(EditFailure {
        code,
        message: message.into(),
        repair: Some(repair.to_string()),
    })
}

/// Whole-file hunk for `create=true`: `replace` becomes the file content.
pub fn create_file_hunk(hunk: &EditHunk) -> Result<AppliedEdits, EditFailure> {
    if hunk.replace.is_empty() {
        return edit_failure(
            "no_op_hunk",
            "create hunk has an empty replace; nothing to write",
            "pass the full new-file content in replace",
        );
    }
    let mut diff = String::from("@@ hunk 1 @@ line 1");
    for line in hunk.replace.lines() {
        diff.push_str("\n+");
        diff.push_str(line);
    }
    Ok(AppliedEdits {
        text: hunk.replace.clone(),
        diff,
        lines_added: hunk.replace.lines().count(),
        lines_removed: 0,
    })
}

pub fn apply_edit_hunks(original: &str, edits: &[EditHunk]) -> Result<AppliedEdits, EditFailure> {
    let mut text = original.to_string();
    let mut sections = Vec::new();
    let mut lines_added = 0usize;
    let mut lines_removed = 0usize;
    for (index, hunk) in edits.iter().enumerate() {
        if hunk.find.is_empty() {
            return edit_failure(
                "edit_failed",
                format!("edits[{index}] has an empty find; that is only valid with create=true"),
                "pass the exact text to replace in find",
            );
        }
        if hunk.find == hunk.replace {
            return edit_failure(
                "no_op_hunk",
                format!("edits[{index}] replaces text with identical text"),
                "drop the hunk or change replace",
            );
        }
        let offsets: Vec<usize> = text.match_indices(&hunk.find).map(|(at, _)| at).collect();
        if offsets.is_empty() {
            let hint = closest_line_hint(&text, &hunk.find)
                .map(|hint| format!("; {hint}"))
                .unwrap_or_default();
            return edit_failure(
                "hunk_not_found",
                format!("edits[{index}] matched nothing{hint}"),
                "re-read the file and pass the exact current text in find",
            );
        }
        if offsets.len() > 1 && !hunk.replace_all {
            return edit_failure(
                "ambiguous_hunk",
                format!(
                    "edits[{index}] matches {} times; expected exactly one match",
                    offsets.len()
                ),
                "add surrounding context to find or set replace_all=true",
            );
        }
        for (occurrence, &offset) in offsets.iter().enumerate() {
            let label = if offsets.len() > 1 {
                format!("@@ hunk {} occurrence {} @@", index + 1, occurrence + 1)
            } else {
                format!("@@ hunk {} @@", index + 1)
            };
            let (section, added, removed) =
                render_edit_region(&text, offset, &hunk.find, &hunk.replace, &label);
            sections.push(section);
            lines_added += added;
            lines_removed += removed;
        }
        // Apply from the last occurrence backwards so earlier offsets stay
        // valid; offsets were collected non-overlapping left-to-right.
        for &offset in offsets.iter().rev() {
            text.replace_range(offset..offset + hunk.find.len(), &hunk.replace);
        }
    }
    Ok(AppliedEdits {
        text,
        diff: sections.join("\n"),
        lines_added,
        lines_removed,
    })
}

/// Hunk-labelled context-1 before/after rendering of one replacement (a
/// deliberate lightweight projection, not a strict unified diff). Returns the
/// section text plus added/removed line counts.
pub fn render_edit_region(
    text: &str,
    offset: usize,
    find: &str,
    replace: &str,
    label: &str,
) -> (String, usize, usize) {
    let region_start = text[..offset].rfind('\n').map(|at| at + 1).unwrap_or(0);
    let match_end = offset + find.len();
    let region_end = text[match_end..]
        .find('\n')
        .map(|at| match_end + at)
        .unwrap_or(text.len());
    let old_lines: Vec<&str> = text[region_start..region_end].split('\n').collect();
    let new_region = format!(
        "{}{}{}",
        &text[region_start..offset],
        replace,
        &text[match_end..region_end]
    );
    let new_lines: Vec<&str> = new_region.split('\n').collect();
    let mut prefix = 0;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old_lines.len() - prefix
        && suffix < new_lines.len() - prefix
        && old_lines[old_lines.len() - 1 - suffix] == new_lines[new_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let removed = &old_lines[prefix..old_lines.len() - suffix];
    let added = &new_lines[prefix..new_lines.len() - suffix];
    let region_first_line = text[..region_start].matches('\n').count();
    let first_changed_line = region_first_line + prefix;
    let file_lines: Vec<&str> = text.split('\n').collect();
    let mut section = format!("{label} line {}", first_changed_line + 1);
    if first_changed_line > 0
        && let Some(context) = file_lines.get(first_changed_line - 1)
        && !context.is_empty()
    {
        section.push_str(&format!("\n {context}"));
    }
    for line in removed {
        section.push_str(&format!("\n-{line}"));
    }
    for line in added {
        section.push_str(&format!("\n+{line}"));
    }
    if let Some(context) = file_lines.get(first_changed_line + removed.len())
        && !context.is_empty()
    {
        section.push_str(&format!("\n {context}"));
    }
    if removed.is_empty() && added.is_empty() {
        // The replacement only moved a line boundary (e.g. dropped a trailing
        // newline); there is no whole changed line to show.
        section.push_str("\n~ newline-only change");
    }
    (section, added.len(), removed.len())
}

/// Cheap near-miss hint for hunk_not_found: the first file line containing
/// the find's first non-empty line, clamped for the error message.
pub fn closest_line_hint(text: &str, find: &str) -> Option<String> {
    let probe = find.lines().find(|line| !line.trim().is_empty())?.trim();
    let (number, line) = text
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains(probe))?;
    let trimmed = line.trim();
    let shown: String = trimmed.chars().take(80).collect();
    let ellipsis = if trimmed.chars().count() > 80 {
        "…"
    } else {
        ""
    };
    Some(format!("closest line {}: {shown}{ellipsis}", number + 1))
}

/// Sibling temp for [`write_atomic`]. Pid-only `.{file}.tz-edit-{pid}` is the
/// same path for every publisher in this process.
fn unique_edit_tmp(directory: &Path, file_name: &str) -> PathBuf {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
    directory.join(format!(
        ".{file_name}.tz-edit-{}-{nonce}",
        std::process::id()
    ))
}

/// Write via a temp file in the same directory plus rename so a crash or
/// concurrent reader never observes a half-written file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tz-edit".to_string());
    // SAFETY: no persist gate (flock/mutex) covers this publish. T1 and T2
    // concurrent `edit` of the same dest both computed
    // `.{file}.tz-edit-{pid}`; T2's fs::write truncated T1's tmp, then T1
    // renamed and published T2's partial or mixed bytes. Pid+nonce makes
    // each publisher's tmp unique; last rename still wins with a complete
    // file. Kill-mid-rename leftover is a distinct class (orphan tmp).
    let temp_path = unique_edit_tmp(directory, &file_name);
    fs::write(&temp_path, bytes)?;
    match fs::rename(&temp_path, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&temp_path);
            Err(err)
        }
    }
}

pub fn degraded_shell_response(
    command: &str,
    mode: Mode,
    output: &str,
    error: String,
) -> ToolResponse {
    let mut response = ToolResponse::ok(
        "shell",
        Mode::Passthrough,
        output.to_string(),
        Vec::new(),
        Accounting::measured(
            count_tokens(output),
            count_tokens(output),
            0,
            count_tokens(output),
            0,
            Some(0),
        ),
    );
    response.content_type = Some(ContentType::ShellOutput.to_string());
    response.diagnostic = Some(tokenzero_core::Diagnostic {
        code: "cache_write_failed".to_string(),
        message: format!("could not persist exact shell bytes for {command}"),
        repair: Some("rerun after fixing recovery cache permissions".to_string()),
    });
    response.telemetry = Some(json!({
        "command": command,
        "requested_mode": mode.to_string(),
        "transport_status": "degraded",
        "degraded": true,
        "storage_error": error,
        "output_strategy": "exact_passthrough_storage_failed"
    }));
    response
}

/// A dedup/diff substitution computed during the read loop but applied only
/// after the recovery refs it advertises have actually persisted — a note
/// that replaces content with refs is only safe when the refs resolve.
pub enum PendingSubstitution {
    Dedup {
        idx: usize,
        note: String,
        note_tokens: usize,
        full_tokens: usize,
        serve_count: usize,
        cross_session: bool,
    },
    Diff {
        idx: usize,
        text: String,
        diff_tokens: usize,
        full_tokens: usize,
        telemetry: DiffTelemetry,
    },
}

/// Seen-set note for an identical re-read (docs/codemode.md §5a). Both refs
/// are the freshly minted ones for this serve, so the note alone recovers
/// the exact bytes even if the client compacted the earlier payload away.
/// Callers must only emit it after those refs persisted.
pub fn unchanged_read_note(
    path: &Path,
    text: &str,
    stored: &StoredPayload,
    cross_session: bool,
) -> String {
    let when = if cross_session {
        "served in a prior session; bytes match disk"
    } else {
        "served earlier this session"
    };
    format!(
        "unchanged: {} ({when})\n# {} — {} lines, {} tokens; full bytes: expand {}",
        stored.file_ref,
        path.display(),
        text.lines().count(),
        stored.raw_tokens,
        stored.blob_ref
    )
}

/// Seen-set note for identical re-run find/grep output; the echoed query is
/// clamped exactly like zero-hit notes.
pub fn unchanged_search_note(
    tool: &str,
    query: &str,
    output: &str,
    stored: &StoredPayload,
) -> String {
    format!(
        "unchanged: {} (served earlier this session)\n# {tool} {} — {} matches, {} tokens; full results: expand {}",
        stored.file_ref,
        zero_hit_label(query),
        output.lines().count(),
        stored.raw_tokens,
        stored.blob_ref
    )
}

/// Diff-aware re-read (docs/codemode.md §5b): recover the previously served
/// bytes through the existing recovery API, render a unified diff, and
/// return it only when strictly cheaper than the full render. Any miss —
/// pruned base, oversized side, tie or larger diff — returns `None` and the
/// caller serves full. The base expansion is charged as recovery tokens on
/// `store`, keeping recovery-adjusted savings honest.
pub fn diff_since_served(
    store: &mut RecoveryStore,
    path: &Path,
    text: &str,
    previous: &ServedRecord,
    stored: &StoredPayload,
    full_tokens: usize,
) -> Option<(String, usize, DiffTelemetry)> {
    if text.len() > DIFF_MAX_BYTES
        || previous.byte_len > DIFF_MAX_BYTES
        || text.lines().count() > DIFF_MAX_LINES
        || previous.line_count > DIFF_MAX_LINES
    {
        return None;
    }
    // Diff bases are an internal session optimization, not a user recovery
    // request. If the base is no longer DURABLE (external prune removed the
    // cache/ref-index under this live process), fall back to the full render:
    // serving a diff would reference a base the agent cannot expand later.
    // In-memory state alone does not count (bxqo.1 / F-021).
    if !store.has_ref_durable(&previous.blob_ref) {
        return None;
    }
    let base = store.expand(&previous.blob_ref, Some("raw"), None, None, None, None);
    if !base.found {
        return None;
    }
    let render = diff::unified_diff(&base.content, text)?;
    let assembled = format!(
        "# read {} — changed since served this session (diff vs {})\n{}\nfull file: expand {}",
        path.display(),
        previous.blob_ref,
        render.text,
        stored.blob_ref
    );
    let diff_tokens = count_tokens(&assembled);
    if diff_tokens >= full_tokens {
        return None;
    }
    Some((
        assembled,
        diff_tokens,
        DiffTelemetry {
            hunks: render.hunks,
            plus: render.plus,
            minus: render.minus,
            base_ref: previous.blob_ref.clone(),
        },
    ))
}

pub fn pick_cheaper<'a>(flat: &'a str, compact: &'a str) -> (&'a str, bool) {
    if count_tokens(compact) < count_tokens(flat) {
        (compact, true)
    } else {
        (flat, false)
    }
}

pub fn preview(text: &str) -> String {
    const MAX_LINES: usize = 6;
    const MAX_CHARS: usize = 320;

    let lines = text.lines().collect::<Vec<_>>();
    let shown = lines.len().min(MAX_LINES);
    let more = lines.len().saturating_sub(shown);
    let marker = (more > 0).then(|| format!("\n+{more} more lines"));
    let marker_chars = marker.as_deref().map_or(0, |value| value.chars().count());
    let body_chars = MAX_CHARS.saturating_sub(marker_chars);
    let mut value = lines[..shown].join("\n");
    if value.chars().count() > body_chars {
        value = value.chars().take(body_chars).collect();
    }
    if let Some(marker) = marker {
        value.push_str(&marker);
    }
    value
}

pub fn captured_stream_text(text: &str, capture: &StreamCapture, stream_name: &str) -> String {
    if !capture.truncated {
        return text.to_string();
    }
    let mut value = text.to_string();
    if !value.is_empty() && !value.ends_with('\n') {
        value.push('\n');
    }
    value.push_str(&format!(
        "[tokenzero:{stream_name} truncated: captured {} of {} bytes",
        capture.captured_bytes, capture.bytes_seen
    ));
    if let Some(path) = capture.spill_path.as_deref() {
        value.push_str(&format!("; spill_path: {path}"));
    }
    value.push_str("]\n");
    value
}

/// Compatibility opt-out for the default slim CLI ToolResponse envelope.
/// `0`/`off`/`false`/`no` selects the full forensic envelope.
pub const SLIM_ENVELOPE_ENV: &str = "TOKENZERO_SLIM_ENVELOPE";

static FULL_CLI_ENVELOPE_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Select the full forensic envelope for this CLI process (`--json=full`).
pub fn request_full_cli_envelope() {
    FULL_CLI_ENVELOPE_REQUESTED.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn slim_envelope_enabled() -> bool {
    if FULL_CLI_ENVELOPE_REQUESTED.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    std::env::var(SLIM_ENVELOPE_ENV)
        .map(|raw| {
            !matches!(
                raw.trim().to_ascii_lowercase().as_str(),
                "0" | "off" | "false" | "no"
            )
        })
        .unwrap_or(true)
}

pub fn cli_json(response: &ToolResponse) -> String {
    if slim_envelope_enabled() {
        return slim_cli_json(response);
    }
    serde_json::to_string_pretty(response).unwrap_or_else(|_| {
        format!("{{\"schema_version\":\"{CLI_SCHEMA_VERSION}\",\"status\":\"error\"}}")
    })
}

/// Slim projection: keeps the stable minimum envelope, payload, and every
/// durable ref as a bare string. `--json=full` restores advisory accounting,
/// telemetry, mode, and content-type blocks. Deterministic for the same input.
fn slim_cli_json(response: &ToolResponse) -> String {
    let mut doc = serde_json::Map::new();
    doc.insert(
        "schema_version".into(),
        serde_json::json!(response.schema_version),
    );
    doc.insert("status".into(), serde_json::json!(response.status));
    doc.insert("tool".into(), serde_json::json!(response.tool));
    if let Some(ack) = &response.ack {
        doc.insert("ack".into(), serde_json::json!(ack));
    }
    if let Some(visible) = &response.visible {
        // The capsule wrapper ({kind,text}) costs ~28B per call and "capsule"
        // is the only kind the CLI ever emits, so slim carries the bare text.
        doc.insert("visible".into(), serde_json::json!(visible.text));
    }
    if !response.refs.is_empty() {
        // A complete single-file read carries both an immutable blob ref and a
        // live file selector for the same bytes. The blob already provides
        // exact restart-safe recovery, so repeating the file selector in the
        // slim envelope adds no recovery capability. Keep both in the full
        // forensic envelope and whenever the visible payload is incomplete.
        let visible_bytes = response.visible.as_ref().map(|visible| visible.text.len());
        let redundant_complete_file_ref = response.tool == "read"
            && response.status == "ok"
            && response.refs.len() == 2
            && visible_bytes.is_some_and(|bytes| {
                // Text rendering may remove one terminal newline. The blob
                // remains the exact byte source in either representation.
                let exact_or_one_terminal_newline = |record_bytes: usize| {
                    record_bytes == bytes || record_bytes == bytes.saturating_add(1)
                };
                response.refs.iter().any(|record| {
                    record.kind == "blob" && exact_or_one_terminal_newline(record.bytes)
                }) && response.refs.iter().any(|record| {
                    record.kind == "file" && exact_or_one_terminal_newline(record.bytes)
                })
            });
        doc.insert(
            "refs".into(),
            serde_json::json!(
                response
                    .refs
                    .iter()
                    .filter(|record| !(redundant_complete_file_ref && record.kind == "file"))
                    .map(|record| record.ref_id.as_str())
                    .collect::<Vec<_>>()
            ),
        );
    }
    // detail_ref is defined as refs.first() (tokenzero-core ToolResponse::new),
    // so restating it costs a full 74B ref for zero information. Emit it only
    // when it is not already recoverable from the refs array.
    if let Some(detail_ref) = &response.detail_ref
        && !response
            .refs
            .iter()
            .any(|record| record.ref_id == *detail_ref)
    {
        doc.insert("detail_ref".into(), serde_json::json!(detail_ref));
    }
    if let Some(error) = &response.error {
        doc.insert(
            "error".into(),
            serde_json::to_value(error).unwrap_or(serde_json::Value::Null),
        );
        if let Some(diagnostic) = &response.diagnostic {
            doc.insert(
                "diagnostic".into(),
                serde_json::to_value(diagnostic).unwrap_or(serde_json::Value::Null),
            );
        }
    }
    if let Some(safety) = &response.safety {
        doc.insert("safety".into(), safety.clone());
    }
    if let Some(recovery) = &response.recovery {
        doc.insert(
            "recovery".into(),
            serde_json::to_value(recovery).unwrap_or(serde_json::Value::Null),
        );
    }
    if let Some(channels) = &response.channels {
        doc.insert(
            "channels".into(),
            serde_json::to_value(channels).unwrap_or(serde_json::Value::Null),
        );
    }
    if let Some(cache_status) = &response.cache_status {
        doc.insert("cache_status".into(), serde_json::json!(cache_status));
    }
    if let Some(saved) = response.saved_tokens_estimate {
        doc.insert("saved_tokens_estimate".into(), serde_json::json!(saved));
    }
    if let Some(remaining) = response.remaining_budget_tokens {
        doc.insert(
            "remaining_budget_tokens".into(),
            serde_json::json!(remaining),
        );
    }
    if let Some(exhausted) = response.budget_exhausted {
        doc.insert("budget_exhausted".into(), serde_json::json!(exhausted));
    }
    serde_json::to_string(&serde_json::Value::Object(doc)).unwrap_or_else(|_| {
        format!(
            "{{\"schema_version\":\"{CLI_SCHEMA_VERSION}\",\"status\":\"error\",\"tool\":\"{}\"}}",
            response.tool
        )
    })
}

const INLINE_EXACT_READ_MAX_BYTES: usize = 256;

/// `ok` for `tool` with no diagnostic / safety / recovery / channels.
fn clean_ok_envelope(response: &ToolResponse, tool: &str) -> bool {
    response.status == "ok"
        && response.tool == tool
        && response.diagnostic.is_none()
        && response.safety.is_none()
        && response.recovery.is_none()
        && response.channels.is_none()
}

fn quiet_verified_edit(response: &ToolResponse) -> bool {
    if !clean_ok_envelope(response, "edit")
        || response.ack.is_some()
        || response
            .visible
            .as_ref()
            .is_none_or(|visible| !visible.text.is_empty())
        || response.refs.is_empty()
        || response.refs.iter().any(|record| !record.live)
        || !response.refs.iter().any(|record| record.kind == "undo")
    {
        return false;
    }
    response
        .telemetry
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .is_some_and(|telemetry| {
            telemetry
                .get("transport_status")
                .and_then(serde_json::Value::as_str)
                == Some("ok")
                && telemetry
                    .get("exact_refs_available")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
                && telemetry
                    .get("dry_run")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false)
                && telemetry
                    .get("degraded")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false)
                && telemetry.get("warning").is_none()
                && telemetry
                    .get("storage_error")
                    .is_none_or(serde_json::Value::is_null)
        })
}

fn inline_exact_small_read(response: &ToolResponse, complete_source: bool) -> Option<String> {
    if !complete_source
        || !clean_ok_envelope(response, "read")
        || response
            .telemetry
            .as_ref()
            .and_then(|value| value.get("output_strategy"))
            .and_then(serde_json::Value::as_str)
            != Some("full")
    {
        return None;
    }
    let visible = response.visible.as_ref().filter(|visible| {
        visible.text.len() <= INLINE_EXACT_READ_MAX_BYTES
            && !visible
                .text
                .chars()
                .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
    })?;
    let mut blob_refs = response
        .refs
        .iter()
        .filter(|record| record.kind == "blob" && record.live);
    let blob = blob_refs.next().filter(|_| blob_refs.next().is_none())?;
    (visible.text.len() == blob.bytes).then(|| visible.text.clone())
}

fn redundant_warm_search_refs(response: &ToolResponse) -> bool {
    if !clean_ok_envelope(response, "grep") {
        return false;
    }
    let Some(telemetry) = response
        .telemetry
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .filter(|telemetry| {
            telemetry
                .get("output_strategy")
                .and_then(serde_json::Value::as_str)
                == Some("seen_set_dedup")
                && telemetry
                    .get("transport_status")
                    .and_then(serde_json::Value::as_str)
                    == Some("ok")
                && telemetry
                    .get("exact_refs_available")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
                && telemetry
                    .get("degraded")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false)
                && telemetry.get("warning").is_none()
                && telemetry
                    .get("storage_error")
                    .is_none_or(serde_json::Value::is_null)
                && telemetry
                    .get("truncated_by_visit")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false)
        })
    else {
        return false;
    };
    let search_refs = response
        .refs
        .iter()
        .filter(|record| record.kind == "search")
        .collect::<Vec<_>>();
    if search_refs.is_empty()
        || search_refs.iter().any(|record| !record.live)
        || telemetry.get("matches").and_then(serde_json::Value::as_u64)
            != Some(search_refs.len() as u64)
    {
        return false;
    }
    let mut blobs = response
        .refs
        .iter()
        .filter(|record| record.kind == "blob" && record.live && record.bytes > 0);
    blobs.next().is_some_and(|blob| {
        blobs.next().is_none()
            && response
                .visible
                .as_ref()
                .is_some_and(|visible| visible.text.contains(&blob.ref_id))
    })
}

pub fn render_text(response: &ToolResponse) -> String {
    render_text_inner(response, false)
}

/// Render a CLI text response whose resolved read input is one complete source.
/// Eligibility stays out-of-band so JSON, MCP, and ranged responses are unchanged.
pub fn render_text_with_complete_read(response: &ToolResponse) -> String {
    render_text_inner(response, true)
}

fn render_text_inner(response: &ToolResponse, complete_source: bool) -> String {
    if let Some(error) = &response.error {
        return format!("error: {} ({})\n", error.message, error.code);
    }
    if quiet_verified_edit(response) {
        return String::new();
    }
    if let Some(inline) = inline_exact_small_read(response, complete_source) {
        return inline;
    }
    let omit_search_refs = redundant_warm_search_refs(response);
    let mut out = String::new();
    if let Some(visible) = &response.visible {
        out.push_str(visible.text.trim_end());
        out.push('\n');
    }
    if !is_compact_shell_response(response) {
        for record in &response.refs {
            if omit_search_refs && record.kind == "search" {
                continue;
            }
            // Full shell capsules already anchor their refs in the header;
            // appending those again doubles every ref line. Only refs the
            // visible text does not carry (e.g. capture_ref) are added.
            if out.contains(&record.ref_id) {
                continue;
            }
            out.push_str(&format!("{}_ref: {}\n", record.kind, record.ref_id));
        }
    }
    out
}

pub fn is_compact_shell_response(response: &ToolResponse) -> bool {
    response.tool == "shell"
        && matches!(
            response
                .telemetry
                .as_ref()
                .and_then(|value| value.get("output_strategy"))
                .and_then(|value| value.as_str()),
            Some("compact_adaptive_shell") | Some("minimal_envelope_shell")
        )
}
