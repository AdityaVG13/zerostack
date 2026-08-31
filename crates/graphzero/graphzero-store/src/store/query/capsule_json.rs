//! JSON rendering for snap capsules and budgeted symbol capsules.

use std::path::Path;

use super::super::expand::json_escape;
use super::budget::{
    append_accounting, compact_truncated_budgeted, enforce_visible_byte_cap, knapsack_matches,
    spill_id_for_json, tokens_for_str,
};
use super::types::{Capsule, DestinationRef, QueryCapsule, RouteDiagnostics, SnapRoute};

pub fn kind_name(kind: u8) -> &'static str {
    match kind {
        0 => "calls",
        1 => "imports",
        2 => "refs",
        3 => "co_changed",
        4 => "session_followed",
        5 => "runtime_called",
        6 => "linter_smell",
        _ => "unknown",
    }
}

pub(crate) fn full_query_capsule_json(capsule: &QueryCapsule) -> String {
    let coverage_capsule = Capsule {
        query: capsule.query.clone(),
        snapshot_id: capsule.snapshot_id,
        matches: Vec::new(),
        tier_a: capsule.coverage.tier_a,
        tier_b: capsule.coverage.tier_b,
        tier_c: capsule.coverage.tier_c,
        budget: capsule.budget,
        freshness: super::types::FreshnessDiagnostics {
            check_freshness: capsule.coverage.freshness_verified,
            ..Default::default()
        },
    };
    render_query_capsule_json(
        &capsule.query,
        capsule.budget,
        capsule.route,
        &capsule.destinations,
        &coverage_capsule,
        &capsule.diagnostics,
        false,
        0,
        None,
        capsule.coverage.semantic_tier_percent,
    )
}

pub fn query_capsule_to_json(capsule: &QueryCapsule, store_root: Option<&Path>) -> String {
    let full = full_query_capsule_json(capsule);
    apply_query_capsule_budget(capsule, &full, store_root)
}

fn apply_query_capsule_budget(
    capsule: &QueryCapsule,
    full_json: &str,
    store_root: Option<&Path>,
) -> String {
    let full_tokens = tokens_for_str(full_json);
    let budget_bytes = capsule.budget.saturating_mul(4);
    if full_json.len() <= budget_bytes {
        let mut out = full_json.to_string();
        append_accounting(&mut out, full_tokens, full_tokens, false);
        return out;
    }
    let id = spill_id_for_json(store_root, full_json);
    if capsule.budget <= 5 {
        let query_ref = format!("query/{id}");
        let reference = if capsule.budget > 1 && capsule.destinations.len() == 1 {
            &capsule.destinations[0].destination_ref
        } else {
            &query_ref
        };
        return serde_json::to_string(reference).expect("query ref serialization cannot fail");
    }
    let mut dests = capsule.destinations.clone();
    let omitted = dests.len().saturating_sub(1);
    if dests.len() > 1 {
        dests.truncate(1);
    }
    let coverage_capsule = Capsule {
        query: capsule.query.clone(),
        snapshot_id: capsule.snapshot_id,
        matches: Vec::new(),
        tier_a: capsule.coverage.tier_a,
        tier_b: capsule.coverage.tier_b,
        tier_c: capsule.coverage.tier_c,
        budget: capsule.budget,
        freshness: super::types::FreshnessDiagnostics {
            check_freshness: capsule.coverage.freshness_verified,
            ..Default::default()
        },
    };
    let mut out = render_query_capsule_json(
        &capsule.query,
        capsule.budget,
        capsule.route,
        &dests,
        &coverage_capsule,
        &capsule.diagnostics,
        true,
        omitted,
        Some(format!("query/{id}")),
        capsule.coverage.semantic_tier_percent,
    );
    let visible_tokens = tokens_for_str(&out);
    append_accounting(&mut out, visible_tokens, full_tokens, false);
    out
}

