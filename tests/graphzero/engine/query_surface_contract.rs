mod common;

use common::surface::{
    PREVENTED_READ_ACCOUNTING_SURFACES, all_surface_coverage_probes, body, coverage_probe, execute,
    named, req,
};
use graphzero_engine::query_surface::{QuerySurfaceError, QuerySurfaceRequest, QuerySurfaceRouter};
use graphzero_store::{ContentHash, ExpandResolver, GzRef};

// Purpose ledger (verification-purpose-gate): test -> protected public
// behavior · failure class · oracle strength · decision.
//
// | Test | Behavior | Failure class | Oracle | Decision |
// |---|---|---|---|---|
// | symbol_surface | decl ref + tier-a coverage | decl/coverage regression | strong | keep |
// | callers_surface_and_evidence_expandable | caller edges carry resolvable evidence | edge/evidence regression | strong | keep |
// | deps_surface | deps emits only imports edges, capped confidence, resolvable refs | import edge/attribution regression | strengthened | rewrite |
// | deps_surface_reports_concrete_import_edges | import edge existence + target name | broken import assembly | strong | add |
// | outline_surface / outline_line_ranges_and_skeleton / outline_budget_one_keeps_skeleton_inline | outline rows + budget-1 skeleton | layout/budget regression | strong | keep |
// | context_surface | capsule content + evidence footer resolves | capsule assembler/ref regression | strengthened | rewrite |
// | hot_tier_c_absence / changes_tier_c_gate | tier-c stays empty with absence evidence | tier-c leak | strong | keep |
// | word_surface | word hits non-empty | lexical regression | medium | keep |
// | search_dedup_sha / search_hits_carry_matched_line_snippets_not_full_payloads / whole_blob_path_hits_have_empty_snippet | search dedup + snippet/expand contract | search/expand regression | strong | keep |
// | evidence_missing_is_error_variant | typed error variant | error contract | strong | keep |
// | every_surface_has_coverage_footer | all surfaces emit coverage footer | footer regression | medium | keep |
// | budget_one_spills_heavy_surfaces / budget_one_preserves_symbol_not_found_error | budget-1 shell + error/accounting visibility | budget contract | strong | keep |
// | callpath_surface_returns_shortest_call_chain | shortest chain + evidence | callpath regression | strong | keep |
// | reading_set_surface_ranks_target_callers_and_callees / reading_set_emits_closure_confidence_contract / reading_set_mutation_style_static_caller_chain_is_inside_closure | ranking/closure/mutation model | reading-set regression | strong | keep |
// | durable_accounting_surfaces_are_declared_and_enforced / orient_and_reading_set_report_prevented_read_accounting | prevented-read accounting complement | accounting regression | strong | keep |

#[test]
fn symbol_surface() {
    let fx = common::indexed_fixture();
    let resp = execute(&fx, named("symbol", "alpha", 800));
    assert_eq!(resp.surface, "symbol");
    let decl = resp.decl_ref.as_ref().unwrap();
    assert!(
        decl.starts_with("g:") || decl.starts_with("gz://"),
        "decl_ref={decl}"
    );
    assert!(resp.coverage.tier_a > 0.0);
}

