//! Phase 0/3 greenfield identity smokes. Subject ≠ Oracle. MCP registry
//! labels are forbidden. Missing drivers must be `None`, not a deleted path.

use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use serde_json::Value;
use tokenzero_test_support::{
    CrashBoundary, CrashWindowKind, ExecutionEnvelope, FAILURE_BUNDLE_SCHEMA,
    FAILURE_FIRST_DIVERGENCE_JSONPTR, FORBIDDEN_MCP_ENGINE_IDENTITY, FORBIDDEN_MCP_REGISTRY_ENGINE,
    GauntletEngineIdentity, GauntletIdentityPair, GauntletOracle, SPEC_TAG_WIRES, SUBJECT_IDENTITY,
    ScenarioAgreement, SpecTagClass, assert_distinct, compare_bytes,
    is_forbidden_gauntlet_identity, scenario,
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("TokenZero repo root")
}

#[test]
fn identity_guard_rejects_self_comparison() {
    let subject = GauntletEngineIdentity::Subject.as_str();
    assert_eq!(subject, SUBJECT_IDENTITY);
    assert!(
        catch_unwind(|| assert_distinct(subject, subject)).is_err(),
        "subject==oracle must panic (K-9 self-comparison)"
    );
    let oracle = GauntletOracle::Spec.as_str();
    assert!(
        catch_unwind(|| assert_distinct(oracle, oracle)).is_err(),
        "oracle==oracle must panic"
    );
    GauntletIdentityPair::new(GauntletOracle::Spec).assert_distinct();
}

#[test]
fn identity_guard_rejects_forbidden_mcp_identity() {
    let oracle = GauntletOracle::Spec.as_str();
    for forbidden in [FORBIDDEN_MCP_ENGINE_IDENTITY, FORBIDDEN_MCP_REGISTRY_ENGINE] {
        assert!(is_forbidden_gauntlet_identity(forbidden));
        assert!(
            catch_unwind(|| assert_distinct(forbidden, oracle)).is_err(),
            "{forbidden} must not be usable as gauntlet Subject"
        );
        assert_ne!(SUBJECT_IDENTITY, forbidden);
        assert_ne!(oracle, forbidden);
    }
}

#[test]
fn mixed_oracles_are_distinct_from_subject_and_each_other() {
    let subject = SUBJECT_IDENTITY;
    let mut seen = HashSet::new();
    assert!(seen.insert(subject));
    assert_eq!(GauntletOracle::ALL.len(), 6);
    for mode in GauntletOracle::ALL {
        let oracle = mode.as_str();
        assert!(!oracle.is_empty(), "{mode} identity empty");
        assert_ne!(oracle, subject, "{mode} collided with Subject");
        assert!(
            !is_forbidden_gauntlet_identity(oracle),
            "{mode} used a forbidden MCP identity"
        );
        assert!(
            seen.insert(oracle),
            "duplicate oracle identity string: {oracle}"
        );
        GauntletIdentityPair::new(*mode).assert_distinct();
    }
}

#[test]
fn artifact_id_ignores_run_id() {
    let pair = GauntletIdentityPair::new(GauntletOracle::Spec);
    let mut left = ExecutionEnvelope::from_pair("spec-smoke", 7, pair, vec!["a".into()]);
    let mut right = left.clone();
    left.run_id = Some("run-1".into());
    right.run_id = Some("run-2".into());
    assert_eq!(left.artifact_id(), right.artifact_id());
    let mut other = right.clone();
    other.scenario_id = "other-scenario".into();
    assert_ne!(left.artifact_id(), other.artifact_id());
    left.assert_engine_identities(pair);
}

#[test]
fn failure_bundle_first_divergence_jsonptr_and_provenance() {
    let pair = GauntletIdentityPair::new(GauntletOracle::Spec);
    let envelope = ExecutionEnvelope::from_pair("byte-div", 9, pair, vec!["abc".into()]);
    envelope.assert_engine_identities(pair);
    compare_bytes(
        &envelope,
        "equal",
        "cargo test -p tokenzero-test-support --test gauntlet_oracle_smoke",
        b"abc",
        b"abc",
    )
    .expect("equal bytes must not emit a bundle");
    let bundle = compare_bytes(
        &envelope,
        "abc-vs-abX",
        "cargo test -p tokenzero-test-support --test gauntlet_oracle_smoke -- failure_bundle_first_divergence_jsonptr_and_provenance",
        b"abc",
        b"abX",
    )
    .expect_err("abc vs abX must diverge");
    assert_eq!(bundle.schema, FAILURE_BUNDLE_SCHEMA);
    assert_eq!(
        bundle.first_divergence_jsonptr(),
        FAILURE_FIRST_DIVERGENCE_JSONPTR
    );
    let first = bundle
        .dereference(FAILURE_FIRST_DIVERGENCE_JSONPTR)
        .expect("jsonptr must resolve");
    assert_eq!(first["byte_offset"], 2);
    assert_eq!(first["subject_byte"], "0x63");
    assert_eq!(first["oracle_byte"], "0x58");
    assert_eq!(bundle.provenance.seed, 9);
    assert_eq!(bundle.provenance.fixture_id, "abc-vs-abX");
    assert!(
        bundle
            .provenance
            .repro_command
            .contains("gauntlet_oracle_smoke")
    );
    assert!(!bundle.provenance.schedule_fingerprint.is_empty());
    assert_eq!(
        bundle.provenance.git_sha,
        "862e3e682cb8aee0e150c1cb0b116cb2e23a44e2"
    );
    assert_ne!(
        bundle.engines.subject_identity,
        bundle.engines.oracle_identity
    );
}

