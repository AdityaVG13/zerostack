use std::collections::BTreeMap;

use tokenzero_core::{ContentType, count_tokens};
use tokenzero_recovery::RecoveryStore;

#[derive(Clone, Copy)]
enum AliasKind {
    Path,
    Symbol,
}

fn classify(value: &str) -> Option<AliasKind> {
    if value.len() < 16 || value.contains("://") {
        return None;
    }
    if value.contains('/') {
        return Some(AliasKind::Path);
    }
    let mut segments = value.split("::");
    let first = segments.next()?;
    let rest = segments.collect::<Vec<_>>();
    (!first.is_empty()
        && !rest.is_empty()
        && std::iter::once(first).chain(rest).all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        }))
    .then_some(AliasKind::Symbol)
}

/// Borrows every candidate out of `text` rather than allocating a `String` per
/// token. This runs on every response, so an allocation per whitespace-delimited
/// token was pure per-request cost on text that usually has nothing to alias.
fn candidates(text: &str) -> Vec<(&str, Vec<(usize, usize)>)> {
    let mut found = BTreeMap::<&str, Vec<(usize, usize)>>::new();
    let mut start = None;
    for (index, character) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        let allowed =
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | '/' | ':');
        if allowed {
            start.get_or_insert(index);
            continue;
        }
        let Some(token_start) = start.take() else {
            continue;
        };
        let mut token_end = index;
        while token_end > token_start && matches!(text.as_bytes()[token_end - 1], b'.' | b':') {
            token_end -= 1;
        }
        if token_end > token_start {
            let raw = &text[token_start..token_end];
            let value_end = raw
                .rsplit_once(':')
                .filter(|(path, suffix)| {
                    path.contains('/')
                        && !suffix.is_empty()
                        && suffix.bytes().all(|byte| byte.is_ascii_digit())
                })
                .map_or(token_end, |(_, suffix)| token_end - suffix.len() - 1);
            let value = &text[token_start..value_end];
            if classify(value).is_some() {
                found
                    .entry(value)
                    .or_default()
                    .push((token_start, value_end));
            }
        }
    }
    let mut repeated = found
        .into_iter()
        .filter(|(_, spans)| spans.len() > 1)
        .collect::<Vec<_>>();
    repeated.sort_by_key(|(_, spans)| spans[0].0);
    repeated
}

/// True when `text` contains at least one repeated path/symbol atom worth
/// aliasing. This is a pure scan: it opens no store and mints no ordinal, so
/// callers can skip the whole aliasing pipeline on the common no-candidate
/// response instead of paying for a store lease and a full-text token recount.
pub fn has_alias_candidates(text: &str) -> bool {
    if !may_contain_alias_atom(text) {
        return false;
    }
    let floor = ordinal_token_floor();
    candidates(text)
        .into_iter()
        .any(|(value, _)| count_tokens(value) > floor)
}

/// Cheap necessary condition for an alias candidate. `classify` only ever
/// accepts a value containing `/` (path) or `::` (symbol), so text with neither
/// cannot produce a candidate and does not need the char-by-char scan or any
/// tokenizer call. This is a conservative prefilter: it may say "maybe" and let
/// the real scan decide, but it never says "no" to text `classify` would accept.
fn may_contain_alias_atom(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut previous_colon = false;
    for &byte in bytes {
        if byte == b'/' {
            return true;
        }
        if byte == b':' {
            if previous_colon {
                return true;
            }
            previous_colon = true;
        } else {
            previous_colon = false;
        }
    }
    false
}

/// `count_tokens` runs a real tokenizer, so the shortest possible ordinal form
/// is measured once per process instead of once per candidate.
fn ordinal_token_floor() -> usize {
    static FLOOR: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *FLOOR.get_or_init(|| count_tokens("tz://o/1/1"))
}

/// Returns `Some(rewritten)` only when aliasing actually replaced something, so
/// callers can tell "nothing changed" from "changed" without comparing strings.
pub fn alias_repeated_paths_and_symbols_if_changed(
    store: &mut RecoveryStore,
    text: &str,
) -> Option<String> {
    let mut replacements = Vec::<(usize, usize, String)>::new();
    let floor = ordinal_token_floor();
    for (value, spans) in candidates(text) {
        if count_tokens(value) <= floor {
            continue;
        }
        let Ok(full_ref) = store.store_blob(value, ContentType::Unknown) else {
            continue;
        };
        let _short_ref = store.register_session_visible_alias(&full_ref);
        let Ok(range) = store.reserve_ordinal_range(1) else {
            continue;
        };
        let Ok(ordinal_ref) = store.store_ordinal_alias_deferred(range, 0, &full_ref) else {
            continue;
        };
        if count_tokens(value) <= count_tokens(&ordinal_ref) || store.persist_pending().is_err() {
            continue;
        }
        replacements.extend(
            spans
                .into_iter()
                .map(|(start, end)| (start, end, ordinal_ref.clone())),
        );
    }
    if replacements.is_empty() {
        return None;
    }
    replacements.sort_by_key(|(start, _, _)| *start);
    let mut rewritten = text.to_string();
    for (start, end, alias) in replacements.into_iter().rev() {
        rewritten.replace_range(start..end, &alias);
    }
    // Candidate-local token counts are not a sound proof of a win: BPE cost is
    // contextual, so an ordinal that is cheaper standalone can cost more once
    // the surrounding tokens merge. Publish the rewrite only when the final
    // whole string is strictly cheaper than the original.
    if count_tokens(&rewritten) >= count_tokens(text) {
        return None;
    }
    Some(rewritten)
}

