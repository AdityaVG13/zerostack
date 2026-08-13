use std::fmt::Write;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;
use sha2::{Digest, Sha256};
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

fn native_matrix() -> Value {
    serde_json::from_slice(
        &fs::read(
            root().join("conformance/models/packaged-codemode-native-matrix-2026-08-12.json"),
        )
        .expect("packaged native matrix"),
    )
    .expect("packaged native matrix JSON")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            write!(hex, "{byte:02x}").expect("writing to String cannot fail");
            hex
        })
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
    assert_eq!(
        dispatches.load(Ordering::SeqCst),
        0,
        "worker dispatch occurred"
    );
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

#[test]
fn packaged_native_matrix_binds_three_hosts_and_targeted_inputs() {
    let matrix = native_matrix();
    assert_eq!(
        matrix["schema"],
        "zerostack.packaged_codemode.native_matrix.v1"
    );
    assert_eq!(
        matrix["assembly_manifest_digest"],
        evidence()["assembly"]["manifestDigest"]
    );

    let command = matrix["verifier"]["exact_command"]
        .as_str()
        .expect("exact verifier command");
    assert!(command.contains("--test program_assembly"));
    assert!(command.contains("--test canonical_mixed"));
    assert!(command.contains("--test one_process"));
    assert!(!command.contains("--workspace"));

    let expected_platforms = [
        "native-macos-aarch64",
        "native-linux-aarch64",
        "native-windows-x86_64-msvc",
    ];
    let receipts = matrix["platform_receipts"]
        .as_array()
        .expect("platform receipts");
    assert_eq!(receipts.len(), expected_platforms.len());
    for (receipt, expected_platform) in receipts.iter().zip(expected_platforms) {
        assert_eq!(receipt["platform_profile"], expected_platform);
        assert_eq!(receipt["result"], "passed_native");
        assert!(receipt["failure_code"].is_null());
        for head in receipt["source_repository_heads"]
            .as_object()
            .expect("source heads")
            .values()
        {
            let head = head.as_str().expect("source head string");
            assert_eq!(head.len(), 40);
            assert!(head.bytes().all(|byte| byte.is_ascii_hexdigit()));
        }
        for digest in receipt["output_artifact_hashes"]
            .as_object()
            .expect("artifact hashes")
            .values()
        {
            let digest = digest.as_str().expect("artifact digest string");
            assert_eq!(digest.len(), 64);
            assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
        }
    }

    for (path, expected) in matrix["input_fixture_hashes"]
        .as_object()
        .expect("input fixture hashes")
    {
        let bytes = fs::read(root().join(path)).expect("bound fixture remains tracked");
        assert_eq!(
            sha256_hex(&bytes),
            expected.as_str().expect("fixture digest")
        );
    }
}
