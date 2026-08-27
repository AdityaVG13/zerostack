//! `tz_recall`: lossless full-text search over the on-disk recovery cache.
//!
//! The recovery store's in-memory state is private to `tokenzero-recovery`,
//! so recall reads the persisted cache file as tolerant JSON: entries that
//! match the published `StoredFile` shape (`ref_id` + `text` + optional
//! `path`/`content_type`) are searched, anything else is skipped, and any
//! read/parse failure degrades to zero hits with a diagnostic. The store is
//! never mutated — recall is a read-only meta-query, and every hit line
//! carries the exact `tz://` ref so the full payload stays one `tz_expand`
//! away.

use std::collections::HashSet;
use std::path::Path;

use serde_json::Value;

/// One matched line inside a stored payload.
pub(crate) struct RecallHit {
    pub ref_id: String,
    /// Source path when the payload had one, else its content type.
    pub label: String,
    pub line: usize,
    pub text: String,
}

pub(crate) struct RecallOutcome {
    pub hits: Vec<RecallHit>,
    pub payloads_searched: usize,
    pub truncated: bool,
    /// A cache that exists but cannot be read or parsed. A missing cache is
    /// an empty store, not an error.
    pub unreadable: bool,
}

const MAX_HIT_LINE_CHARS: usize = 160;

/// Render each payload ref once. Validated flat-search hit groups also factor
/// their shared directory prefix under one `# root:` header, and when every
/// hit shares one non-empty suffix, that suffix is factored under `# suffix:`.
/// The projection remains lossless: the group selector preserves every
/// stored-payload line, and root + relative row (+ suffix when emitted)
/// recovers the matched text exactly.
pub(crate) fn render_hits(hits: &[RecallHit]) -> String {
    let mut groups = Vec::new();
    let mut start = 0;
    while start < hits.len() {
        let first = &hits[start];
        let mut end = start + 1;
        while end < hits.len() && hits[end].ref_id == first.ref_id && hits[end].label == first.label
        {
            end += 1;
        }
        let group = &hits[start..end];
        let flat = group
            .iter()
            .map(|hit| format!("{} {}:{}: {}", hit.ref_id, hit.label, hit.line, hit.text))
            .collect::<Vec<_>>()
            .join("\n");
        let compact = compact_group(group);
        let compact_is_cheaper = compact.len() < flat.len()
            && tokenzero_core::count_tokens(&compact) < tokenzero_core::count_tokens(&flat);
        groups.push(if compact_is_cheaper { compact } else { flat });
        start = end;
    }
    groups.join("\n")
}

fn compact_group(hits: &[RecallHit]) -> String {
    let first = &hits[0];
    let mut lines = vec![format!(
        "{} {}{}",
        first.ref_id,
        first.label,
        line_selector(hits)
    )];
    let Some(paths) = hits
        .iter()
        .map(|hit| flat_search_path(&hit.text))
        .collect::<Option<Vec<_>>>()
    else {
        lines.extend(hits.iter().map(|hit| format!("{}: {}", hit.line, hit.text)));
        return lines.join("\n");
    };
    // When every absolute-path hit shares one non-empty suffix (line/column
    // plus matched row), factor it under `# suffix:` so the projection stays
    // lossless with root + relative path + suffix reconstructing the hit.
    let shared_suffix = {
        let first_suffix = &hits[0].text[paths[0].len()..];
        if first_suffix.is_empty() {
            None
        } else {
            hits.iter()
                .zip(&paths)
                .all(|(hit, path)| &hit.text[path.len()..] == first_suffix)
                .then_some(first_suffix)
        }
    };
    if let (Some(suffix), Some(root)) = (
        shared_suffix,
        common_directory_prefix(paths.clone().into_iter()),
    ) {
        lines.push(format!("# root: {root}"));
        lines.push(format!("# suffix: {suffix}"));
        lines.extend(
            paths
                .iter()
                .map(|path| path.strip_prefix(root).unwrap_or(path).to_string()),
        );
        return lines.join("\n");
    }
    if let Some(root) = common_directory_prefix(paths.into_iter()) {
        lines.push(format!("# root: {root}"));
        lines.extend(
            hits.iter()
                .map(|hit| hit.text.strip_prefix(root).unwrap_or(&hit.text).to_string()),
        );
    } else {
        lines.extend(hits.iter().map(|hit| format!("{}: {}", hit.line, hit.text)));
    }
    lines.join("\n")
}

