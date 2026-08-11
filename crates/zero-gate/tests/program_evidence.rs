//! End-to-end Program evidence assembly: real artifact files on disk, the
//! production assembler, and the `zerostack-program-evidence` CLI.
//!
//! The CLI must seal a verified `AggregateProgramReceiptV1` from collected
//! evidence and must fail closed (nonzero exit) when any engine or evidence
//! class is missing, partial, stale, or digest-mismatched — there is no
//! fixture fallback.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};
use tempfile::TempDir;
use zero_abi::{EngineIdentity, canonical_json, sha256, sha256_hex};
use zero_gate::{
    AggregateProgramReceiptV1, AppliedGcEvidenceV1, EngineEvidenceSourceV1, EngineIdV1,
    EvidenceClassV1, GcProducerEpochV1, GcReport, ProgramEvidenceArtifactV1,
    ProgramEvidenceFailureV1, ProgramEvidenceManifestV1, assemble_program_evidence,
};
use zero_store::{
    GC_RECORD_TYPE_DRY_RUN, GC_SCHEMA_VERSION, GcRunReceipt, GcRunState, gc_contract_digest_hex,
};

const BIN: &str = env!("CARGO_BIN_EXE_zerostack-program-evidence");

fn program_id() -> [u8; 32] {
    sha256(b"integration-program")
}

fn gc_report() -> GcReport {
    let receipt = GcRunReceipt {
        schema_version: GC_SCHEMA_VERSION.into(),
        record_type: GC_RECORD_TYPE_DRY_RUN.into(),
        store_contract_digest: gc_contract_digest_hex(),
        run_id: "program-evidence-integration".into(),
        store_root: "/tmp/program-evidence-integration".into(),
        evaluated_at: "2026-08-11T00:00:00.000Z".into(),
        apply: true,
        state: GcRunState::Complete,
        objects: Vec::new(),
        planned: Vec::new(),
        deleted: Vec::new(),
    };
    let epochs = [
        EngineIdentity::FsZero,
        EngineIdentity::GraphZero,
        EngineIdentity::TokenZero,
    ]
    .into_iter()
    .map(|engine| GcProducerEpochV1 { engine, epoch: 1 })
    .collect();
    let applied = AppliedGcEvidenceV1::new(receipt, epochs, 0).unwrap();
    GcReport::new_applied(1, program_id(), applied)
}

fn report_value(class: EvidenceClassV1) -> Value {
    use zero_gate::{
        LifecycleReport, LifecycleState, McpReport, PlannerReport, ProgramUsage, WorkerClosureKind,
        WorkerReport, mcp_evidence_digest,
    };
    let id = program_id();
    let tools = sha256(b"tools");
    match class {
        EvidenceClassV1::Planner => {
            serde_json::to_value(&PlannerReport::new(1, id, sha256(b"plan"), 3)).unwrap()
        }
        EvidenceClassV1::Worker => serde_json::to_value(&WorkerReport::new(
            1,
            id,
            sha256(b"worker"),
            3,
            WorkerClosureKind::Commit,
            mcp_evidence_digest(2, 5, tools),
            sha256(b"effects"),
            sha256(b"output"),
            ProgramUsage {
                cpu_ns: 100,
                memory_bytes: 1024,
                io_bytes: 512,
            },
        ))
        .unwrap(),
        EvidenceClassV1::Mcp => serde_json::to_value(&McpReport::new(1, id, 2, 5, tools)).unwrap(),
        EvidenceClassV1::Lifecycle => {
            serde_json::to_value(&LifecycleReport::new(1, id, 5, 3, LifecycleState::Closed))
                .unwrap()
        }
        EvidenceClassV1::Gc => serde_json::to_value(gc_report()).unwrap(),
    }
}

fn head(byte: u8) -> String {
    format!("{:02x}", byte).repeat(20)
}

/// Seals one artifact file (zeroed-field digest; see `artifact_digest`).
fn sealed_artifact_bytes(mut value: Value) -> Vec<u8> {
    value["artifact_sha256"] = json!("0".repeat(64));
    let mut length = 0u64;
    for _ in 0..4 {
        value["artifact_bytes"] = json!(length);
        let canonical = canonical_json(&value);
        let next = canonical.len() as u64;
        if next == length {
            break;
        }
        length = next;
    }
    let sha = sha256_hex(canonical_json(&value).as_bytes());
    value["artifact_sha256"] = json!(sha);
    canonical_json(&value).into_bytes()
}

fn artifact_value(class: EvidenceClassV1, source: &str, hub: &str) -> Value {
    json!({
        "contract": class.contract(),
        "schema_version": 1,
        "source_head": source,
        "hub_head": hub,
        "artifact_sha256": "0".repeat(64),
        "artifact_bytes": 0,
        "report": report_value(class),
    })
}

