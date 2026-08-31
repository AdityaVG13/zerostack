use std::process::Command;

use serde_json::{Value, json};
use tempfile::tempdir;
use zero_abi::{KernelBudget, Sha256Digest, ZeroKernelOutcome, sha256};
use zero_gate::project_image::{
    CausalGraphRef, DemandScenario, ExactObject, PerObjectLayers, ProjectImageManifest,
    ProofGraphRef, ShadowResourceLedger,
};
use zero_gate::{
    CoverageAtom, DemandRequest, GraphZeroCompletenessInput, NativeBaseline, ProtectedScope,
};
use zero_gauge::observation::{
    MachineFingerprint, MeasuredUsage, Observation, ObservationKind, TaskIdentity,
};
use zero_gauge::report::SavingsReport;
use zero_kernel::ZeroKernel;

fn digest(seed: u8) -> Sha256Digest {
    Sha256Digest::from_bytes(sha256(&[seed; 32]))
}

fn budget() -> KernelBudget {
    KernelBudget {
        wall_ms: 20_000,
        cpu_ms: 20_000,
        memory_bytes: 128 * 1024 * 1024,
        call_limit: 32,
        task_limit: 4,
        output_byte_limit: 64 * 1024,
    }
}

fn snap_to_file_request(completeness: zero_abi::ZeroHandle) -> Value {
    let atom = digest(0x21);
    let manifest = ProjectImageManifest::new(
        digest(0x20),
        vec![ExactObject::new(atom, 128).unwrap()],
        CausalGraphRef {
            digest: None,
            unknown_reason: Some("integration fixture".into()),
        },
        ProofGraphRef {
            digest: None,
            unknown_reason: Some("integration fixture".into()),
        },
        vec![],
        vec![PerObjectLayers {
            object_root: atom,
            l1_provider_cached: Some(true),
            l2_logically_valid: Some(true),
            l3_physically_resident: Some(true),
            l2_needs_refetch: false,
            unknown_reason: None,
        }],
        vec![DemandScenario {
            scenario_id: "task-main".into(),
            demanded_object_roots: vec![atom],
            demand_weight: 1,
            window_id: None,
            unknown_reason: None,
        }],
        ShadowResourceLedger {
            rows: vec![],
            unknown_reason: None,
        },
    )
    .unwrap();
    let demand = DemandRequest::new("task-main".into(), vec![atom]).unwrap();
    let scope = ProtectedScope::new("integration-scope".into(), vec![]).unwrap();
    json!({
        "snapToFile": {
            "manifest": manifest,
            "demand": demand,
            "scope": scope,
            "completeness": completeness,
            "nativeBaseline": NativeBaseline::new(4_096, 3),
        }
    })
}

fn model_json(value: &Value) -> Value {
    match value {
        Value::String(text) => serde_json::from_str(text).unwrap(),
        other => other.clone(),
    }
}

fn observation(kind: ObservationKind, tokens: u64, bytes: u64, calls: u64) -> Observation {
    Observation {
        task: TaskIdentity {
            task_id: "integration-task".into(),
            corpus_sha: Some("b".repeat(64)),
        },
        machine: MachineFingerprint {
            os: "darwin".into(),
            arch: "arm64".into(),
            cpu_model: "test-cpu".into(),
            kernel: "test-kernel".into(),
            rustc_version: "rustc-test".into(),
            git_sha: "a".repeat(40),
            cargo_profile: "test".into(),
        },
        kind,
        usage: MeasuredUsage::new(tokens, bytes, calls),
    }
}

#[test]
fn snap_to_file_is_reachable_through_z_read() {
    let root = tempdir().unwrap();
    let kernel = ZeroKernel::canonical(
        root.path(),
        root.path().join(".zerostack"),
        "snap-product-integration",
        budget(),
    )
    .unwrap();
    let atom = digest(0x21);
    let completeness = GraphZeroCompletenessInput::new(
        digest(0x31),
        "index-current".into(),
        "task-main".into(),
        vec![CoverageAtom {
            atom_root: atom,
            covered: Some(true),
        }],
        1,
    )
    .unwrap();
    let completeness = kernel
        .register_snap_to_file_completeness(completeness)
        .unwrap();
    let source = format!(
        "return await z.read({});",
        serde_json::to_string(&snap_to_file_request(completeness)).unwrap()
    );

    let response = kernel.execute_cell(&source).unwrap();
    assert_eq!(response.outcome, ZeroKernelOutcome::Completed);
    let result = model_json(response.value.as_ref().unwrap());
    assert_eq!(result["packet"]["outcome"], "snapped");
    assert!(result["handle"].is_object());
    assert_eq!(result["expansion"]["certified_atoms"], 1);
}

#[test]
fn product_cli_renders_comparable_savings_report() {
    let root = tempdir().unwrap();
    let native_path = root.path().join("native.json");
    let zero_path = root.path().join("zero.json");
    std::fs::write(
        &native_path,
        serde_json::to_vec(&observation(
            ObservationKind::NativeBaseline,
            1_000,
            8_000,
            20,
        ))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        &zero_path,
        serde_json::to_vec(&observation(ObservationKind::ZeroDirect, 100, 800, 2)).unwrap(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_zero-kernel"))
        .args([
            "savings-report",
            "--native",
            native_path.to_str().unwrap(),
            "--zero",
            zero_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: SavingsReport = serde_json::from_slice(&output.stdout).unwrap();
    report.validate().unwrap();
    assert_eq!(report.token_fraction(), (900, 1_000));
    assert_eq!(report.byte_fraction(), (7_200, 8_000));
    assert_eq!(report.call_fraction(), (18, 20));
}

#[test]
fn product_cli_owns_program_evidence_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_zero-kernel"))
        .args(["program-evidence", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(
            "zero-kernel program-evidence --manifest <manifest.json> --out <receipt.json>"
        )
    );
}
