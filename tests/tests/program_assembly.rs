use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;
use zero_abi::{
    AssemblyFailureCodeV1, AssemblyManifestV1, DigestV1, EngineIdentity,
    validate_assembly_pre_dispatch_v1,
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("shared test crate is nested under repository root")
        .to_path_buf()
}

fn evidence() -> Value {
    serde_json::from_slice(
        &fs::read(root().join("conformance/models/program-aggregate-2026-08-11.json"))
            .expect("program evidence"),
    )
    .expect("program evidence JSON")
}

fn manifest() -> AssemblyManifestV1 {
    serde_json::from_slice(
        &fs::read(root().join("conformance/models/program-assembly-2026-08-11.json"))
            .expect("program assembly"),
    )
    .expect("program assembly JSON")
}

#[test]
fn generated_program_assembly_binds_every_packaged_worker_and_report() {
    let evidence = evidence();
    let manifest = manifest();
    manifest.validate().expect("assembly manifest validates");
    assert_eq!(
        manifest.digest().expect("assembly digest").to_hex(),
        evidence["assembly"]["manifestDigest"]
            .as_str()
            .expect("bound manifest digest")
    );
    assert_eq!(
        manifest.abi_contract_digest.to_hex(),
        evidence["assembly"]["abiContractDigest"]
            .as_str()
            .expect("bound ABI digest")
    );
    assert_eq!(
        manifest.aggregate_capability_catalog_digest.to_hex(),
        evidence["plan"]["sha256"].as_str().expect("plan digest")
    );

    let expected = [
        ("fszero", EngineIdentity::FsZero),
        ("graphzero", EngineIdentity::GraphZero),
        ("tokenzero", EngineIdentity::TokenZero),
    ];
    for ((key, engine), worker) in expected.into_iter().zip(&manifest.workers) {
        assert_eq!(worker.engine, engine);
        assert_eq!(
            worker.artifact_digest.to_hex(),
            evidence["engines"][key]["binary"]["sha256"]
                .as_str()
                .expect("artifact digest")
        );
        let artifact = manifest
            .linked_artifacts
            .iter()
            .find(|artifact| artifact.artifact_digest == worker.artifact_digest)
            .expect("worker artifact is linked");
        assert_eq!(
            artifact.source_revision,
            evidence["engines"][key]["sourceHead"]
                .as_str()
                .expect("source head")
        );
        let report_path = evidence["engines"][key]["worker"]["report"]
            .as_str()
            .expect("tracked report path");
        assert!(report_path.starts_with("tests/data/program-aggregate-reports/"));
        let report: Value = serde_json::from_slice(
            &fs::read(root().join(report_path)).expect("tracked report remains available in CI"),
        )
        .expect("tracked report JSON");
        assert_eq!(
            worker.capability_catalog_digest.to_hex(),
            report["provenance"]["checks_digest"]
                .as_str()
                .expect("capability checks digest")
        );
    }

    validate_assembly_pre_dispatch_v1(
        &manifest,
        &manifest.expectation().expect("manifest expectation"),
    )
    .expect("matching assembly admits dispatch");
}

fn reject_before_dispatch(
    manifest: &AssemblyManifestV1,
    mutate: impl FnOnce(&mut zero_abi::AssemblyExpectationV1),
    expected_code: AssemblyFailureCodeV1,
) {
    let dispatches = AtomicUsize::new(0);
    let mut expected = manifest.expectation().expect("manifest expectation");
    mutate(&mut expected);
    let validation = validate_assembly_pre_dispatch_v1(manifest, &expected);
    if validation.is_ok() {
        dispatches.fetch_add(1, Ordering::SeqCst);
    }
    assert_eq!(validation.unwrap_err().code(), expected_code);
    assert_eq!(dispatches.load(Ordering::SeqCst), 0, "worker dispatch occurred");
}

#[test]
fn assembly_identity_mutants_fail_typed_before_worker_dispatch() {
    let manifest = manifest();
    reject_before_dispatch(
        &manifest,
        |expected| expected.manifest_digest = DigestV1::ZERO,
        AssemblyFailureCodeV1::ManifestDigestMismatch,
    );
    reject_before_dispatch(
        &manifest,
        |expected| expected.workers[0].artifact_digest = DigestV1::ZERO,
        AssemblyFailureCodeV1::WorkerDigestMismatch,
    );
    reject_before_dispatch(
        &manifest,
        |expected| expected.workers[1].capability_catalog_digest = DigestV1::ZERO,
        AssemblyFailureCodeV1::CapabilityCatalogDigestMismatch,
    );
}
