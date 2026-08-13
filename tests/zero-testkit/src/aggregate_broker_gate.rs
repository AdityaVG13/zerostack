//! Aggregate broker conformance for the assembly-bound two-phase gate.

pub const TWO_PHASE_GATE_VECTORS_V1: &str =
    include_str!("../conformance/two-phase-gate/v1/vectors.json");

#[cfg(test)]
mod tests {
    use super::TWO_PHASE_GATE_VECTORS_V1;
    use serde::Deserialize;
    use std::{collections::BTreeMap, fs, path::PathBuf, process::Command};
    use zero_abi::{DigestV1, raw_worker::EffectClass, sha256_hex};
    use zero_gate::{
        ExecutionSurface, ExecutionTrace, FailureCode, FinalReceipt, FrozenBaselineV1, Guard,
        QualityEnvelopeFailureCodeV1, ResourceUsage, prepare, validate_receipt_record,
    };

    use crate::kernel_fixture::{KernelMutationFixtureV2, kernel_mutation_fixture_v2};

    #[derive(Deserialize)]
    struct Vectors {
        schema_version: u16,
        vector_set: String,
        assembly_manifest_digest: String,
        source_repository_heads: BTreeMap<String, String>,
        expected_guard_order: Vec<String>,
        surfaces: Vec<String>,
        trace_cases: Vec<TraceCase>,
        typed_mutants: BTreeMap<String, String>,
    }

    #[derive(Deserialize)]
    struct TraceCase {
        case_id: String,
        trace: ExecutionTrace,
        expected_status: String,
        expected_failure_code: Option<String>,
    }

    #[derive(Deserialize)]
    struct ArchiveIndex {
        schema_version: u16,
        vector_set: String,
        evidence_scope: String,
        files: BTreeMap<String, String>,
        immutable: bool,
    }

    fn digest(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    fn surface_name(surface: ExecutionSurface) -> &'static str {
        match surface {
            ExecutionSurface::Mcp => "mcp",
            ExecutionSurface::Cli => "cli",
            ExecutionSurface::ClaudeCode => "claude_code",
            ExecutionSurface::Pi => "pi",
        }
    }

    fn fixture(surface: ExecutionSurface) -> KernelMutationFixtureV2 {
        kernel_mutation_fixture_v2(
            surface,
            digest(1),
            digest(2),
            "87c8ef5df0699b6345e4a829876b3f086f9c3ae5".into(),
        )
        .unwrap()
    }

    fn execute(surface: ExecutionSurface) -> zero_gate::CommitReceipt {
        let fixture = fixture(surface);
        let permit = prepare(fixture.request).unwrap();
        let mut execution = permit.start();
        execution
            .dispatch(
                zero_gate::PeerOwner::ZeroStack,
                ResourceUsage {
                    fuel: 20,
                    elapsed_ms: 10,
                    io_bytes: 64,
                    memory_bytes: 1024,
                    processes: 1,
                    risk_units: 1,
                    worker_steps: 1,
                },
            )
            .unwrap();
        execution.record_verification(digest(8)).unwrap();
        execution.stage_effect(fixture.staged_effect).unwrap();
        assert_eq!(
            execution.reject_early_publish().code,
            FailureCode::EarlyVisibleByte
        );
        execution.buffer_visible(b"brokered result").unwrap();
        let ready = execution
            .close_transaction(fixture.transaction_closure)
            .unwrap();
        let FinalReceipt::Commit(receipt) = ready.finalize().unwrap() else {
            panic!("expected commit receipt")
        };
        receipt
    }