#[allow(clippy::too_many_arguments)]
pub fn render_query_capsule_json(
    query: &str,
    budget: usize,
    route: SnapRoute,
    destinations: &[DestinationRef],
    coverage: &Capsule,
    diagnostics: &RouteDiagnostics,
    truncated: bool,
    omitted_count: usize,
    full_ref: Option<String>,
    semantic_tier_percent: f64,
) -> String {
    let mut dests = String::new();
    for (i, d) in destinations.iter().enumerate() {
        if i > 0 {
            dests.push(',');
        }
        let path_json = serde_json::to_string(&d.path).unwrap_or_else(|_| "null".into());
        // Snap-to-file target fields: canonical path#Lx-Ly plus
        // intent metadata and, for top hits, the inlined content window.
        let mut extra = String::new();
        if let Some(target) = &d.target {
            extra.push_str(&format!(",\"target\":\"{}\"", json_escape(target)));
        }
        if let Some(kind) = &d.kind {
            extra.push_str(&format!(",\"kind\":\"{}\"", json_escape(kind)));
        }
        if let Some(symbol) = &d.symbol {
            extra.push_str(&format!(",\"sym\":\"{}\"", json_escape(symbol)));
        }
        if let Some(content) = &d.content {
            extra.push_str(&format!(",\"content\":\"{}\"", json_escape(content)));
        }
        dests.push_str(&format!(
            "{{\"ref\":\"{}\",\"evidence_ref\":\"{}\",\"label\":\"{}\",\"path\":{path_json}{extra}}}",
            json_escape(&d.destination_ref),
            json_escape(&d.evidence_ref),
            json_escape(&d.label)
        ));
    }
    let sym_diag = diagnostics
        .symbol_route
        .map(|s| format!(",\"symbol_route\":\"{s}\""))
        .unwrap_or_default();
    let notes = if diagnostics.notes.is_empty() {
        String::new()
    } else {
        let joined: Vec<_> = diagnostics
            .notes
            .iter()
            .map(|n| format!("\"{}\"", json_escape(n)))
            .collect();
        format!(",\"notes\":[{}]", joined.join(","))
    };
    let degraded = if diagnostics.degraded_tiers.is_empty() {
        String::new()
    } else {
        format!(
            ",\"degraded_tiers\":[{}]",
            diagnostics
                .degraded_tiers
                .iter()
                .map(|t| format!("\"{t}\""))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let dedup = if diagnostics.removed_count > 0 {
        format!(
            ",\"removed_count\":{},\"session_deduped\":{},\"byte_deduped\":{},\"entity_deduped\":{}",
            diagnostics.removed_count,
            diagnostics.removed_count,
            diagnostics.byte_deduped,
            diagnostics.entity_deduped
        )
    } else {
        String::new()
    };
    let trunc = if truncated {
        let full_ref = full_ref
            .as_deref()
            .map(|r| format!(",\"full_ref\":\"{}\"", json_escape(r)))
            .unwrap_or_default();
        format!(",\"truncated\":true,\"omitted_count\":{omitted_count}{full_ref}")
    } else {
        String::new()
    };
    let actual_used = tokens_for_str(query) + dests.len().div_ceil(4) + 16;
    let used = actual_used.min(budget);
    let remaining = budget.saturating_sub(used);
    let budget_exceeded = actual_used > budget;
    let dedup_ledger = crate::process_dedup_ledger();
    format!(
        "{{\"schema_version\":1,\"query\":\"{}\",\"budget\":{budget},\"snapshot_id\":{},\"route\":\"{}\",\"destinations\":[{dests}],\"coverage\":{{\"tier_a\":{:.4},\"tier_b\":{:.4},\"tier_c\":{:.4},\"semantic_tier_percent\":{:.4},\"freshness_verified\":{}}},\"diagnostics\":{{{sym_diag}{notes}{degraded}{dedup}}},\"ledger\":{{\"requested_budget\":{budget},\"used_budget\":{used},\"actual_used_budget\":{actual_used},\"remaining_budget\":{remaining},\"budget_exceeded\":{budget_exceeded},\"truncated\":{truncated}{trunc},\"byte_dedup_rate_pct\":{},\"entity_cross_view_dedup_rate_pct\":{},\"max_repeat_encounter_pct\":{},\"repeat_encounter_pct_gate\":{}}}}}",
        json_escape(query),
        coverage.snapshot_id,
        route.as_str(),
        coverage.tier_a,
        coverage.tier_b,
        coverage.tier_c,
        semantic_tier_percent,
        coverage.freshness.check_freshness
            && coverage.freshness.events.is_empty()
            && diagnostics.degraded_tiers.is_empty(),
        dedup_ledger.byte_dedup_rate_pct(),
        dedup_ledger.entity_cross_view_dedup_rate_pct(),
        dedup_ledger.max_repeat_encounter_pct,
        crate::REPEAT_ENCOUNTER_PCT,
        sym_diag = sym_diag.trim_start_matches(','),
    )
}

pub fn render_budgeted_capsule(capsule: &Capsule) -> String {
    let mut s = format!(
        "{{\"query\":\"{}\",\"snapshot\":{},\"matches\":[",
        json_escape(&capsule.query),
        capsule.snapshot_id
    );
    for (i, m) in capsule.matches.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"symbol\":\"{}\",\"defs\":[",
            json_escape(&m.name)
        ));
        for (j, d) in m.defs.iter().enumerate() {
            if j > 0 {
                s.push(',');
            }
            s.push_str(&format!("{{\"ref\":\"{}\"", d.evidence_ref));
            if let Some(p) = &d.path {
                s.push_str(&format!(",\"path\":\"{}\"", json_escape(p)));
            }
            if d.stale {
                s.push_str(",\"stale\":true");
            }
            s.push('}');
        }
        s.push_str("],\"edges\":[");
        for (j, e) in m.edges.iter().enumerate() {
            if j > 0 {
                s.push(',');
            }
            let src = e
                .source
                .as_ref()
                .map(|s| format!(",\"source\":\"{}\"", json_escape(s)))
                .unwrap_or_default();
            s.push_str(&format!(
                "{{\"kind\":\"{}\",\"to\":\"{}\",\"confidence\":{:.2},\"evidence_ref\":\"{}\"{src}}}",
                kind_name(e.kind),
                json_escape(&e.to),
                e.confidence,
                e.evidence_ref
            ));
        }
        s.push_str("]}");
    }
    s.push_str(&format!(
        "],\"coverage\":{{\"tier_a\":{:.4},\"tier_b\":{:.4},\"tier_c\":{:.4}}},\"budget\":{}}}",
        capsule.tier_a, capsule.tier_b, capsule.tier_c, capsule.budget
    ));
    s
}