#[test]
fn scenario_both_error_is_agreement() {
    let pair = GauntletIdentityPair::new(GauntletOracle::Spec);
    match scenario(
        "both-err",
        pair,
        || Err::<u8, _>("subject-err"),
        || Err("oracle-err"),
    ) {
        ScenarioAgreement::BothErr { subject, oracle } => {
            assert_eq!(subject, "subject-err");
            assert_eq!(oracle, "oracle-err");
        }
        ScenarioAgreement::BothOk(_) => panic!("both-error must be agreement, not Ok"),
    }
}

#[test]
fn scenario_one_error_one_ok_is_hard_fail() {
    let pair = GauntletIdentityPair::new(GauntletOracle::Spec);
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            scenario("divergent-ok", pair, || Ok(1u8), || Err("oracle-err"));
        }))
        .is_err(),
        "subject Ok / oracle Err must panic"
    );
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            scenario(
                "divergent-err",
                pair,
                || Err::<u8, _>("subject-err"),
                || Ok(()),
            );
        }))
        .is_err(),
        "subject Err / oracle Ok must panic"
    );
}

#[test]
fn spec_tag_catalog_does_not_mark_ambiguous_as_wired() {
    let verifiable = SPEC_TAG_WIRES
        .iter()
        .filter(|row| row.class == SpecTagClass::Verifiable)
        .count();
    let ambiguous = SPEC_TAG_WIRES
        .iter()
        .filter(|row| row.class == SpecTagClass::Ambiguous)
        .count();
    assert_eq!(verifiable, 33, "Phase 2 Verifiable count");
    assert_eq!(ambiguous, 7, "Phase 2 Ambiguous count");
    let wired = SPEC_TAG_WIRES.iter().filter(|row| row.is_wired()).count();
    assert_eq!(
        wired, 21,
        "Phase 4 live-wired Verifiable count (METRIC-001)"
    );
    let root = repo_root();
    for row in SPEC_TAG_WIRES {
        if row.class == SpecTagClass::Ambiguous {
            assert!(
                !row.is_wired(),
                "{} is Ambiguous and must stay uncovered",
                row.tag
            );
            assert!(row.existing_driver.is_none());
        }
        if let Some(driver) = row.existing_driver {
            assert!(
                Path::new(&root.join(driver)).exists(),
                "{} driver {} missing on disk (use None, do not cite a deleted path)",
                row.tag,
                driver
            );
        }
    }
}

#[test]
fn crash_boundary_drivers_are_live_in_process_not_subprocess_armed() {
    let root = repo_root();
    assert_eq!(CrashBoundary::ALL.len(), 8);
    for boundary in CrashBoundary::ALL {
        let driver = boundary.existing_driver().unwrap_or_else(|| {
            panic!(
                "{} existing_driver must name a live crash-window test",
                boundary.as_str()
            )
        });
        match boundary {
            CrashBoundary::PersistLockConcurrentWriters
            | CrashBoundary::PersistLockTmpSweep
            | CrashBoundary::AfterWalTornTailKeepsComplete => {
                assert!(
                    !boundary.is_subprocess_armed(),
                    "{} is not a Pattern 65 abort window",
                    boundary.as_str()
                );
            }
            _ => {
                assert!(
                    boundary.is_subprocess_armed(),
                    "{} must arm Pattern 65 subprocess abort",
                    boundary.as_str()
                );
                assert_eq!(driver.kind, CrashWindowKind::SubprocessAbort);
            }
        }
        assert!(
            root.join(driver.path).exists(),
            "{} driver {} missing on disk",
            boundary.as_str(),
            driver.path
        );
        let census = boundary.deleted_driver_census();
        assert!(
            !root.join(census.path).exists(),
            "{} census path {} reappeared; keep existing_driver on the live file",
            boundary.as_str(),
            census.path
        );
        assert_ne!(
            driver.path,
            census.path,
            "{} must not cite the deleted census path as live",
            boundary.as_str()
        );
    }
}

