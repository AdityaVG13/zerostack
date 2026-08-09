//! Frozen Z5 model, runtime record schema, and proof receipt checks.

use serde_json::{json, Value};
use std::{collections::BTreeSet, fs, path::PathBuf};
use zero_abi::sha256_hex;
use zerostack_shared_tests::racc::{validate_racc_schema, RACC_TWO_PHASE_GATE_SCHEMA};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
fn digest(byte: u8) -> Value {
    json!(vec![byte; 32])
}

fn guard_events(count: usize) -> Vec<Value> {
    let guards = [
        "g0_canonical",
        "g1_coherence",
        "g2_finite_plan",
        "g3_attribution",
        "g4_resources",
        "g5_robust_snap",
        "g6_safety_shield",
        "g7_performance",
        "g8_transaction_closure",
        "g9_receipt_commitment",
    ];
    (0..count)
        .map(|index| {
            json!({
                "guard": guards[index],
                "predecessor": if index == 0 { Value::Null } else { json!(guards[index - 1]) },
                "status": "passed"
            })
        })
        .collect()
}

fn trace(count: usize) -> Value {
    json!({
        "events": guard_events(count),
        "executed_instructions": 5,
        "worker_steps": 1,
        "buffered_visible_bytes": 15,
        "staged_effects": 1,
        "execution_failure": null
    })
}

#[test]
fn two_phase_gate_record_schema_accepts_exact_permit_and_receipt_shapes() {
    let permit = json!({
        "schema_version": 1,
        "permit_id": digest(1),
        "binding_digest": digest(2),
        "admission_digest": digest(15),
        "surface": "mcp",
        "trace": trace(8)
    });
    validate_racc_schema(RACC_TWO_PHASE_GATE_SCHEMA, &permit).unwrap();

    let receipt = json!({
        "schema_version": 1,
        "kind": "commit",
        "permit_id": digest(1),
        "binding_digest": digest(2),
        "admission_digest": digest(15),
        "assembly_manifest_digest": digest(3),
        "source_tree_digest": digest(4),
        "source_repository_heads": [{"repository":"ZeroStack","head":"87c8ef5df0699b6345e4a829876b3f086f9c3ae5"}],
        "image_digest": digest(5),
        "plan_digest": digest(6),
        "comparison_identity_digest": digest(7),
        "surface": "mcp",
        "verification_digest": digest(8),
        "output_digest": digest(9),
        "effects_digest": digest(10),
        "resource_usage": {"fuel":20,"elapsed_ms":10,"io_bytes":64,"memory_bytes":1024,"processes":1,"risk_units":1,"worker_steps":1},
        "predecessor_receipt_head": digest(11),
        "successor_root": digest(12),
        "trace_digest": digest(13),
        "receipt_head": digest(14),
        "failure_code": null,
        "restoration": {"attempted":0,"completed":0,"debt":0}
    });
    validate_racc_schema(RACC_TWO_PHASE_GATE_SCHEMA, &receipt).unwrap();

    let mut forged = permit;
    forged["trace"]["events"] = json!(guard_events(7));
    assert!(validate_racc_schema(RACC_TWO_PHASE_GATE_SCHEMA, &forged).is_err());
    let mut drift = receipt;
    drift["unbound"] = json!(true);
    assert!(validate_racc_schema(RACC_TWO_PHASE_GATE_SCHEMA, &drift).is_err());
    let mut missing_admission = drift;
    missing_admission.as_object_mut().unwrap().remove("unbound");
    missing_admission
        .as_object_mut()
        .unwrap()
        .remove("admission_digest");
    assert!(validate_racc_schema(RACC_TWO_PHASE_GATE_SCHEMA, &missing_admission).is_err());
}

#[test]
fn two_phase_gate_model_freezes_predecessors_bindings_and_not_run_boundary() {
    let model: Value =
        serde_json::from_slice(&fs::read(root().join("models/two-phase-gate-v1.json")).unwrap())
            .unwrap();
    assert_eq!(model["authority"]["freeze_id"], "Z5");
    assert_eq!(model["guard_order"].as_array().unwrap().len(), 10);
    for (index, guard) in model["guard_order"].as_array().unwrap().iter().enumerate() {
        assert_eq!(guard["id"], format!("G{index}"));
        if index == 0 {
            assert!(guard["predecessor"].is_null());
        } else {
            assert_eq!(guard["predecessor"], format!("G{}", index - 1));
        }
    }
    let bindings = model["receipt_bindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<BTreeSet<_>>();
    for required in [
        "assembly_manifest_digest",
        "admission_digest",
        "source_repository_heads",
        "plan_digest",
        "output_digest",
        "effects_digest",
        "resource_usage",
        "predecessor_receipt_head",
        "successor_root",
        "trace_digest",
    ] {
        assert!(bindings.contains(required), "missing {required}");
    }
    assert_eq!(
        model["platform_profiles"]["windows_amd64"]["status"],
        "NOT_RUN"
    );
    assert_eq!(
        model["platform_profiles"]["windows_amd64"]["native_evidence"],
        false
    );
}

#[test]
fn two_phase_gate_proof_receipt_is_partial_hash_bound_and_non_promotable() {
    let path = root().join("models/two-phase-gate-v1-proof-receipt.json");
    let bytes = fs::read(&path).unwrap();
    let receipt: Value = serde_json::from_slice(&bytes).unwrap();
    let sidecar =
        fs::read_to_string(root().join("models/two-phase-gate-v1-proof-receipt.sha256")).unwrap();
    assert_eq!(sidecar.trim(), sha256_hex(&bytes));
    for field in [
        "schema_version",
        "bead_key",
        "claim_or_freeze_ids",
        "assembly_manifest_digest",
        "source_repository_heads",
        "model_or_spec_version",
        "toolchain_identities",
        "exact_commands",
        "input_fixture_hashes",
        "output_artifact_hashes",
        "mutants_run",
        "platform_profile",
        "result",
        "failure_code",
        "residual_assumptions",
        "started_at",
        "completed_at",
        "immutability",
    ] {
        assert!(!receipt[field].is_null(), "missing {field}");
    }
    assert_eq!(receipt["result"]["status"], "PARTIAL");
    assert_eq!(receipt["result"]["promotable"], false);
    assert_eq!(
        receipt["platform_profile"]["windows_amd64"]["status"],
        "NOT_RUN"
    );
    assert_eq!(receipt["failure_code"], "NATIVE_WINDOWS_NOT_RUN");
    for (path, expected) in receipt["output_artifact_hashes"].as_object().unwrap() {
        let actual = sha256_hex(&fs::read(root().parent().unwrap().join(path)).unwrap());
        assert_eq!(&actual, expected.as_str().unwrap(), "{path}");
    }
}