#[test]
fn callers_surface_and_evidence_expandable() {
    let fx = common::indexed_fixture();
    let resolver = ExpandResolver::new(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let resp = execute(&fx, named("callers", "beta", 800));
    assert!(!resp.edges.is_empty(), "gamma/alpha should call beta");
    for e in &resp.edges {
        assert!(!e.evidence_ref.is_empty());
        assert_eq!(e.source, "tier_a");
        let gz = GzRef::parse(&e.evidence_ref).unwrap();
        resolver.resolve(&gz, &e.evidence_ref).unwrap();
    }
}

#[test]
fn deps_surface() {
    let fx = common::indexed_fixture();
    let resolver = ExpandResolver::new(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let resp = execute(&fx, named("deps", "alpha", 800));
    assert_eq!(resp.surface, "deps");
    assert!(
        resp.coverage.tier_a > 0.0,
        "deps must keep coverage semantics"
    );
    // deps exposes ONLY imports edges attributed to the queried symbol with
    // capped confidence and resolvable evidence (deps handler contract).
    for e in &resp.edges {
        assert_eq!(e.kind, "imports");
        assert_eq!(e.from.as_deref(), Some("alpha"));
        assert_eq!(e.source, "tier_a");
        assert!(e.confidence <= 0.7, "syntactic imports capped at 0.7");
        let gz = GzRef::parse(&e.evidence_ref).unwrap();
        assert!(
            !resolver
                .resolve(&gz, &e.evidence_ref)
                .unwrap()
                .bytes
                .is_empty(),
            "deps evidence must expand"
        );
    }
}

#[test]
fn deps_surface_reports_concrete_import_edges() {
    let repo = common::indexed_repo(&[
        ("src/a.rs", "pub struct Widget;\n"),
        (
            "src/b.rs",
            "use crate::Widget;\n\npub fn make() -> Widget {\n    Widget\n}\n",
        ),
    ]);
    let resolver = ExpandResolver::new(&repo.store, Some(&repo.repo)).unwrap();
    let resp = QuerySurfaceRouter::execute(&repo.snapshot, &named("deps", "make", 800)).unwrap();
    assert_eq!(resp.surface, "deps");
    assert!(
        !resp.edges.is_empty(),
        "importing symbol must report concrete import edges"
    );
    let targets: Vec<&str> = resp.edges.iter().map(|e| e.to.as_str()).collect();
    assert!(
        targets.iter().any(|to| to.contains("Widget")),
        "deps must name the imported target, got {targets:?}"
    );
    for e in &resp.edges {
        assert_eq!(e.kind, "imports");
        assert_eq!(e.from.as_deref(), Some("make"));
        assert!(e.confidence <= 0.7);
        let gz = GzRef::parse(&e.evidence_ref).unwrap();
        assert!(
            !resolver
                .resolve(&gz, &e.evidence_ref)
                .unwrap()
                .bytes
                .is_empty()
        );
    }
}

#[test]
fn outline_surface() {
    let fx = common::indexed_fixture();
    let resp = execute(
        &fx,
        QuerySurfaceRequest {
            surface: "outline".into(),
            path: Some("src/a.rs".into()),
            ..Default::default()
        },
    );
    assert!(resp.outline.iter().any(|o| o.name == "alpha"));
}

#[test]
fn outline_line_ranges_and_skeleton() {
    let fx = common::indexed_fixture();
    let resp = execute(
        &fx,
        QuerySurfaceRequest {
            surface: "outline".into(),
            path: Some("src/a.rs".into()),
            budget: Some(800),
            ..Default::default()
        },
    );
    let alpha = resp
        .outline
        .iter()
        .find(|o| o.name == "alpha")
        .expect("alpha outline row");
    assert_eq!(alpha.start_line, Some(1));
    assert_eq!(alpha.end_line, Some(3), "alpha fn spans full block");
    let beta = resp
        .outline
        .iter()
        .find(|o| o.name == "beta")
        .expect("beta outline row");
    assert_eq!(beta.start_line, Some(5));
    assert_eq!(beta.end_line, Some(7), "beta fn spans full block");
    assert!(!resp.skeleton.is_empty());
    assert!(resp.skeleton.starts_with("src/a.rs:"));
    assert!(
        resp.skeleton.contains("alpha 1-3"),
        "skeleton={}",
        resp.skeleton
    );
    assert!(
        resp.skeleton.contains("beta 5-7"),
        "skeleton={}",
        resp.skeleton
    );
}

#[test]
fn outline_budget_one_keeps_skeleton_inline() {
    let fx = common::indexed_fixture();
    let req = QuerySurfaceRequest {
        surface: "outline".into(),
        path: Some("src/a.rs".into()),
        budget: Some(1),
        ..Default::default()
    };
    let text = body(&fx, &req, 1);
    assert!(
        text.starts_with("src/a.rs:"),
        "budget=1 outline should return skeleton shell, got: {text}"
    );
    assert!(text.contains("alpha"));
}

#[test]
fn context_surface() {
    let fx = common::indexed_fixture();
    let resolver = ExpandResolver::new(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let resp = execute(&fx, named("context", "alpha", 800));
    let capsule = resp.capsule.as_ref().expect("context capsule");
    assert!(
        capsule.as_object().is_some_and(|o| !o.is_empty()),
        "context capsule must carry assembled content"
    );
    assert!(
        !resp.refs_footer.is_empty(),
        "context must emit an evidence footer"
    );
    for footer_ref in &resp.refs_footer {
        let gz = GzRef::parse(footer_ref).unwrap();
        assert!(
            !resolver.resolve(&gz, footer_ref).unwrap().bytes.is_empty(),
            "context footer ref must expand: {footer_ref}"
        );
    }
    // The capsule itself must reference expandable evidence; a broken context
    // assembler that drops destinations fails here instead of passing on
    // capsule-presence alone.
    let capsule_text = serde_json::to_string(capsule).unwrap();
    assert!(
        resp.refs_footer
            .iter()
            .any(|r| capsule_text.contains(r.as_str()))
            || capsule_text.contains("gz://"),
        "capsule must carry expandable evidence refs"
    );
}

#[test]
fn hot_tier_c_absence() {
    let fx = common::indexed_fixture();
    let resp = execute(&fx, req("hot"));
    assert_eq!(resp.coverage.tier_c, 0.0);
    assert!(resp.absence_certificate.is_some() || resp.rows.is_empty());
}

#[test]
fn changes_tier_c_gate() {
    let fx = common::indexed_fixture();
    let resp = execute(&fx, req("changes"));
    assert_eq!(resp.coverage.tier_c, 0.0);
}

#[test]
fn word_surface() {
    let fx = common::indexed_fixture();
    let resp = execute(&fx, named("word", "alpha", 50));
    assert!(!resp.hits.is_empty());
}

#[test]
fn search_dedup_sha() {
    let fx = common::indexed_fixture();
    let resolver = ExpandResolver::new(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let resp = execute(&fx, named("search", "alpha", 50));
    let mut seen = std::collections::HashSet::new();
    for h in &resp.hits {
        assert!(seen.insert(h.content_sha256.clone()));
        let gz = GzRef::parse(&h.evidence_ref).unwrap();
        let expanded = resolver.resolve(&gz, &h.evidence_ref).unwrap();
        assert_eq!(h.content_sha256, ContentHash::of(&expanded.bytes).to_hex());
    }
}

#[test]
fn search_hits_carry_matched_line_snippets_not_full_payloads() {
    let fx = common::indexed_fixture();
    let resolver = ExpandResolver::new(&fx.store_root, Some(&fx.repo_root)).unwrap();
    let resp = execute(&fx, named("search", "alpha", 50));
    let hit = resp
        .hits
        .iter()
        .find(|h| h.label == "alpha")
        .expect("alpha symbol hit");
    assert!(
        hit.snippet.contains("fn alpha"),
        "snippet must contain the matched line, got: {}",
        hit.snippet
    );
    let full = resolver
        .resolve(&GzRef::parse(&hit.evidence_ref).unwrap(), &hit.evidence_ref)
        .unwrap();
    assert!(
        hit.snippet.len() <= 240 + '…'.len_utf8(),
        "snippet must stay snippet-sized"
    );
    assert!(
        !full.bytes.is_empty(),
        "full payload must stay reachable via evidence_ref"
    );
    assert!(
        hit.snippet.lines().count() <= 3,
        "snippet is matched line(s) plus ~1 line context, got: {}",
        hit.snippet
    );
}

#[test]
fn whole_blob_path_hits_have_empty_snippet() {
    let fx = common::indexed_fixture();
    let resp = execute(&fx, named("search", "a.rs", 50));
    for h in resp
        .hits
        .iter()
        .filter(|h| h.evidence_ref.ends_with("#B0-0"))
    {
        assert!(
            h.snippet.is_empty(),
            "whole-blob refs must not inline content, got: {}",
            h.snippet
        );
    }
}

#[test]
fn evidence_missing_is_error_variant() {
    let err = QuerySurfaceError::EvidenceMissing;
    assert_eq!(err.to_string(), "EVIDENCE_MISSING");
}

#[test]
fn every_surface_has_coverage_footer() {
    let fx = common::indexed_fixture();
    for surface in all_surface_coverage_probes() {
        let resp = execute(&fx, coverage_probe(surface));
        assert!(resp.coverage.tier_a >= 0.0 && resp.coverage.tier_a <= 1.0);
        assert!(resp.coverage.snapshot_id > 0);
    }
}

#[test]
fn budget_one_spills_heavy_surfaces() {
    let fx = common::indexed_fixture();
    for (surface, query) in [("context", "alpha"), ("search", "alpha")] {
        let req = QuerySurfaceRequest {
            surface: surface.into(),
            query: Some(query.into()),
            budget: Some(1),
            ..Default::default()
        };
        let text = body(&fx, &req, 1);
        assert!(text.starts_with("q:"), "{surface}: {text}");
    }
}

#[test]
fn budget_one_preserves_symbol_not_found_error() {
    let fx = common::indexed_fixture();
    let req = QuerySurfaceRequest {
        surface: "symbol".into(),
        name: Some("does_not_exist".into()),
        query: Some("does_not_exist".into()),
        budget: Some(1),
        ..Default::default()
    };
    let text = body(&fx, &req, 1);
    assert!(
        !text.starts_with("q:"),
        "budget=1 error response must remain visible, got: {text}"
    );
    let value: serde_json::Value =
        serde_json::from_str(&text).expect("error response remains JSON");
    assert_eq!(
        value.get("error").and_then(|v| v.as_str()),
        Some("SYMBOL_NOT_FOUND")
    );
    assert!(
        value.get("accounting").is_some(),
        "visible error response keeps accounting: {text}"
    );
}

#[test]
fn callpath_surface_returns_shortest_call_chain() {
    let fx = common::indexed_fixture();
    let resp = execute(
        &fx,
        QuerySurfaceRequest {
            surface: "callpath".into(),
            name: Some("gamma".into()),
            query: Some("beta".into()),
            budget: Some(800),
            ..Default::default()
        },
    );
    let hops: Vec<_> = resp
        .edges
        .iter()
        .map(|e| (e.from.as_deref().unwrap_or(""), e.to.as_str()))
        .collect();
    assert_eq!(hops, vec![("gamma", "alpha"), ("alpha", "beta")]);
    assert!(resp.edges.iter().all(|e| !e.evidence_ref.is_empty()));
}

#[test]
fn reading_set_surface_ranks_target_callers_and_callees() {
    let fx = common::indexed_fixture();
    let resp = execute(&fx, named("reading_set", "beta", 800));
    assert_eq!(resp.surface, "reading_set");
    assert_eq!(resp.symbol.as_deref(), Some("beta"));
    let names: Vec<_> = resp
        .reading_set
        .iter()
        .map(|e| (e.kind.as_str(), e.target.as_str()))
        .collect();
    assert!(names.contains(&("target", "beta")), "{names:?}");
    assert!(names.contains(&("caller", "alpha")), "{names:?}");
    assert!(names.contains(&("caller", "gamma")), "{names:?}");
    assert!(resp.reading_set.iter().all(|e| !e.reason.is_empty()));
    assert!(resp.reading_set.iter().all(|e| !e.evidence_ref.is_empty()));
    let alpha = resp
        .reading_set
        .iter()
        .find(|e| e.target == "alpha")
        .unwrap();
    let gamma = resp
        .reading_set
        .iter()
        .find(|e| e.target == "gamma")
        .unwrap();
    assert!(alpha.rank <= gamma.rank);
    assert_eq!(alpha.depth, Some(1));
    assert_eq!(gamma.depth, Some(2));
}

#[test]
fn reading_set_emits_closure_confidence_contract() {
    let fx = common::indexed_fixture();
    let resp = execute(&fx, named("reading_set", "beta", 800));
    let closure = resp
        .reading_set_closure
        .as_ref()
        .expect("reading set closure contract");
    assert_eq!(closure.confidence_level, "structural");
    assert!(
        closure.guarantee.contains("statically-resolvable"),
        "{}",
        closure.guarantee
    );
    assert!(
        closure
            .sound_when
            .iter()
            .any(|rule| rule.contains("indexed static call/import/reference edges")),
        "{:?}",
        closure.sound_when
    );
    assert!(
        closure
            .out_of_scope
            .iter()
            .any(|rule| rule.contains("reflection")),
        "{:?}",
        closure.out_of_scope
    );
    assert!(
        resp.reading_set
            .iter()
            .all(|entry| !entry.confidence_level.is_empty()),
        "{:?}",
        resp.reading_set
    );
}

#[test]
fn reading_set_mutation_style_static_caller_chain_is_inside_closure() {
    let fx = common::indexed_fixture();
    let resp = execute(&fx, named("reading_set", "beta", 800));
    let included: std::collections::HashSet<_> = resp
        .reading_set
        .iter()
        .map(|entry| entry.target.as_str())
        .collect();

    // Mutation model: a breaking signature change to beta can require edits in
    // the declaration, the direct caller, and transitive static callers. The
    // reading set must contain that whole statically-resolvable repair set.
    for expected in ["beta", "alpha", "gamma", "function_foo"] {
        assert!(
            included.contains(expected),
            "missing {expected}: {included:?}"
        );
    }
}

#[test]
fn durable_accounting_surfaces_are_declared_and_enforced() {
    let fx = common::indexed_fixture();
    let mut seen = std::collections::BTreeSet::new();
    for surface in PREVENTED_READ_ACCOUNTING_SURFACES {
        let query = if *surface == "reading_set" {
            "beta"
        } else {
            "alpha"
        };
        let resp = execute(&fx, named(surface, query, 800));
        let accounting = resp.accounting.as_ref().expect("durable accounting");
        assert!(
            seen.insert(accounting.scope.clone()),
            "duplicate accounting scope"
        );
        assert_eq!(accounting.schema_version, 1);
    }
    assert_eq!(
        seen,
        [
            "orient_symbol".to_string(),
            "reading_set_closure".to_string()
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn orient_and_reading_set_report_prevented_read_accounting() {
    let fx = common::indexed_fixture();

    let orient = execute(&fx, named("symbol", "alpha", 800));
    let orient_accounting = orient.accounting.as_ref().expect("orient accounting");
    assert_eq!(orient_accounting.scope, "orient_symbol");
    assert!(orient_accounting.indexed_files >= orient_accounting.required_files);
    assert_eq!(
        orient_accounting.prevented_files,
        orient_accounting.indexed_files - orient_accounting.required_files
    );
    assert_eq!(
        orient_accounting.prevented_bytes,
        orient_accounting.indexed_bytes - orient_accounting.required_bytes
    );

    let reading = execute(&fx, named("reading_set", "beta", 800));
    let reading_accounting = reading.accounting.as_ref().expect("reading_set accounting");
    assert_eq!(reading_accounting.scope, "reading_set_closure");
    assert!(
        reading_accounting.required_files > 0,
        "{reading_accounting:?}"
    );
    assert_eq!(
        reading_accounting.prevented_bytes,
        reading_accounting.indexed_bytes - reading_accounting.required_bytes
    );
}
