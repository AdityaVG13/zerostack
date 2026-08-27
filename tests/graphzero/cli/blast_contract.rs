mod common;

use common::blast_contract::{LOAD_CONFIG_INTENT, PARSE_REF_INTENT, blast_capsule};

#[test]
fn break_sites_have_gz_evidence_and_at_least_three() {
    let cap = blast_capsule(PARSE_REF_INTENT);
    assert!(
        cap.break_sites.len() >= 3,
        "expected >=3 break sites, got {}",
        cap.break_sites.len()
    );
    for site in &cap.break_sites {
        assert!(
            site.evidence_ref.starts_with("gz://"),
            "bad evidence: {}",
            site.evidence_ref
        );
    }
}

#[test]
fn covering_tests_include_cli_rs() {
    let cap = blast_capsule(PARSE_REF_INTENT);
    assert!(
        cap.covering_tests
            .iter()
            .any(|t| t.path_hint.contains("cli.rs")),
        "covering_tests: {:?}",
        cap.covering_tests
    );
}

#[test]
fn coverage_footer_and_certificate_gaps_when_tier_b_partial() {
    let cap = blast_capsule(PARSE_REF_INTENT);
    assert!(cap.coverage.tier_b_percent < 100.0 || cap.coverage.tier_a_percent <= 100.0);
    let gaps = cap.certificate.get("gaps").and_then(|v| v.as_array());
    if cap.coverage.tier_b_percent < 100.0 {
        assert!(
            gaps.map(|g| !g.is_empty()).unwrap_or(false),
            "expected tier-B gap"
        );
    }
    assert!(cap.certificate.get("tier_b_pct").is_some());
}

#[test]
fn break_sites_order_stable_across_runs() {
    let a = blast_capsule(PARSE_REF_INTENT);
    let b = blast_capsule(PARSE_REF_INTENT);
    assert_eq!(a.break_sites, b.break_sites);
}

#[test]
fn silent_risk_includes_string_key() {
    let cap = blast_capsule(LOAD_CONFIG_INTENT);
    assert!(
        cap.silent_risk.iter().any(|r| r.kind == "string_key"),
        "silent_risk: {:?}",
        cap.silent_risk
    );
    for r in &cap.silent_risk {
        assert!(r.evidence_ref.starts_with("gz://"));
    }
}

#[test]
fn blast_reports_unaffected_file_accounting() {
    let cap = blast_capsule(PARSE_REF_INTENT);
    assert_eq!(cap.accounting.scope, "blast_unaffected_files");
    assert!(cap.accounting.indexed_files >= cap.accounting.required_files);
    assert_eq!(
        cap.accounting.prevented_files,
        cap.accounting.indexed_files - cap.accounting.required_files
    );
    assert_eq!(
        cap.accounting.prevented_bytes,
        cap.accounting.indexed_bytes - cap.accounting.required_bytes
    );
}

#[test]
fn covering_tests_differ_for_symbols_with_disjoint_dependencies() {
    let parse = blast_capsule(PARSE_REF_INTENT);
    let config = blast_capsule(LOAD_CONFIG_INTENT);
    let parse_paths: std::collections::BTreeSet<&str> = parse
        .covering_tests
        .iter()
        .map(|t| t.path_hint.as_str())
        .collect();
    let config_paths: std::collections::BTreeSet<&str> = config
        .covering_tests
        .iter()
        .map(|t| t.path_hint.as_str())
        .collect();
    assert_ne!(
        parse_paths, config_paths,
        "covering_tests must be per-symbol, got identical sets: {parse_paths:?}"
    );
    assert!(
        !parse_paths.is_empty(),
        "parse_ref should keep at least one covering test"
    );
    assert!(
        !config_paths.contains("tests/cli.rs"),
        "load_config has no dependency on tests/cli.rs: {config_paths:?}"
    );
}