fn write_engine(
    base: &Path,
    engine: EngineIdV1,
    source: &str,
    hub: &str,
) -> EngineEvidenceSourceV1 {
    let dir = base.join(engine.key());
    std::fs::create_dir_all(&dir).unwrap();
    let mut files = BTreeMap::new();
    for class in EvidenceClassV1::ALL {
        let path = dir.join(format!("{}.json", class.key()));
        std::fs::write(
            &path,
            sealed_artifact_bytes(artifact_value(class, source, hub)),
        )
        .unwrap();
        files.insert(class.key().to_owned(), path);
    }
    EngineEvidenceSourceV1 {
        head: head(0x44),
        files,
    }
}

fn write_manifest(base: &Path, manifest: &ProgramEvidenceManifestV1) -> std::path::PathBuf {
    let path = base.join("manifest.json");
    let value = serde_json::to_value(&manifest).expect("manifest serializes");
    std::fs::write(&path, canonical_json(&value)).unwrap();
    path
}

fn valid_manifest(base: &Path) -> ProgramEvidenceManifestV1 {
    let source = head(0x11);
    let hub = head(0x22);
    let mut engines = BTreeMap::new();
    for engine in EngineIdV1::ALL {
        engines.insert(
            engine.key().to_owned(),
            write_engine(base, engine, &source, &hub),
        );
    }
    ProgramEvidenceManifestV1 {
        version: 1,
        source_head: source,
        hub_head: hub,
        assembly_manifest_digest: "ab".repeat(32),
        engines,
    }
}

fn fs_load(path: &Path) -> Result<Vec<u8>, zero_gate::ProgramEvidenceErrorV1> {
    std::fs::read(path).map_err(|error| zero_gate::ProgramEvidenceErrorV1::io(error.to_string()))
}

#[test]
fn assembler_seals_a_verified_receipt_from_real_files() {
    let base = TempDir::new().unwrap();
    let manifest = valid_manifest(base.path());
    let receipt = assemble_program_evidence(&manifest, fs_load).expect("assembles");
    receipt.verify().expect("receipt verifies");
    assert_eq!(receipt.engines.len(), 3);
    assert_eq!(receipt.source_repository_heads.len(), 4);
    // Canonical round-trip: the sealed receipt re-parses to the same receipt.
    let bytes = receipt.canonical_bytes().unwrap();
    let decoded = AggregateProgramReceiptV1::from_canonical_bytes(&bytes).unwrap();
    assert_eq!(decoded, receipt);
}

