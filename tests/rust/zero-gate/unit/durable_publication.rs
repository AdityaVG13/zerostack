    use std::{borrow::Cow, collections::BTreeMap};

    use super::*;
    use tempfile::tempdir;
    use zero_cert::{
        EvidenceCertificate, ObjectId, OperatorLock, Provenance, Resolver, SpanRef, TestId, verify,
    };
    use zero_store::{
        JournalPathsV1, commit_journal_v1, initialize_published_root_v1, prepare_journal_v1,
    };

    use crate::two_phase::{
        AttributionClass, ExecutionBinding, ExecutionSurface, ResourceUsage, RestorationAccounting,
        SourceHead, TWO_PHASE_SCHEMA_VERSION, WorkerEnvelope, candidate_protocol_identity_v1,
        seal_receipt_record_for_test,
    };
    use crate::{
        ExactNeutralCertificateV1, FrozenBaselineV1, QualityAdmissionV1, QualityEvidenceV1,
        ReasoningSafepointV1, ReasoningStateStatusV1, SemanticCutCertificateRecordV1,
        SemanticCutClaimV1, SemanticCutEvidenceV1,
    };
    use zero_abi::{
        NativeStatePolicyV1, ReasoningContractV1, raw_worker::EffectClass,
        verify_strict_no_downshift_v1,
    };

    fn abi(byte: u8) -> AbiDigestV1 {
        AbiDigestV1::from_bytes([byte; 32])
    }
    fn paths(directory: &std::path::Path) -> JournalPathsV1 {
        JournalPathsV1::new(
            directory.join("root.json"),
            directory.join("journal.json"),
            directory.join("cartridge.json"),
            directory.join("owner.json"),
            directory.join("recovery.json"),
        )
        .unwrap()
    }
    fn reasoning_contract() -> ReasoningContractV1 {
        ReasoningContractV1::new(
            abi(1),
            abi(20),
            abi(21),
            abi(22),
            abi(23),
            "enabled",
            "high",
            8_192,
            4_096,
            2_048,
            1_024,
            NativeStatePolicyV1::ExactRequired,
            false,
            BTreeMap::new(),
        )
        .unwrap()
    }

    fn quality_admission() -> crate::QualityAdmissionRecordV1 {
        let reasoning_contract = reasoning_contract();
        let reasoning_contract_digest = *reasoning_contract.identity_digest().unwrap().as_bytes();
        let binding = ExecutionBinding {
            schema_version: TWO_PHASE_SCHEMA_VERSION,
            assembly_manifest_digest: [2; 32],
            source_tree_digest: [1; 32],
            source_repository_heads: vec![SourceHead {
                repository: "ZeroStack".into(),
                head: "87c8ef5df0699b6345e4a829876b3f086f9c3ae5".into(),
            }],
            image_digest: [1; 32],
            state_snapshot_digest: [1; 32],
            task_fingerprint_digest: [1; 32],
            plan_digest: [1; 32],
            fixed_model_digest: [1; 32],
            baseline_reasoning_contract: reasoning_contract.clone(),
            reasoning_contract,
            baseline_reasoning_contract_digest: reasoning_contract_digest,
            reasoning_contract_digest,
            comparison_identity_digest: [1; 32],
            semantic_cut_verifier_identity_digest: [1; 32],
            predecessor_receipt_head: [1; 32],
        };
        let certificate = ExactNeutralCertificateV1::verify(
            abi(1),
            abi(1),
            abi(3),
            zero_abi::DigestV1::from_bytes(candidate_protocol_identity_v1(&binding)),
            abi(6),
            abi(6),
            abi(7),
            abi(7),
            abi(4),
            abi(4),
        )
        .unwrap();
        QualityAdmissionV1::admit_strict(
            QualityEvidenceV1::ExactNeutral(certificate),
            FrozenBaselineV1::new(abi(3), abi(4), abi(5)).unwrap(),
        )
        .unwrap()
        .record()
    }

    fn semantic_cut_record(reasoning_contract_digest: [u8; 32]) -> SemanticCutCertificateRecordV1 {
        let terminal = |receipt| {
            ReasoningSafepointV1::new(
                [1; 32],
                [2; 32],
                [3; 32],
                reasoning_contract_digest,
                [1; 32],
                [4; 32],
                ReasoningStateStatusV1::ExactPreserved,
                [5; 32],
                [6; 32],
                [7; 32],
                [8; 32],
                [receipt; 32],
            )
            .unwrap()
        };
        let claim = SemanticCutClaimV1::new_exact(
            [1; 32],
            [9; 32],
            [1; 32],
            terminal(10),
            terminal(11),
            [12; 32],
            [12; 32],
            [13; 32],
            [13; 32],
            [14; 32],
            [15; 32],
            [1; 32],
            [1; 32],
            [16; 32],
        )
        .unwrap();
        let bytes = claim.canonical_bytes().unwrap();
        let digest = zero_abi::sha256(&bytes);
        let span = SpanRef {
            object_id: ObjectId(digest),
            object_digest: digest,
            byte_start: 0,
            byte_len: bytes.len() as u64,
            span_digest: digest,
        };
        let certificate = EvidenceCertificate {
            query: Query::TestTrace { test: TestId(9) },
            spans: vec![span],
            payload: Cow::Borrowed(&bytes),
            provenance: Provenance {
                parser_id: "canonical-json".into(),
                parser_version: "1".into(),
                index_id: "native-receipts".into(),
                index_version: "1".into(),
                operator_id: "native-journal".into(),
                operator_version: "1".into(),
            },
            completeness: CompletenessWitness::TestTrace {
                operator: OperatorLock {
                    operator_id: "native-journal".into(),
                    operator_version: "1".into(),
                },
                test: TestId(9),
                exit_code: 0,
                trace_digest: digest,
            },
            input_token_cost: 0,
            backend_work_units: 1,
        };
        let resident = Resident {
            object: ObjectId(digest),
            bytes: &bytes,
        };
        let evidence = verify(&certificate, &resident).unwrap();
        SemanticCutEvidenceV1::verify_owner_scoped(claim, &evidence)
            .unwrap()
            .record()
    }

    fn record() -> ReceiptRecord {
        let reasoning_contract = reasoning_contract();
        let reasoning_contract_digest = *reasoning_contract.identity_digest().unwrap().as_bytes();
        let reasoning_admission =
            verify_strict_no_downshift_v1(&reasoning_contract, &reasoning_contract)
                .unwrap()
                .record();
        let semantic_cut = semantic_cut_record(reasoning_contract_digest);
        let semantic_cut_certificate_digest = semantic_cut.certificate_digest;
        let semantic_cut_verifier_identity_digest = semantic_cut.verifier_identity_digest;
        let terminal_rcq_identity_digest = semantic_cut.claim.terminal_rcq_identity_digest();
        let mut record = ReceiptRecord {
            schema_version: TWO_PHASE_SCHEMA_VERSION,
            kind: ReceiptKind::Commit,
            permit_id: [1; 32],
            binding_digest: [1; 32],
            admission_digest: [1; 32],
            assembly_manifest_digest: [2; 32],
            source_tree_digest: [1; 32],
            source_repository_heads: vec![SourceHead {
                repository: "ZeroStack".into(),
                head: "87c8ef5df0699b6345e4a829876b3f086f9c3ae5".into(),
            }],
            image_digest: [1; 32],
            state_snapshot_digest: [1; 32],
            task_fingerprint_digest: [1; 32],
            plan_digest: [1; 32],
            fixed_model_digest: [1; 32],
            baseline_reasoning_contract: reasoning_contract.clone(),
            reasoning_contract,
            baseline_reasoning_contract_digest: reasoning_contract_digest,
            reasoning_contract_digest,
            reasoning_admission,
            comparison_identity_digest: [1; 32],
            semantic_cut_verifier_identity_digest,
            artifact_set_digest: [1; 32],
            semantic_cut_certificate_digest,
            semantic_cut,
            terminal_rcq_identity_digest,
            snap_certificate_digest: None,
            safety_shield_digest: [1; 32],
            quality_admission: quality_admission(),
            final_quality_selection: crate::QualitySelectionV1::Candidate,
            transaction_receipt_digest: [1; 32],
            deoptimization_execution_receipt_digest: None,
            attribution_class: AttributionClass::Fixed,
            effect_class: EffectClass::ReversibleMutation,
            resource_envelope: WorkerEnvelope {
                fuel: 1,
                deadline_ms: 1,
                io_bytes: 1,
                output_bytes: 1,
                memory_bytes: 1,
                processes: 1,
                risk_units: 1,
                worker_steps: 1,
            },
            surface: ExecutionSurface::Mcp,
            verification_digest: Some([1; 32]),
            output_digest: [1; 32],
            effects_digest: [1; 32],
            resource_usage: ResourceUsage {
                fuel: 1,
                elapsed_ms: 1,
                io_bytes: 1,
                memory_bytes: 1,
                processes: 1,
                risk_units: 1,
                worker_steps: 1,
            },
            predecessor_receipt_head: [1; 32],
            successor_root: [4; 32],
            trace_digest: [1; 32],
            receipt_head: [1; 32],
            failure_code: None,
            restoration: RestorationAccounting {
                attempted: 0,
                completed: 0,
                debt: 0,
            },
        };
        seal_receipt_record_for_test(&mut record);
        record
    }

    struct Resident<'a> {
        object: ObjectId,
        bytes: &'a [u8],
    }
    impl Resolver for Resident<'_> {
        fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
            (*object_id == self.object).then_some(self.bytes)
        }
        fn trusted_operator_version<'a>(&'a self, operator_id: &str) -> Option<&'a str> {
            (operator_id == "native-journal").then_some("1")
        }
        fn trusted_parser_version<'a>(&'a self, parser_id: &str) -> Option<&'a str> {
            (parser_id == "canonical-json").then_some("1")
        }
        fn trusted_index_version<'a>(&'a self, index_id: &str) -> Option<&'a str> {
            (index_id == "native-receipts").then_some("1")
        }
    }

    fn native_receipt(
        checks: Vec<NativeDurabilityCheckV1>,
        result: NativeDurabilityResultV1,
    ) -> NativeDurabilityReceiptV1 {
        NativeDurabilityReceiptV1 {
            schema_version: DURABLE_PUBLICATION_SCHEMA_VERSION_V1,
            durable_profile_id: DurableProfileIdV1::ApfsStrict,
            durable_profile_digest: DurableProfileV1::new(DurableProfileIdV1::ApfsStrict).digest(),
            platform: NativePlatformV1::Macos,
            filesystem: "apfs".into(),
            source_repository_head: "87c8ef5df0699b6345e4a829876b3f086f9c3ae5".into(),
            source_tree_digest: abi(6),
            artifact_digest: abi(7),
            exact_command_digest: abi(8),
            execution_authority_digest: abi(9),
            native_run_id: "native-run-1".into(),
            checks,
            result,
        }
    }

    fn verify_native_receipt(
        receipt: &NativeDurabilityReceiptV1,
    ) -> Result<VerifiedDurableFilesystemEvidenceV1, DurablePublicationErrorV1> {
        let payload = canonical_json(&serde_json::to_value(receipt).unwrap()).into_bytes();
        let digest = zero_abi::sha256(&payload);
        let object = ObjectId(digest);
        let certificate = EvidenceCertificate {
            query: Query::TestTrace { test: TestId(6) },
            spans: vec![SpanRef {
                object_id: object,
                byte_start: 0,
                byte_len: payload.len() as u64,
                object_digest: digest,
                span_digest: digest,
            }],
            payload: Cow::Owned(payload.clone()),
            provenance: Provenance {
                parser_id: "canonical-json".into(),
                parser_version: "1".into(),
                index_id: "native-receipts".into(),
                index_version: "1".into(),
                operator_id: "native-journal".into(),
                operator_version: "1".into(),
            },
            completeness: CompletenessWitness::TestTrace {
                operator: OperatorLock {
                    operator_id: "native-journal".into(),
                    operator_version: "1".into(),
                },
                test: TestId(6),
                exit_code: 0,
                trace_digest: digest,
            },
            input_token_cost: 0,
            backend_work_units: 1,
        };
        let resident = Resident {
            object,
            bytes: &payload,
        };
        let verified = verify(&certificate, &resident).unwrap();
        verify_native_durability_receipt_v1(&verified)
    }

    fn required_checks() -> Vec<NativeDurabilityCheckV1> {
        vec![
            NativeDurabilityCheckV1::FileSync,
            NativeDurabilityCheckV1::AtomicReplace,
            NativeDurabilityCheckV1::DirectorySync,
            NativeDurabilityCheckV1::KillReopen,
        ]
    }

    fn evidence() -> (tempfile::TempDir, DurablePublicationEvidenceV1) {
        let directory = tempdir().unwrap();
        let paths = paths(directory.path());
        let binding = JournalBindingV1::new(
            abi(1),
            abi(2),
            DurableProfileIdV1::ApfsStrict,
            abi(3),
            abi(4),
            abi(5),
        );
        initialize_published_root_v1(&paths, binding.old_root).unwrap();
        let cartridge = prepare_journal_v1(&paths, binding.clone()).unwrap();
        let recovery = commit_journal_v1(&paths, &cartridge).unwrap();
        let root = zero_store::read_published_root_v1(&paths).unwrap();
        let profile = verify_native_receipt(&native_receipt(
            required_checks(),
            NativeDurabilityResultV1::PassedNative,
        ))
        .unwrap();
        (
            directory,
            DurablePublicationEvidenceV1 {
                schema_version: 1,
                journal_binding: binding,
                recovery_receipt: recovery,
                published_root: root,
                filesystem_evidence: profile,
            },
        )
    }

    #[test]
    fn durable_publication_requires_verified_journal_and_profile_evidence() {
        let (_directory, evidence) = evidence();
        assert_ne!(
            verify_durable_publication_v1(&record(), &evidence).unwrap(),
            AbiDigestV1::ZERO
        );
    }

    #[test]
    fn native_receipt_rejects_rename_only_and_not_run_claims() {
        let mut checks = required_checks();
        checks.pop();
        assert_eq!(
            verify_native_receipt(&native_receipt(
                checks,
                NativeDurabilityResultV1::PassedNative,
            ))
            .unwrap_err()
            .code,
            DurablePublicationFailureCodeV1::RenameOnlyEvidence
        );
        assert_eq!(
            verify_native_receipt(&native_receipt(
                required_checks(),
                NativeDurabilityResultV1::NotRun,
            ))
            .unwrap_err()
            .code,
            DurablePublicationFailureCodeV1::UnverifiedNativeEvidence
        );
    }

    #[test]
    fn durable_publication_rejects_invalid_base_kernel_receipt() {
        let (_directory, evidence) = evidence();
        let mut gate = record();
        gate.receipt_head = [0x99; 32];
        assert_eq!(
            verify_durable_publication_v1(&gate, &evidence)
                .unwrap_err()
                .code,
            DurablePublicationFailureCodeV1::InvalidBaseReceipt
        );
    }

    #[test]
    fn durable_publication_rejects_incomplete_recovery_mutant() {
        let (_directory, mut evidence) = evidence();
        evidence.recovery_receipt.promotable = false;
        assert_eq!(
            verify_durable_publication_v1(&record(), &evidence)
                .unwrap_err()
                .code,
            DurablePublicationFailureCodeV1::IncompleteRecovery
        );
    }

    #[test]
    fn durable_publication_rejects_schema_kind_assembly_root_and_profile_mutants() {
        let (_directory, mut evidence) = evidence();
        evidence.schema_version = 2;
        assert_eq!(
            verify_durable_publication_v1(&record(), &evidence)
                .unwrap_err()
                .code,
            DurablePublicationFailureCodeV1::SchemaVersionMismatch
        );
        evidence.schema_version = 1;

        let mut gate = record();
        gate.kind = ReceiptKind::Fallback;
        assert_eq!(
            verify_durable_publication_v1(&gate, &evidence)
                .unwrap_err()
                .code,
            DurablePublicationFailureCodeV1::NonCommitReceipt
        );
        gate = record();
        gate.assembly_manifest_digest = [9; 32];
        assert_eq!(
            verify_durable_publication_v1(&gate, &evidence)
                .unwrap_err()
                .code,
            DurablePublicationFailureCodeV1::AssemblyMismatch
        );
        gate = record();
        gate.successor_root = [9; 32];
        assert_eq!(
            verify_durable_publication_v1(&gate, &evidence)
                .unwrap_err()
                .code,
            DurablePublicationFailureCodeV1::RootMismatch
        );

        evidence.filesystem_evidence.durable_profile_id = DurableProfileIdV1::Ext4XfsStrict;
        evidence.filesystem_evidence.durable_profile_digest =
            DurableProfileV1::new(DurableProfileIdV1::Ext4XfsStrict).digest();
        assert_eq!(
            verify_durable_publication_v1(&record(), &evidence)
                .unwrap_err()
                .code,
            DurablePublicationFailureCodeV1::ProfileMismatch
        );
    }