    fn archive_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/two-phase-gate/v1")
    }

    #[test]
    fn aggregate_broker_gate_requires_finalize_before_publication_on_every_surface() {
        let vectors: Vectors = serde_json::from_str(TWO_PHASE_GATE_VECTORS_V1).unwrap();
        assert_eq!(vectors.schema_version, 1);
        assert_eq!(vectors.vector_set, "zerostack.two-phase-gate-kat.v1");
        assert_eq!(vectors.assembly_manifest_digest.len(), 64);
        assert_eq!(vectors.source_repository_heads.len(), 1);
        let mut observed_surfaces = Vec::new();
        for surface in [
            ExecutionSurface::Mcp,
            ExecutionSurface::Cli,
            ExecutionSurface::ClaudeCode,
            ExecutionSurface::Pi,
        ] {
            let receipt = execute(surface);
            receipt.trace().verify_complete().unwrap();
            validate_receipt_record(&receipt.record()).unwrap();
            assert_eq!(receipt.record().surface, surface);
            let published = receipt.publish();
            assert_eq!(published.visible_bytes, b"brokered result");
            assert_eq!(published.approved_effects.len(), 1);
            observed_surfaces.push(surface_name(surface));
        }
        assert_eq!(
            observed_surfaces,
            vectors
                .surfaces
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        assert_eq!(vectors.expected_guard_order.len(), 10);
    }

    #[test]
    fn aggregate_broker_gate_trace_mutants_are_typed_and_non_promotable() {
        let vectors: Vectors = serde_json::from_str(TWO_PHASE_GATE_VECTORS_V1).unwrap();
        for case in vectors.trace_cases {
            let result = case.trace.verify_complete();
            match case.expected_status.as_str() {
                "passed" => assert!(result.is_ok(), "{}", case.case_id),
                "rejected" => {
                    let code = result.unwrap_err().code.as_str();
                    assert_eq!(
                        Some(code),
                        case.expected_failure_code.as_deref(),
                        "{}",
                        case.case_id
                    );
                }
                other => panic!("unknown fixture status {other}"),
            }
        }
        for required in [
            "execute_without_permit",
            "early_visible_byte",
            "irreversible_pre_evidence_effect",
            "forged_permit",
            "unbounded_worker",
            "semantic_cut_crossing",
            "incomplete_trace",
            "unaccounted_fallback",
            "missing_approval_grant",
            "forged_receipt",
        ] {
            assert!(
                vectors.typed_mutants.contains_key(required),
                "missing {required}"
            );
        }
    }

    #[test]
    fn aggregate_broker_gate_archive_is_hash_bound_and_cross_language() {
        let root = archive_dir();
        let index: ArchiveIndex =
            serde_json::from_slice(&fs::read(root.join("index.json")).unwrap()).unwrap();
        assert_eq!(index.schema_version, 1);
        assert_eq!(index.vector_set, "zerostack.two-phase-gate-kat.v1");
        assert_eq!(
            index.evidence_scope,
            "cross_language_kat_only; native_process_evidence_not_inferred"
        );
        assert!(index.immutable);
        for (path, expected) in index.files {
            assert_eq!(
                sha256_hex(&fs::read(root.join(&path)).unwrap()),
                expected,
                "{path}"
            );
        }
        let output = Command::new("python3")
            .arg(root.join("runners/python/verify_v1.py"))
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Python runner failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            "two_phase_gate_kat:python:v1:passed"
        );
    }

    #[test]
    fn aggregate_broker_gate_runtime_mutants_fail_closed() {
        let mut unbounded = fixture(ExecutionSurface::Mcp).request;
        unbounded.envelope.processes = 0;
        assert_eq!(
            prepare(unbounded).unwrap_err().error().code,
            FailureCode::UnboundedWorker
        );

        assert_eq!(
            FrozenBaselineV1::new(DigestV1::ZERO, DigestV1::ZERO, DigestV1::ZERO)
                .unwrap_err()
                .failure_code(),
            QualityEnvelopeFailureCodeV1::MissingBinding
        );

        let mut missing_approval = fixture(ExecutionSurface::Mcp).request;
        missing_approval.effect_class = EffectClass::ApprovalRequiredMutation;
        assert_eq!(
            prepare(missing_approval).unwrap_err().error().code,
            FailureCode::MissingApprovalGrant
        );

        let mut forged_receipt = execute(ExecutionSurface::Mcp).record();
        validate_receipt_record(&forged_receipt).unwrap();
        forged_receipt.effects_digest[0] ^= 1;
        assert_eq!(
            validate_receipt_record(&forged_receipt).unwrap_err().code,
            FailureCode::ForgedReceipt
        );

        assert_eq!(
            Guard::G9ReceiptCommitment.predecessor(),
            Some(Guard::G8TransactionClosure)
        );
    }
}
