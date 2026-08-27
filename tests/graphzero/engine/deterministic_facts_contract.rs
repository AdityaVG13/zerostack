//! RACC deterministic-facts-only contract guard (bead vz89.9).
//!
//! Representative operators run twice per process (and across two independently
//! indexed copies of the same sources) and must yield byte-identical canonical
//! fact output. Rust seeds every HashMap differently per instance, so a second
//! in-process run exposes unordered-iteration leakage; the second index exposes
//! absolute temp paths, timestamps, and random ids baked into facts.

mod common;

use graphzero_engine::blast::{blast_radius, retrieval_neighborhood};
use graphzero_engine::deterministic_facts::{audit_facts, audit_value, canonical_facts};
use graphzero_engine::query_surface::{QuerySurfaceRequest, QuerySurfaceRouter};
use graphzero_engine::rewrite_closure::{PropagationPolicy, Relation, rewrite_closure};

const WORKSPACE: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/app\", \"crates/shared\"]\nresolver = \"3\"\n",
    ),
    (
        "crates/shared/src/lib.rs",
        "pub fn shared_contract(input: &str) -> usize {\n    input.len()\n}\n\npub fn helper() -> usize {\n    shared_contract(\"x\")\n}\n",
    ),
    (
        "crates/app/src/main.rs",
        "use shared::shared_contract;\n\nfn run() -> usize {\n    shared_contract(\"hello\")\n}\n\nfn main() {\n    let _ = run();\n}\n",
    ),
    (
        "crates/app/src/util.rs",
        "use shared::helper;\n\npub fn call_helper() -> usize {\n    helper()\n}\n",
    ),
];

fn surface_requests() -> Vec<QuerySurfaceRequest> {
    vec![
        QuerySurfaceRequest {
            surface: "symbol".into(),
            name: Some("shared_contract".into()),
            query: Some("shared_contract".into()),
            budget: Some(40),
            ..Default::default()
        },
        QuerySurfaceRequest {
            surface: "callers".into(),
            name: Some("shared_contract".into()),
            budget: Some(40),
            ..Default::default()
        },
        QuerySurfaceRequest {
            surface: "deps".into(),
            name: Some("shared_contract".into()),
            budget: Some(40),
            ..Default::default()
        },
        QuerySurfaceRequest {
            surface: "outline".into(),
            path: Some("crates/shared/src/lib.rs".into()),
            budget: Some(40),
            ..Default::default()
        },
        QuerySurfaceRequest {
            surface: "reading_set".into(),
            name: Some("shared_contract".into()),
            budget: Some(40),
            ..Default::default()
        },
        QuerySurfaceRequest {
            surface: "search".into(),
            query: Some("shared_contract".into()),
            budget: Some(40),
            ..Default::default()
        },
    ]
}

/// Canonical fact output of every representative operator, for one index.
fn operator_facts(fx: &common::IndexedRepo) -> Vec<(String, String)> {
    let snapshot = &fx.snapshot;
    let mut facts = Vec::new();

    for req in surface_requests() {
        let response = QuerySurfaceRouter::execute(snapshot, &req).unwrap_or_else(|error| {
            panic!("surface {} failed: {error:?}", req.surface);
        });
        facts.push((
            format!("surface:{}", req.surface),
            canonical_facts(&response),
        ));
    }

    let capsule = blast_radius(snapshot, "change signature of shared_contract", 40).unwrap();
    facts.push(("blast_radius".to_string(), canonical_facts(&capsule)));

    let neighborhood =
        retrieval_neighborhood(snapshot, &["shared_contract".to_string()], 3, 100).unwrap();
    facts.push((
        "retrieval_neighborhood".to_string(),
        canonical_facts(&neighborhood),
    ));

    let policy = PropagationPolicy {
        relations: vec![Relation::Calls, Relation::Refs, Relation::Imports],
        ..Default::default()
    };
    let closure = rewrite_closure(snapshot, "shared_contract", &policy).unwrap();
    facts.push(("rewrite_closure".to_string(), canonical_facts(&closure)));

    facts
}

/// Same process, same snapshot, two runs: exposes unordered iteration.
#[test]
fn representative_operators_are_byte_identical_across_repeat_runs() {
    let fx = common::indexed_repo(WORKSPACE);
    let first = operator_facts(&fx);
    let second = operator_facts(&fx);
    assert_eq!(first.len(), second.len());
    for ((name, a), (_, b)) in first.iter().zip(second.iter()) {
        assert_eq!(
            a, b,
            "{name}: canonical fact output differs across identical runs — \
             results are not cacheable (nondeterminism leaked into facts)"
        );
    }
}

/// Two independently indexed copies of identical sources under different temp
/// roots: exposes absolute temp paths, wall-clock stamps, and random ids.
#[test]
fn representative_operators_are_byte_identical_across_independent_indexes() {
    let a = common::indexed_repo(WORKSPACE);
    let b = common::indexed_repo(WORKSPACE);
    assert_ne!(a.repo, b.repo, "fixtures must live under distinct roots");
    for ((name, left), (_, right)) in operator_facts(&a).iter().zip(operator_facts(&b).iter()) {
        assert_eq!(
            left, right,
            "{name}: canonical fact output depends on the index location or \
             creation time — results are not cacheable"
        );
    }
}

/// Every emitted payload must satisfy the fact-kind allowlist and carry no
/// nondeterministic or speculative content.
#[test]
fn representative_operator_payloads_pass_the_fact_contract() {
    let fx = common::indexed_repo(WORKSPACE);
    let snapshot = &fx.snapshot;

    for req in surface_requests() {
        let response = QuerySurfaceRouter::execute(snapshot, &req).unwrap();
        let violations = audit_facts(&response);
        assert!(
            violations.is_empty(),
            "surface {} violated the deterministic-facts contract: {violations:?}",
            req.surface
        );
    }

    let capsule = blast_radius(snapshot, "change signature of shared_contract", 40).unwrap();
    assert!(
        audit_facts(&capsule).is_empty(),
        "{:?}",
        audit_facts(&capsule)
    );

    let neighborhood =
        retrieval_neighborhood(snapshot, &["shared_contract".to_string()], 3, 100).unwrap();
    assert!(
        audit_facts(&neighborhood).is_empty(),
        "{:?}",
        audit_facts(&neighborhood)
    );
}

/// The guard itself must reject each nondeterminism class, otherwise the tests
/// above are vacuous.
#[test]
fn guard_rejects_each_nondeterminism_class() {
    let cases = [
        (
            "timestamp",
            serde_json::json!({"edges": [{"kind": "calls", "observed_at": 1}]}),
        ),
        (
            "date literal",
            serde_json::json!({"reason": "indexed 2026-07-29"}),
        ),
        (
            "absolute temp path",
            serde_json::json!({"target": "/tmp/.tmpAbCd12/repo/src/a.rs"}),
        ),
        (
            "random id",
            serde_json::json!({"session": "9b0e1816-8acc-43d1-972c-5216392fd9bc"}),
        ),
        (
            "speculative claim",
            serde_json::json!({"reason": "this probably does auth"}),
        ),
        (
            "unknown fact kind",
            serde_json::json!({"kind": "needs_refactor"}),
        ),
    ];
    for (label, payload) in cases {
        assert!(
            !audit_value(&payload).is_empty(),
            "guard failed to reject {label}"
        );
    }
}