fn line_selector(hits: &[RecallHit]) -> String {
    let start = hits[0].line;
    if hits
        .iter()
        .enumerate()
        .all(|(offset, hit)| hit.line == start.saturating_add(offset))
    {
        if hits.len() == 1 {
            format!("#L{start}")
        } else {
            format!("#L{start}-{}", hits[hits.len() - 1].line)
        }
    } else {
        format!(
            "#L{}",
            hits.iter()
                .map(|hit| hit.line.to_string())
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

fn flat_search_path(text: &str) -> Option<&str> {
    for (separator, _) in text.match_indices(':') {
        let suffix = &text[separator + 1..];
        let digits = suffix.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || suffix.as_bytes().get(digits) != Some(&b':') {
            continue;
        }
        let path = &text[..separator];
        let bytes = path.as_bytes();
        let absolute = path.starts_with('/')
            || path.starts_with("\\\\")
            || (bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'/' | b'\\'));
        if absolute {
            return Some(path);
        }
    }
    None
}

fn common_directory_prefix<'a>(values: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let values = values.collect::<Vec<_>>();
    let first = *values.first()?;
    if values.len() < 2 {
        return None;
    }
    let shared_len = values.iter().skip(1).fold(first.len(), |limit, value| {
        let equal_bytes = first
            .chars()
            .zip(value.chars())
            .take_while(|(left, right)| left == right)
            .map(|(ch, _)| ch.len_utf8())
            .sum::<usize>();
        limit.min(equal_bytes)
    });
    let prefix = &first[..shared_len];
    let separator = prefix.rfind(['/', '\\'])? + 1;
    (separator > 1).then_some(&first[..separator])
}

/// Recall parses the whole cache file per query — a linear scan bounded by
/// the store's own eviction ceiling (8 MB by default). This guard mirrors
/// the recovery crate's `max_load_bytes` so a foreign or corrupted file at
/// the cache path degrades to `unreadable` instead of an unbounded parse.
const MAX_RECALL_LOAD_BYTES: u64 = 16_000_000;

/// Case-insensitive substring search across every stored payload's lines.
/// `files` entries are searched first (they carry source paths); `blobs`
/// whose exact text was already covered by a file entry are skipped so the
/// same content never reports twice.
pub(crate) fn recall_search(cache_path: &Path, query: &str, max_hits: usize) -> RecallOutcome {
    let mut outcome = RecallOutcome {
        hits: Vec::new(),
        payloads_searched: 0,
        truncated: false,
        unreadable: false,
    };
    if std::fs::metadata(cache_path).is_ok_and(|meta| meta.len() > MAX_RECALL_LOAD_BYTES) {
        outcome.unreadable = true;
        return outcome;
    }
    let raw = match std::fs::read_to_string(cache_path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return outcome,
        Err(_) => {
            outcome.unreadable = true;
            return outcome;
        }
    };
    let Ok(snapshot) = serde_json::from_str::<Value>(&raw) else {
        outcome.unreadable = true;
        return outcome;
    };
    let needle = query.to_lowercase();
    let mut seen_texts: HashSet<u64> = HashSet::new();
    for entry in snapshot_entries(&snapshot) {
        let Some(text) = entry.get("text").and_then(Value::as_str) else {
            continue;
        };
        let Some(ref_id) = entry.get("ref_id").and_then(Value::as_str) else {
            continue;
        };
        if !seen_texts.insert(text_fingerprint(text)) {
            continue;
        }
        outcome.payloads_searched += 1;
        let label = entry_label(entry, false);
        for (idx, line) in text.lines().enumerate() {
            if !line.to_lowercase().contains(&needle) {
                continue;
            }
            outcome.hits.push(RecallHit {
                ref_id: ref_id.to_string(),
                label: label.clone(),
                line: idx + 1,
                text: clamp_line(line),
            });
        }
    }
    let order: Vec<String> = snapshot
        .get("order")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if !order.is_empty() {
        outcome.hits.sort_by(|left, right| {
            tokenzero_recovery::score_from_order(&order, &left.ref_id)
                .total_cmp(&tokenzero_recovery::score_from_order(&order, &right.ref_id))
                .reverse()
                .then(left.ref_id.cmp(&right.ref_id))
                .then(left.line.cmp(&right.line))
        });
    }
    if outcome.hits.len() > max_hits {
        outcome.hits.truncate(max_hits);
        outcome.truncated = true;
    }
    outcome
}

/// Compact "what TokenZero already served this workspace" listing for
/// post-compaction recovery: most-recent stored payloads first, each with
/// its exact `tz://` ref, token-budgeted. Pure read over the persisted
/// cache; returns `None` when there is nothing to restore.
pub(crate) fn build_session_pack(cache_path: &Path, max_tokens: usize) -> Option<String> {
    let raw = std::fs::read_to_string(cache_path).ok()?;
    let snapshot: Value = serde_json::from_str(&raw).ok()?;
    let entries: Vec<(&str, &Value)> = snapshot_entries(&snapshot)
        .filter_map(|entry| {
            entry
                .get("ref_id")
                .and_then(Value::as_str)
                .map(|ref_id| (ref_id, entry))
        })
        .collect();
    if entries.is_empty() {
        return None;
    }
    let total_unique = entries
        .iter()
        .filter_map(|(_, entry)| entry.get("text").and_then(Value::as_str))
        .filter(|text| !text.is_empty())
        .map(text_fingerprint)
        .collect::<HashSet<u64>>()
        .len();
    let by_ref: std::collections::HashMap<&str, &Value> = entries.iter().copied().collect();
    // The order array is append-ordered; walk it backwards for recency and
    // dedup identical content (a file and its blob share bytes).
    let order: Vec<&str> = snapshot
        .get("order")
        .and_then(Value::as_array)
        .map(|refs| refs.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut seen_texts: HashSet<u64> = HashSet::new();
    let mut listed = 0usize;
    let header = "# tokenzero session pack — context already served in this workspace\n\
                  Recover any item exactly with `expand <ref>`; search all stored content with `recall <query>` — never re-read or re-run for content listed here.";
    let mut pack = header.to_string();
    let candidates = order
        .iter()
        .rev()
        .filter_map(|ref_id| by_ref.get(ref_id).map(|entry| (*ref_id, *entry)))
        .chain(entries.iter().map(|(ref_id, entry)| (*ref_id, *entry)));
    for (ref_id, entry) in candidates {
        let Some(text) = entry.get("text").and_then(Value::as_str) else {
            continue;
        };
        if text.is_empty() || !seen_texts.insert(text_fingerprint(text)) {
            continue;
        }
        let label = entry_label(entry, true);
        let line = format!(
            "\n- {label} — {} lines, ~{} tok — {ref_id}",
            text.lines().count(),
            text.len() / 4
        );
        let extended = format!("{pack}{line}");
        if tokenzero_core::count_tokens(&extended) > max_tokens {
            break;
        }
        pack = extended;
        listed += 1;
    }
    if listed == 0 {
        return None;
    }
    let remainder = total_unique.saturating_sub(listed);
    if remainder > 0 {
        pack.push_str(&format!(
            "\n(+ {remainder} more stored payloads — `recall <query>` searches them all)"
        ));
    }
    Some(pack)
}

fn snapshot_entries(snapshot: &Value) -> impl Iterator<Item = &Value> {
    ["files", "blobs"].into_iter().flat_map(move |section| {
        snapshot
            .get(section)
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|map| map.values())
    })
}

fn entry_label(entry: &Value, shell_capture: bool) -> String {
    match entry.get("path").and_then(Value::as_str) {
        Some(path) if shell_capture && path.starts_with("shell:") => "shell capture".to_string(),
        Some(path) => path.to_string(),
        None => entry
            .get("content_type")
            .and_then(Value::as_str)
            .unwrap_or("payload")
            .to_string(),
    }
}

fn clamp_line(line: &str) -> String {
    let trimmed = line.trim();
    let clamped: String = trimmed.chars().take(MAX_HIT_LINE_CHARS).collect();
    if clamped.chars().count() < trimmed.chars().count() {
        format!("{clamped}...")
    } else {
        clamped
    }
}

fn text_fingerprint(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

