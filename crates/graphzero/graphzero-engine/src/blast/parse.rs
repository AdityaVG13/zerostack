//! Intent parsing and planned-edit impact helpers.

use std::collections::{BTreeSet, HashMap};

use super::traverse::blast_radius;
use super::types::{
    BLAST_SCHEMA_VERSION, BlastError, BlastIntentParse, CoveringTest, PlannedImpact,
    SpeculativeBlastReport, SpeculativeBlastRequest,
};
use graphzero_store::Snapshot;

/// Parse free-text blast/reserve intent (canonical implementation in graphzero-store).
pub fn parse_intent(intent: &str) -> BlastIntentParse {
    graphzero_store::parse_intent(intent)
}

pub fn impact_before_edit(
    snapshot: &Snapshot,
    request: SpeculativeBlastRequest,
    budget: usize,
) -> Result<SpeculativeBlastReport, BlastError> {
    // Validate any supplied FSZero world envelope before graph work so
    // unsupported majors, malformed envelopes, and mismatched world refs
    // fail loudly instead of degrading to an empty or wrong result.
    let world_ref = crate::world_envelope::bind_world_envelope(
        &request.world_ref,
        request.world_envelope.as_deref(),
    )
    .map_err(|e| BlastError::WorldEnvelope(format!("{e:#}")))?;
    if world_ref.trim().is_empty() {
        return Err(BlastError::Parse("empty world_ref".into()));
    }
    let mut base = Vec::new();
    let mut impacted_symbols = BTreeSet::new();
    let mut impacted_files = BTreeSet::new();
    let mut impacted_test_paths = BTreeSet::new();
    let mut tests_by_path: HashMap<String, CoveringTest> = HashMap::new();

    for symbol in &request.focus_symbols {
        let capsule = blast_radius(snapshot, &format!("change signature of {symbol}"), budget)?;
        impacted_symbols.insert(capsule.target_symbol.clone());
        for site in &capsule.break_sites {
            impacted_symbols.insert(site.symbol.clone());
        }
        for test in &capsule.covering_tests {
            impacted_test_paths.insert(test.path_hint.clone());
            tests_by_path.insert(test.path_hint.clone(), test.clone());
        }
        base.push(capsule);
    }

    let graph_paths: BTreeSet<&str> = snapshot
        .path_records()
        .map(|(_, rec)| rec.path.as_str())
        .collect();
    let mut planned_impacts = Vec::new();
    for edit in &request.planned_edits {
        impacted_files.insert(edit.path.clone());
        if !graph_paths.contains(edit.path.as_str()) {
            planned_impacts.push(PlannedImpact {
                kind: "untracked_file".into(),
                symbol: None,
                path: edit.path.clone(),
                provenance: world_ref.clone(),
                detail: "planned edit touches a path absent from the current graph".into(),
            });
        }

        let before_calls = call_tokens(&edit.before);
        let after_calls = call_tokens(&edit.after);
        for removed in before_calls
            .iter()
            .copied()
            .filter(|token| !after_calls.contains(token))
        {
            impacted_symbols.insert(removed.to_owned());
            planned_impacts.push(PlannedImpact {
                kind: "removed_call".into(),
                symbol: Some(removed.to_owned()),
                path: edit.path.clone(),
                provenance: world_ref.clone(),
                detail: format!("planned edit removes call to {removed}"),
            });
        }
        for added in after_calls
            .iter()
            .copied()
            .filter(|token| !before_calls.contains(token))
        {
            impacted_symbols.insert(added.to_owned());
            planned_impacts.push(PlannedImpact {
                kind: "added_call".into(),
                symbol: Some(added.to_owned()),
                path: edit.path.clone(),
                provenance: world_ref.clone(),
                detail: format!("planned edit adds call to {added}"),
            });
        }
        if let Some((from, to)) = renamed_identifier(&edit.before, &edit.after) {
            impacted_symbols.insert(from.to_owned());
            impacted_symbols.insert(to.to_owned());
            planned_impacts.push(PlannedImpact {
                kind: "renamed_symbol".into(),
                symbol: Some(from.to_owned()),
                path: edit.path.clone(),
                provenance: world_ref.clone(),
                detail: format!("planned edit renames {from} to {to}"),
            });
        }
    }

    let mut impacted_tests = Vec::new();
    for path in impacted_test_paths {
        if let Some(test) = tests_by_path.remove(&path) {
            impacted_tests.push(test);
        }
    }
    impacted_tests.sort_by(|a, b| a.path_hint.cmp(&b.path_hint));

    Ok(SpeculativeBlastReport {
        schema_version: BLAST_SCHEMA_VERSION,
        world_ref,
        focus_symbols: request.focus_symbols,
        base,
        planned_impacts,
        impacted_symbols: impacted_symbols.into_iter().collect(),
        impacted_files: impacted_files.into_iter().collect(),
        impacted_tests,
    })
}

fn call_tokens(src: &str) -> BTreeSet<&str> {
    let bytes = src.as_bytes();
    let mut out = BTreeSet::new();
    let mut i = 0;
    while i < bytes.len() {
        if !is_ident_start(bytes[i] as char) {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < bytes.len() && is_ident_continue(bytes[i] as char) {
            i += 1;
        }
        let token = &src[start..i];
        let mut j = i;
        while j < bytes.len() && (bytes[j] as char).is_ascii_whitespace() {
            j += 1;
        }
        if j < bytes.len()
            && bytes[j] == b'('
            && !matches!(token, "fn" | "if" | "for" | "while" | "match")
        {
            out.insert(token);
        }
    }
    out
}

fn renamed_identifier<'before, 'after>(
    before: &'before str,
    after: &'after str,
) -> Option<(&'before str, &'after str)> {
    let before_tokens = identifier_tokens(before);
    let after_tokens = identifier_tokens(after);
    let from = single_difference(&before_tokens, &after_tokens)?;
    let to = single_difference(&after_tokens, &before_tokens)?;
    Some((from, to))
}

fn single_difference<'left, 'right>(
    left: &BTreeSet<&'left str>,
    right: &BTreeSet<&'right str>,
) -> Option<&'left str> {
    let mut difference = left.iter().copied().filter(|token| !right.contains(token));
    let only = difference.next()?;
    difference.next().is_none().then_some(only)
}

fn identifier_tokens(src: &str) -> BTreeSet<&str> {
    let bytes = src.as_bytes();
    let mut out = BTreeSet::new();
    let mut i = 0;
    while i < bytes.len() {
        if !is_ident_start(bytes[i] as char) {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < bytes.len() && is_ident_continue(bytes[i] as char) {
            i += 1;
        }
        out.insert(&src[start..i]);
    }
    out
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}
fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}