pub fn capsule_to_json(capsule: &Capsule, store_root: Option<&Path>) -> String {
    let full = render_budgeted_capsule(capsule);
    let full_tokens = tokens_for_str(&full);
    if capsule.budget <= 1 {
        let id = spill_id_for_json(store_root, &full);
        return compact_truncated_budgeted(
            &capsule.query,
            capsule.snapshot_id,
            capsule.budget.max(1),
            &id,
            full_tokens,
        );
    }
    let budget_bytes = capsule.budget.saturating_mul(4);
    if full.len() <= budget_bytes {
        let mut out = full;
        enforce_visible_byte_cap(
            &mut out,
            budget_bytes,
            full_tokens,
            &capsule.query,
            capsule.snapshot_id,
            capsule.budget,
            "",
        );
        return out;
    }
    let id = spill_id_for_json(store_root, &full);
    let (kept, _omitted) = knapsack_matches(&capsule.matches, budget_bytes);
    let omitted_matches = capsule.matches.len().saturating_sub(kept.len());
    let mut budgeted = capsule.clone();
    budgeted.matches = kept;
    let mut out = render_budgeted_capsule(&budgeted);
    if out.len() > budget_bytes {
        budgeted.matches.clear();
        out = render_budgeted_capsule(&budgeted);
    }
    if out.len() > budget_bytes {
        out = format!(
            "{{\"query\":\"{}\",\"snapshot\":{},\"matches\":[],\"coverage\":{{\"tier_a\":{:.4},\"tier_b\":{:.4},\"tier_c\":{:.4}}},\"budget\":{}}}",
            json_escape(&capsule.query),
            capsule.snapshot_id,
            capsule.tier_a,
            capsule.tier_b,
            capsule.tier_c,
            capsule.budget
        );
    }
    let tail = format!(
        ",\"truncated\":{{\"omitted_matches\":{omitted_matches},\"full_ref\":\"query/{id}\"}}}}"
    );
    out.truncate(out.len() - 1);
    out.push_str(&tail);
    enforce_visible_byte_cap(
        &mut out,
        budget_bytes,
        full_tokens,
        &capsule.query,
        capsule.snapshot_id,
        capsule.budget,
        &id,
    );
    out
}