#[test]
fn hub_002_no_fszero_graphzero_crate_deps() {
    let root = repo_root();
    let mut tomls = vec![root.join("Cargo.toml")];
    let crates = root.join("crates");
    for entry in std::fs::read_dir(&crates).expect("crates/") {
        let entry = entry.expect("crate dir");
        let cargo = entry.path().join("Cargo.toml");
        if cargo.is_file() {
            tomls.push(cargo);
        }
    }
    assert!(
        tomls.len() > 1,
        "expected workspace + crate Cargo.toml files"
    );
    for path in &tomls {
        for (idx, line) in std::fs::read_to_string(path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
            .lines()
            .enumerate()
        {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                continue;
            }
            let lower = trimmed.to_ascii_lowercase();
            assert!(
                !lower.contains("fszero"),
                "{}:{} imports FSZero: {trimmed}",
                path.display(),
                idx + 1
            );
            assert!(
                !lower.contains("graphzero"),
                "{}:{} imports GraphZero: {trimmed}",
                path.display(),
                idx + 1
            );
        }
    }

    // Product src: rust imports / path-includes, not URI/comment/field names.
    fn walk_rs(dir: &std::path::Path, visit: &mut dyn FnMut(&std::path::Path, &str, usize)) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let entry = entry.expect("src entry");
            let path = entry.path();
            if path.is_dir() {
                walk_rs(&path, visit);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            for (idx, line) in std::fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
                .lines()
                .enumerate()
            {
                visit(&path, line, idx);
            }
        }
    }
    for entry in std::fs::read_dir(&crates).expect("crates/") {
        let src = entry.expect("crate dir").path().join("src");
        if !src.is_dir() {
            continue;
        }
        walk_rs(&src, &mut |path, line, idx| {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with("///") || trimmed.starts_with("//!")
            {
                return;
            }
            let lower = trimmed.to_ascii_lowercase();
            let smuggled = lower.contains("use fszero")
                || lower.contains("use graphzero")
                || lower.contains("extern crate fszero")
                || lower.contains("extern crate graphzero")
                || lower.contains("fszero::")
                || lower.contains("graphzero::");
            assert!(
                !smuggled,
                "{}:{} smuggles sibling engine import: {trimmed}",
                path.display(),
                idx + 1
            );
            if path
                .components()
                .any(|c| c.as_os_str() == "tokenzero-kernel")
            {
                assert!(
                    !lower.contains("tokenzero-engine/src"),
                    "{}:{} kernel must not #[path]-include engine: {trimmed}",
                    path.display(),
                    idx + 1
                );
            }
        });
    }
}

#[test]
fn provider_tokenizer_fixture_and_cli_golden_still_exist() {
    let root = repo_root();
    let goldens = root.join("tests/engine/fixtures/provider-tokenizer-goldens.json");
    let bytes = std::fs::read(&goldens).expect("provider-tokenizer goldens");
    let v: Value = serde_json::from_slice(&bytes).expect("goldens json");
    assert_eq!(v["schema"], "tokenzero.tokenizer-goldens.v1");
    assert!(
        v["entries"]
            .as_array()
            .map(|e| !e.is_empty())
            .unwrap_or(false),
        "tokenizer goldens entries empty"
    );

    let golden = root.join("tests/cli/golden/cli/read_json.golden");
    assert!(
        golden.is_file() && golden.metadata().map(|m| m.len() > 0).unwrap_or(false),
        "Self-Oracle CLI golden missing"
    );

    let toolchain = std::fs::read_to_string(root.join("rust-toolchain.toml")).unwrap();
    assert!(toolchain.contains("clippy"));
    assert!(toolchain.contains("nightly-2026-05-31"));

    assert_eq!(
        GauntletOracle::Spec.as_str(),
        "GauntletOracle::Spec::tokenzero-spec@HEAD-fb73416"
    );
    assert!(
        SUBJECT_IDENTITY.contains("862e3e682cb8aee0e150c1cb0b116cb2e23a44e2"),
        "Subject identity stays Self-oracle prior-commit 862e3e6, not retargeted to HEAD"
    );
}

#[test]
fn embedded_surface_matrix_byte_matches_gauntlet_workspace_when_present() {
    let fixture = repo_root()
        .join("crates/tokenzero-test-support/src/fixtures/supported_surface_matrix.toml");
    let fixture_bytes = std::fs::read(&fixture).expect("embedded fixture");
    let workspace = repo_root()
        .parent()
        .expect("sibling")
        .join("TokenZero__gauntlet_workspace/docs/contracts/supported_surface_matrix.toml");
    if workspace.is_file() {
        let workspace_bytes = std::fs::read(&workspace).expect("workspace matrix");
        assert_eq!(
            fixture_bytes, workspace_bytes,
            "workspace supported_surface_matrix.toml must byte-match the TokenZero fixture"
        );
    }
}
