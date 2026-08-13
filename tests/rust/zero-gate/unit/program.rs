    use super::*;

    fn program_id() -> ProgramDigest {
        hash_bytes(b"program-86qk-20")
    }
    fn plan_digest() -> ProgramDigest {
        hash_bytes(b"plan")
    }
    fn worker_id() -> ProgramDigest {
        hash_bytes(b"worker")
    }
    fn effects_digest() -> ProgramDigest {
        hash_bytes(b"effects")
    }

    fn applied_gc_report(id: ProgramDigest, count: usize, freed_bytes: u64) -> GcReport {
        use zero_store::{
            GC_RECORD_TYPE_DRY_RUN, GC_SCHEMA_VERSION, GcCandidate, GcVerdict,
            gc_contract_digest_hex,
        };
        let hashes = (0..count)
            .map(|index| format!("{index:064x}"))
            .collect::<Vec<_>>();
        let receipt = GcRunReceipt {
            schema_version: GC_SCHEMA_VERSION.into(),
            record_type: GC_RECORD_TYPE_DRY_RUN.into(),
            store_contract_digest: gc_contract_digest_hex(),
            run_id: "program-test-applied".into(),
            store_root: "/tmp/program-test-applied".into(),
            evaluated_at: "2026-08-11T00:00:00.000Z".into(),
            apply: true,
            state: GcRunState::Complete,
            objects: hashes
                .iter()
                .map(|hash| GcCandidate {
                    blob_hash: hash.clone(),
                    verdict: GcVerdict::Collect,
                    reason_codes: vec!["no-live-reference".into()],
                    evidence: vec!["test applied receipt".into()],
                })
                .collect(),
            planned: hashes.clone(),
            deleted: hashes,
        };
        let applied = AppliedGcEvidenceV1::new(
            receipt,
            vec![
                GcProducerEpochV1 {
                    engine: EngineIdentity::FsZero,
                    epoch: 1,
                },
                GcProducerEpochV1 {
                    engine: EngineIdentity::GraphZero,
                    epoch: 1,
                },
                GcProducerEpochV1 {
                    engine: EngineIdentity::TokenZero,
                    epoch: 1,
                },
            ],
            freed_bytes,
        )
        .unwrap();
        GcReport::new_applied(PROGRAM_ASSEMBLY_SCHEMA_VERSION, id, applied)
    }
    fn output_digest() -> ProgramDigest {
        hash_bytes(b"output")
    }
    fn tools_digest() -> ProgramDigest {
        hash_bytes(b"tools")
    }

    fn valid_reports() -> ProgramReports {
        let id = program_id();
        let mcp = McpReport::new(PROGRAM_ASSEMBLY_SCHEMA_VERSION, id, 2, 5, tools_digest());
        ProgramReports::new()
            .planner(PlannerReport::new(
                PROGRAM_ASSEMBLY_SCHEMA_VERSION,
                id,
                plan_digest(),
                3,
            ))
            .worker(WorkerReport::new(
                PROGRAM_ASSEMBLY_SCHEMA_VERSION,
                id,
                worker_id(),
                3,
                WorkerClosureKind::Commit,
                mcp_evidence_digest(2, 5, tools_digest()),
                effects_digest(),
                output_digest(),
                ProgramUsage {
                    cpu_ns: 100,
                    memory_bytes: 1024,
                    io_bytes: 512,
                },
            ))
            .mcp(mcp)
            .lifecycle(LifecycleReport::new(
                PROGRAM_ASSEMBLY_SCHEMA_VERSION,
                id,
                5,
                3,
                LifecycleState::Closed,
            ))
            .gc(applied_gc_report(id, 7, 4096))
    }

    #[test]
    fn valid_reports_assemble_into_an_authoritative_proof() {
        let proof = valid_reports()
            .assemble()
            .expect("valid reports must assemble");
        assert_eq!(proof.program_id(), program_id());
        assert_eq!(proof.step_count(), 3);
        assert_eq!(proof.tool_count(), 2);
        assert_eq!(proof.call_count(), 5);
        assert_eq!(proof.collected_objects(), 7);
        assert_eq!(proof.freed_bytes(), 4096);
        proof.verify().expect("proof must verify");
    }

    #[test]
    fn proof_is_opaque_and_linear() {
        let proof = valid_reports()
            .assemble()
            .expect("valid reports must assemble");
        // No Clone, no Deserialize: the type itself enforces linearity. We can
        // only observe through getters.
        let _ = proof.program_digest();
    }

    #[test]
    fn missing_any_source_fails_with_missing_report() {
        let base = valid_reports();
        let cases = [
            (
                ProgramReports::new()
                    .worker(base.worker_report().unwrap().clone())
                    .mcp(base.mcp_report().unwrap().clone())
                    .lifecycle(base.lifecycle_report().unwrap().clone())
                    .gc(base.gc_report().unwrap().clone()),
                EvidenceSource::Planner,
            ),
            (
                ProgramReports::new()
                    .planner(base.planner_report().unwrap().clone())
                    .mcp(base.mcp_report().unwrap().clone())
                    .lifecycle(base.lifecycle_report().unwrap().clone())
                    .gc(base.gc_report().unwrap().clone()),
                EvidenceSource::Worker,
            ),
            (
                ProgramReports::new()
                    .planner(base.planner_report().unwrap().clone())
                    .worker(base.worker_report().unwrap().clone())
                    .lifecycle(base.lifecycle_report().unwrap().clone())
                    .gc(base.gc_report().unwrap().clone()),
                EvidenceSource::Mcp,
            ),
            (
                ProgramReports::new()
                    .planner(base.planner_report().unwrap().clone())
                    .worker(base.worker_report().unwrap().clone())
                    .mcp(base.mcp_report().unwrap().clone())
                    .gc(base.gc_report().unwrap().clone()),
                EvidenceSource::Lifecycle,
            ),
            (
                ProgramReports::new()
                    .planner(base.planner_report().unwrap().clone())
                    .worker(base.worker_report().unwrap().clone())
                    .mcp(base.mcp_report().unwrap().clone())
                    .lifecycle(base.lifecycle_report().unwrap().clone()),
                EvidenceSource::Gc,
            ),
        ];
        for (reports, expected) in cases {
            assert_eq!(
                reports.assemble().unwrap_err(),
                ProgramAssemblyError::MissingReport(expected)
            );
        }
    }

    #[test]
    fn fallback_closure_never_yields_a_proof() {
        let base = valid_reports();
        let id = program_id();
        let reports = ProgramReports::new()
            .planner(base.planner_report().unwrap().clone())
            .worker(WorkerReport::new(
                PROGRAM_ASSEMBLY_SCHEMA_VERSION,
                id,
                worker_id(),
                3,
                WorkerClosureKind::Fallback,
                mcp_evidence_digest(2, 5, tools_digest()),
                effects_digest(),
                output_digest(),
                ProgramUsage::default(),
            ))
            .mcp(base.mcp_report().unwrap().clone())
            .lifecycle(base.lifecycle_report().unwrap().clone())
            .gc(base.gc_report().unwrap().clone());
        assert_eq!(
            reports.assemble().unwrap_err(),
            ProgramAssemblyError::FallbackReceipt
        );
    }

    #[test]
    fn mcp_calls_without_worker_backing_are_synthetic() {
        let base = valid_reports();
        let id = program_id();
        // MCP claims 9 calls; the worker still binds to evidence for 5 calls.
        let forged_mcp = McpReport::new(PROGRAM_ASSEMBLY_SCHEMA_VERSION, id, 2, 9, tools_digest());
        let reports = ProgramReports::new()
            .planner(base.planner_report().unwrap().clone())
            .worker(base.worker_report().unwrap().clone())
            .mcp(forged_mcp)
            .lifecycle(base.lifecycle_report().unwrap().clone())
            .gc(base.gc_report().unwrap().clone());
        assert_eq!(
            reports.assemble().unwrap_err(),
            ProgramAssemblyError::McpEvidenceMismatch
        );
    }

    #[test]
    fn step_count_mismatch_fails_closed() {
        let base = valid_reports();
        let id = program_id();
        let reports = ProgramReports::new()
            .planner(PlannerReport::new(
                PROGRAM_ASSEMBLY_SCHEMA_VERSION,
                id,
                plan_digest(),
                4,
            ))
            .worker(base.worker_report().unwrap().clone())
            .mcp(base.mcp_report().unwrap().clone())
            .lifecycle(base.lifecycle_report().unwrap().clone())
            .gc(base.gc_report().unwrap().clone());
        assert_eq!(
            reports.assemble().unwrap_err(),
            ProgramAssemblyError::StepCountMismatch
        );
    }

    #[test]
    fn gc_before_lifecycle_close_fails_closed() {
        let base = valid_reports();
        let id = program_id();
        let reports = ProgramReports::new()
            .planner(base.planner_report().unwrap().clone())
            .worker(base.worker_report().unwrap().clone())
            .mcp(base.mcp_report().unwrap().clone())
            .lifecycle(base.lifecycle_report().unwrap().clone())
            .gc(GcReport::new(
                PROGRAM_ASSEMBLY_SCHEMA_VERSION,
                id,
                7,
                4096,
                false,
            ));
        assert_eq!(
            reports.assemble().unwrap_err(),
            ProgramAssemblyError::GcBeforeLifecycleClose
        );
    }

    #[test]
    fn malformed_claimed_digest_is_rejected() {
        let base = valid_reports();
        let mut planner = base.planner_report().unwrap().clone();
        // The claimed digest is private; flip it through serialization.
        let value = serde_json::to_value(&planner).unwrap();
        let mut object = value.as_object().unwrap().clone();
        object.insert("digest".to_string(), serde_json::json!(vec![0u8; 32]));
        planner = serde_json::from_value(serde_json::Value::Object(object)).unwrap();
        let reports = ProgramReports::new()
            .planner(planner)
            .worker(base.worker_report().unwrap().clone())
            .mcp(base.mcp_report().unwrap().clone())
            .lifecycle(base.lifecycle_report().unwrap().clone())
            .gc(base.gc_report().unwrap().clone());
        assert_eq!(
            reports.assemble().unwrap_err(),
            ProgramAssemblyError::MalformedReport(EvidenceSource::Planner)
        );
    }

    #[test]
    fn synthetic_zero_binding_is_rejected() {
        let base = valid_reports();
        let id = program_id();
        let reports = ProgramReports::new()
            .planner(base.planner_report().unwrap().clone())
            .worker(base.worker_report().unwrap().clone())
            .mcp(McpReport::new(
                PROGRAM_ASSEMBLY_SCHEMA_VERSION,
                id,
                2,
                5,
                [0u8; 32],
            ))
            .lifecycle(base.lifecycle_report().unwrap().clone())
            .gc(base.gc_report().unwrap().clone());
        assert_eq!(
            reports.assemble().unwrap_err(),
            ProgramAssemblyError::SyntheticReceipt(EvidenceSource::Mcp)
        );
    }

    #[test]
    fn program_id_mismatch_fails_closed() {
        let base = valid_reports();
        let other_id = hash_bytes(b"other-program");
        let reports = ProgramReports::new()
            .planner(PlannerReport::new(
                PROGRAM_ASSEMBLY_SCHEMA_VERSION,
                other_id,
                plan_digest(),
                3,
            ))
            .worker(base.worker_report().unwrap().clone())
            .mcp(base.mcp_report().unwrap().clone())
            .lifecycle(base.lifecycle_report().unwrap().clone())
            .gc(base.gc_report().unwrap().clone());
        assert_eq!(
            reports.assemble().unwrap_err(),
            ProgramAssemblyError::ProgramIdMismatch
        );
    }

    #[test]
    fn lifecycle_transition_invariant_is_enforced() {
        let base = valid_reports();
        let id = program_id();
        let reports = ProgramReports::new()
            .planner(base.planner_report().unwrap().clone())
            .worker(base.worker_report().unwrap().clone())
            .mcp(base.mcp_report().unwrap().clone())
            .lifecycle(LifecycleReport::new(
                PROGRAM_ASSEMBLY_SCHEMA_VERSION,
                id,
                4,
                3,
                LifecycleState::Closed,
            ))
            .gc(base.gc_report().unwrap().clone());
        assert_eq!(
            reports.assemble().unwrap_err(),
            ProgramAssemblyError::LifecycleTransitionMismatch
        );
    }

    #[test]
    fn bounds_are_enforced() {
        let base = valid_reports();
        let id = program_id();
        let reports = ProgramReports::new()
            .planner(base.planner_report().unwrap().clone())
            .worker(base.worker_report().unwrap().clone())
            .mcp(base.mcp_report().unwrap().clone())
            .lifecycle(base.lifecycle_report().unwrap().clone())
            .gc(GcReport::new(
                PROGRAM_ASSEMBLY_SCHEMA_VERSION,
                id,
                MAX_GC_OBJECTS + 1,
                4096,
                true,
            ));
        assert_eq!(
            reports.assemble().unwrap_err(),
            ProgramAssemblyError::BoundsExceeded(EvidenceSource::Gc)
        );
    }

    #[test]
    fn digest_functions_are_deterministic_known_answers() {
        // Freeze the canonical encoding so future edits cannot silently change
        // the commitments without updating the fixtures.
        let id = program_id();
        let p = PlannerReport::new(PROGRAM_ASSEMBLY_SCHEMA_VERSION, id, plan_digest(), 3);
        let w = WorkerReport::new(
            PROGRAM_ASSEMBLY_SCHEMA_VERSION,
            id,
            worker_id(),
            3,
            WorkerClosureKind::Commit,
            mcp_evidence_digest(2, 5, tools_digest()),
            effects_digest(),
            output_digest(),
            ProgramUsage::default(),
        );
        let m = McpReport::new(PROGRAM_ASSEMBLY_SCHEMA_VERSION, id, 2, 5, tools_digest());
        let l = LifecycleReport::new(
            PROGRAM_ASSEMBLY_SCHEMA_VERSION,
            id,
            5,
            3,
            LifecycleState::Closed,
        );
        let g = applied_gc_report(id, 7, 4096);
        let all = vec![
            hex(&p.digest()),
            hex(&w.digest()),
            hex(&m.digest()),
            hex(&l.digest()),
            hex(&g.digest()),
        ];
        let expected = vec![
            "d02d9c1ba9a268bf3105a499d34b539cd59561f8bbace58bcc4a7d36a3ee6769",
            "a20e40ed615196a885eb41717bf8b5d114533d2e27209165f9fcb3bfa41dc085",
            "1183ce8e7e784a49623ae3cb02d8e8f634793f414cbc661eb97a78fca20ead16",
            "f837840cefed81facbb96c4cfa8d76a89e6601565c8b7f2a1d51ce08f974e1bd",
            "99bffbba537270dd0c7599e60ae4c18633344a85e5404e80aca0f9d5331d2e82",
        ];
        assert_eq!(all, expected);
    }

    fn hex(bytes: &ProgramDigest) -> String {
        let mut out = String::with_capacity(64);
        for byte in bytes {
            out.push_str(&format!("{:02x}", byte));
        }
        out
    }