#[test]
fn cli_seals_a_receipt_and_exits_zero() {
    let base = TempDir::new().unwrap();
    let manifest = valid_manifest(base.path());
    let manifest_path = write_manifest(base.path(), &manifest);
    let out = base.path().join("aggregate.json");
    let status = Command::new(BIN)
        .args([
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .expect("run CLI");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let receipt = AggregateProgramReceiptV1::from_canonical_bytes(&std::fs::read(&out).unwrap())
        .expect("receipt parses");
    receipt.verify().expect("receipt verifies");
}

#[test]
fn cli_refuses_to_overwrite_an_existing_receipt() {
    let base = TempDir::new().unwrap();
    let manifest = valid_manifest(base.path());
    let manifest_path = write_manifest(base.path(), &manifest);
    let out = base.path().join("aggregate.json");
    let original = b"preserve existing receipt";
    std::fs::write(&out, original).unwrap();
    let status = Command::new(BIN)
        .args([
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .expect("run CLI");
    assert!(!status.status.success());
    assert!(String::from_utf8_lossy(&status.stderr).contains("immutable receipt"));
    assert_eq!(std::fs::read(out).unwrap(), original);
}

#[test]
fn cli_fails_closed_on_missing_engine() {
    let base = TempDir::new().unwrap();
    let mut manifest = valid_manifest(base.path());
    manifest.engines.remove("tz");
    let manifest_path = write_manifest(base.path(), &manifest);
    let out = base.path().join("aggregate.json");
    let status = Command::new(BIN)
        .args([
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .expect("run CLI");
    assert!(!status.status.success());
    let stderr = String::from_utf8_lossy(&status.stderr);
    assert!(stderr.contains("MissingEngine"), "{stderr}");
    assert!(!out.exists(), "no receipt may be written on failure");
}

#[test]
fn cli_fails_closed_on_missing_evidence_file() {
    let base = TempDir::new().unwrap();
    let manifest = valid_manifest(base.path());
    // The manifest names the artifact but the file is absent (partial input).
    let missing = manifest.engines["fz"].files["gc"].clone();
    std::fs::remove_file(&missing).unwrap();
    let manifest_path = write_manifest(base.path(), &manifest);
    let out = base.path().join("aggregate.json");
    let status = Command::new(BIN)
        .args([
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .expect("run CLI");
    assert!(!status.status.success());
    let stderr = String::from_utf8_lossy(&status.stderr);
    assert!(stderr.contains("ArtifactIo"), "{stderr}");
    assert!(!out.exists());
}

#[test]
fn cli_fails_closed_on_stale_evidence() {
    let base = TempDir::new().unwrap();
    let manifest = valid_manifest(base.path());
    // Re-collect one artifact bound to a foreign hub head.
    let path = manifest.engines["gz"].files["worker"].clone();
    let stale = artifact_value(EvidenceClassV1::Worker, &manifest.source_head, &head(0x99));
    std::fs::write(&path, sealed_artifact_bytes(stale)).unwrap();
    let manifest_path = write_manifest(base.path(), &manifest);
    let out = base.path().join("aggregate.json");
    let status = Command::new(BIN)
        .args([
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .expect("run CLI");
    assert!(!status.status.success());
    let stderr = String::from_utf8_lossy(&status.stderr);
    assert!(stderr.contains("StaleHead"), "{stderr}");
    assert!(!out.exists());
}

#[test]
fn cli_fails_closed_on_digest_mismatch() {
    let base = TempDir::new().unwrap();
    let manifest = valid_manifest(base.path());
    let path = manifest.engines["tz"].files["planner"].clone();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.push(b'\n'); // tamper after collection
    std::fs::write(&path, bytes).unwrap();
    let manifest_path = write_manifest(base.path(), &manifest);
    let out = base.path().join("aggregate.json");
    let status = Command::new(BIN)
        .args([
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .expect("run CLI");
    assert!(!status.status.success());
    let stderr = String::from_utf8_lossy(&status.stderr);
    assert!(stderr.contains("ArtifactDigestMismatch"), "{stderr}");
    assert!(!out.exists());
}

#[test]
fn cli_fails_closed_on_noncanonical_manifest() {
    let base = TempDir::new().unwrap();
    let manifest = valid_manifest(base.path());
    let manifest_path = write_manifest(base.path(), &manifest);
    let text = std::fs::read_to_string(&manifest_path).unwrap();
    // Insert whitespace: valid JSON, noncanonical encoding.
    std::fs::write(
        &manifest_path,
        text.replace("\"version\":1", "\"version\": 1"),
    )
    .unwrap();
    let out = base.path().join("aggregate.json");
    let status = Command::new(BIN)
        .args([
            "--manifest",
            manifest_path.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .expect("run CLI");
    assert!(!status.status.success());
    let stderr = String::from_utf8_lossy(&status.stderr);
    assert!(stderr.contains("NonCanonicalManifest"), "{stderr}");
    assert!(!out.exists());
}

#[test]
fn fail_closed_reports_are_typed_and_distinct() {
    // Missing class vs stale head vs digest mismatch must be distinguishable.
    let base = TempDir::new().unwrap();
    let mut manifest = valid_manifest(base.path());
    manifest.engines.get_mut("fz").unwrap().files.remove("gc");
    let missing_class = assemble_program_evidence(&manifest, fs_load).unwrap_err();
    assert_eq!(
        missing_class.failure_code(),
        &ProgramEvidenceFailureV1::MissingEvidenceClass
    );

    let manifest = valid_manifest(base.path());
    let stale = assemble_program_evidence(&manifest, fs_load).unwrap();
    let _ = stale; // fresh manifest assembles; stale is checked by the CLI tests above
    let mut value = artifact_value(
        EvidenceClassV1::Mcp,
        &manifest.source_head,
        &manifest.hub_head,
    );
    value["report"] = json!({}); // malformed report shape
    let path = manifest.engines["gz"].files["mcp"].clone();
    std::fs::write(&path, sealed_artifact_bytes(value)).unwrap();
    let malformed = assemble_program_evidence(&manifest, fs_load).unwrap_err();
    assert_eq!(
        malformed.failure_code(),
        &ProgramEvidenceFailureV1::MalformedReport
    );
}

#[test]
fn artifact_sha256_binds_heads_and_contract() {
    // Mutating the contract or a head must break the sealed digest.
    let base = TempDir::new().unwrap();
    let manifest = valid_manifest(base.path());
    let path = manifest.engines["fz"].files["planner"].clone();
    let mut value: ProgramEvidenceArtifactV1 =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value.contract = "zerostack.program.worker.v1".into();
    let text = serde_json::to_string(&value).unwrap();
    let sha = sha256_hex(text.as_bytes());
    value.artifact_sha256 = sha;
    value.artifact_bytes = text.len() as u64;
    // The declared digest no longer matches the zeroed-field recomputation.
    std::fs::write(&path, serde_json::to_string(&value).unwrap()).unwrap();
    assert_eq!(
        assemble_program_evidence(&manifest, fs_load)
            .unwrap_err()
            .failure_code(),
        &ProgramEvidenceFailureV1::ContractMismatch
    );
}
