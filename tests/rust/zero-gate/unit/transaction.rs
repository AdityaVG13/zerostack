    use super::*;
    use std::borrow::Cow;
    use tempfile::tempdir;
    use zero_abi::{
        sha256, CwirVerifierClassV1, EffectTargetV1, EffectVerificationPlanV1,
        EffectVerificationStepV1, TypedEffectOperationV1,
    };
    use zero_cert::{
        accept_effect_verification_v1, verify, CompletenessWitness, EffectVerificationOutcomeV1,
        EvidenceCertificate, ObjectId, OperatorLock, Provenance, Query, Resolver, SpanRef,
    };
    use zero_store::{initialize_published_root_v1, DurableProfileIdV1};

    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    struct Resident<'a> {
        bytes: &'a [u8],
    }

    impl Resolver for Resident<'_> {
        fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
            (sha256(self.bytes) == object_id.0).then_some(self.bytes)
        }
        fn trusted_operator_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "read-span").then_some("1")
        }
        fn trusted_parser_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "tree-sitter").then_some("1")
        }
        fn trusted_index_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "zero-index").then_some("2")
        }
    }

    fn accepted(program: &EffectProgramV1) -> zero_cert::EffectAcceptedV1 {
        let bytes = b"exact evidence";
        let object = sha256(bytes);
        let span = SpanRef {
            object_id: ObjectId(object),
            object_digest: object,
            byte_start: 0,
            byte_len: bytes.len() as u64,
            span_digest: object,
        };
        let certificate = EvidenceCertificate {
            query: Query::ReadSpan(span.clone()),
            spans: vec![span],
            payload: Cow::Borrowed(bytes),
            provenance: Provenance {
                parser_id: "tree-sitter".into(),
                parser_version: "1".into(),
                index_id: "zero-index".into(),
                index_version: "2".into(),
                operator_id: "read-span".into(),
                operator_version: "1".into(),
            },
            completeness: CompletenessWitness::ReadSpan {
                operator: OperatorLock {
                    operator_id: "read-span".into(),
                    operator_version: "1".into(),
                },
            },
            input_token_cost: 1,
            backend_work_units: 1,
        };
        let resident = Resident { bytes };
        let verified = verify(&certificate, &resident).unwrap();
        let outcome = accept_effect_verification_v1(
            digest(70),
            program,
            digest(71),
            digest(21),
            program.base_state(),
            digest(20),
            &verified,
        )
        .unwrap();
        let EffectVerificationOutcomeV1::Accepted(accepted) = outcome else {
            panic!("expected accepted effect");
        };
        accepted
    }

    fn program(snapshot: DigestV1, rollback: EffectRollbackV1) -> EffectProgramV1 {
        let target = EffectTargetV1 {
            owner: ArtifactOwnerV1::FsZero,
            target_digest: digest(10),
            required_snapshot: snapshot,
        };
        let step = EffectVerificationStepV1 {
            verifier_digest: digest(20),
            predicate_digest: digest(21),
            environment_digest: digest(22),
            required_snapshot: snapshot,
            verifier_class: CwirVerifierClassV1::ExactChecker,
        };
        let (targets, operations) = if rollback == EffectRollbackV1::ReadOnly {
            let bytes = b"literal".to_vec();
            (
                vec![],
                vec![TypedEffectOperationV1::ReturnLiteral {
                    payload_digest: DigestV1::from_bytes(sha256(&bytes)),
                    bytes,
                }],
            )
        } else {
            (
                vec![target],
                vec![TypedEffectOperationV1::ReplaceExactFile {
                    target: digest(10),
                    expected_before: digest(11),
                    replacement: digest(12),
                }],
            )
        };
        EffectProgramV1::new(
            snapshot,
            "transaction_test",
            targets,
            vec![],
            operations,
            vec![],
            EffectVerificationPlanV1::new(vec![step]).unwrap(),
            rollback,
        )
        .unwrap()
    }

    fn resource(
        kind: TransactionResourceKindV1,
        scope: u8,
        baseline: DigestV1,
        access: TransactionAccessV1,
    ) -> TransactionResourceRequirementV1 {
        TransactionResourceRequirementV1 {
            owner: if kind == TransactionResourceKindV1::ProjectFilesystem {
                ArtifactOwnerV1::FsZero
            } else {
                ArtifactOwnerV1::ZeroStack
            },
            kind,
            scope_digest: digest(scope),
            baseline_state_digest: baseline,
            access,
            authority_digest: digest(scope.wrapping_add(2)),
        }
    }

    fn closed(
        snapshot: DigestV1,
    ) -> (
        EffectClosureRequestV1,
        EffectClosureManifestV1,
        ClosedEffectBoundaryV1,
    ) {
        let project = resource(
            TransactionResourceKindV1::ProjectFilesystem,
            30,
            snapshot,
            TransactionAccessV1::ReadWrite,
        );
        let time = resource(
            TransactionResourceKindV1::Time,
            40,
            digest(41),
            TransactionAccessV1::Read,
        );
        let request = EffectClosureRequestV1::new(
            &program(snapshot, EffectRollbackV1::Journaled),
            vec![time, project],
        )
        .unwrap();
        let manifest = EffectClosureManifestV1::new(
            &request,
            vec![
                EffectResourceClosureV1 {
                    requirement: time,
                    isolation: ResourceIsolationModeV1::RecordedReplay,
                    restoration: ResourceRestorationModeV1::RecordedReplay,
                },
                EffectResourceClosureV1 {
                    requirement: project,
                    isolation: ResourceIsolationModeV1::Journaled,
                    restoration: ResourceRestorationModeV1::JournalRollback,
                },
            ],
        )
        .unwrap();
        let boundary = validate_effect_closure_v1(&request, &manifest).unwrap();
        (request, manifest, boundary)
    }

    fn paths(dir: &std::path::Path) -> JournalPathsV1 {
        JournalPathsV1::new(
            dir.join("root.json"),
            dir.join("journal.json"),
            dir.join("cartridge.json"),
            dir.join("owner-death.json"),
            dir.join("recovery.json"),
        )
        .unwrap()
    }

    fn binding(closed: &ClosedEffectBoundaryV1, new: DigestV1) -> JournalBindingV1 {
        effect_journal_binding_v1(
            closed,
            digest(61),
            DurableProfileIdV1::PortableStrict,
            new,
            digest(62),
        )
        .unwrap()
    }

    #[test]
    fn closure_inventory_is_canonical_and_externally_explicit() {
        let (request, manifest, boundary) = closed(digest(1));
        let bytes = manifest.canonical_bytes().unwrap();
        assert_eq!(
            EffectClosureManifestV1::from_canonical_bytes(&bytes).unwrap(),
            manifest
        );
        assert_eq!(boundary.request_digest(), request.digest().unwrap());
        assert_eq!(boundary.resource_count(), 2);
        assert_eq!(boundary.external_resource_count(), 1);
        assert_ne!(boundary.external_inventory_digest(), DigestV1::ZERO);
        let mut whitespace = bytes;
        whitespace.push(b'\n');
        assert_eq!(
            EffectClosureManifestV1::from_canonical_bytes(&whitespace)
                .unwrap_err()
                .failure_code(),
            TransactionFailureCodeV1::NonCanonicalEncoding
        );
    }

    #[test]
    fn unsupported_missing_and_incompatible_resources_block_speculation() {
        let snapshot = digest(1);
        let project = resource(
            TransactionResourceKindV1::ProjectFilesystem,
            30,
            snapshot,
            TransactionAccessV1::ReadWrite,
        );
        let time = resource(
            TransactionResourceKindV1::Time,
            40,
            digest(41),
            TransactionAccessV1::Read,
        );
        assert_eq!(
            EffectClosureRequestV1::new(
                &program(snapshot, EffectRollbackV1::Journaled),
                vec![time],
            )
            .unwrap_err()
            .failure_code(),
            TransactionFailureCodeV1::MissingOperationResource
        );
        let request = EffectClosureRequestV1::new(
            &program(snapshot, EffectRollbackV1::Journaled),
            vec![project, time],
        )
        .unwrap();
        let unsupported = EffectClosureManifestV1::new(
            &request,
            vec![
                EffectResourceClosureV1 {
                    requirement: project,
                    isolation: ResourceIsolationModeV1::Journaled,
                    restoration: ResourceRestorationModeV1::JournalRollback,
                },
                EffectResourceClosureV1 {
                    requirement: time,
                    isolation: ResourceIsolationModeV1::Unsupported,
                    restoration: ResourceRestorationModeV1::Unsupported,
                },
            ],
        )
        .unwrap();
        assert_eq!(
            validate_effect_closure_v1(&request, &unsupported)
                .unwrap_err()
                .failure_code(),
            TransactionFailureCodeV1::UnsupportedIsolation
        );
        let missing = EffectClosureManifestV1::new(
            &request,
            vec![EffectResourceClosureV1 {
                requirement: project,
                isolation: ResourceIsolationModeV1::Journaled,
                restoration: ResourceRestorationModeV1::JournalRollback,
            }],
        )
        .unwrap();
        assert_eq!(
            validate_effect_closure_v1(&request, &missing)
                .unwrap_err()
                .failure_code(),
            TransactionFailureCodeV1::MissingResource
        );
        let invalid_access = EffectClosureManifestV1::new(
            &request,
            vec![
                EffectResourceClosureV1 {
                    requirement: project,
                    isolation: ResourceIsolationModeV1::ImmutableSnapshot,
                    restoration: ResourceRestorationModeV1::NotNeeded,
                },
                EffectResourceClosureV1 {
                    requirement: time,
                    isolation: ResourceIsolationModeV1::RecordedReplay,
                    restoration: ResourceRestorationModeV1::RecordedReplay,
                },
            ],
        )
        .unwrap();
        assert_eq!(
            validate_effect_closure_v1(&request, &invalid_access)
                .unwrap_err()
                .failure_code(),
            TransactionFailureCodeV1::IsolationAccessMismatch
        );
    }

    #[test]
    fn rollback_class_must_cover_writes_and_raw_fallback_never_speculates() {
        let snapshot = digest(1);
        let project = resource(
            TransactionResourceKindV1::ProjectFilesystem,
            30,
            snapshot,
            TransactionAccessV1::ReadWrite,
        );
        let request = EffectClosureRequestV1::new(
            &program(snapshot, EffectRollbackV1::WorkspaceClone),
            vec![project],
        )
        .unwrap();
        let manifest = EffectClosureManifestV1::new(
            &request,
            vec![EffectResourceClosureV1 {
                requirement: project,
                isolation: ResourceIsolationModeV1::Journaled,
                restoration: ResourceRestorationModeV1::JournalRollback,
            }],
        )
        .unwrap();
        assert_eq!(
            validate_effect_closure_v1(&request, &manifest)
                .unwrap_err()
                .failure_code(),
            TransactionFailureCodeV1::RollbackMismatch
        );

        let raw = EffectProgramV1::new(
            snapshot,
            "raw",
            vec![],
            vec![],
            vec![TypedEffectOperationV1::RawFallback],
            vec![],
            EffectVerificationPlanV1::new(vec![]).unwrap(),
            EffectRollbackV1::RawFallback,
        )
        .unwrap();
        let read = resource(
            TransactionResourceKindV1::ProjectFilesystem,
            31,
            snapshot,
            TransactionAccessV1::Read,
        );
        let request = EffectClosureRequestV1::new(&raw, vec![read]).unwrap();
        let manifest = EffectClosureManifestV1::new(
            &request,
            vec![EffectResourceClosureV1 {
                requirement: read,
                isolation: ResourceIsolationModeV1::ImmutableSnapshot,
                restoration: ResourceRestorationModeV1::NotNeeded,
            }],
        )
        .unwrap();
        assert_eq!(
            validate_effect_closure_v1(&request, &manifest)
                .unwrap_err()
                .failure_code(),
            TransactionFailureCodeV1::RawFallbackIsNotSpeculation
        );
    }

    #[test]
    fn external_transactional_writes_remain_explicit_restoration_debt() {
        let snapshot = digest(1);
        let step = EffectVerificationStepV1 {
            verifier_digest: digest(20),
            predicate_digest: digest(21),
            environment_digest: digest(22),
            required_snapshot: snapshot,
            verifier_class: CwirVerifierClassV1::ExactChecker,
        };
        let effect = EffectProgramV1::new(
            snapshot,
            "external_tx",
            vec![],
            vec![],
            vec![TypedEffectOperationV1::InvokeCapability {
                owner: ArtifactOwnerV1::ZeroStack,
                capability: "external.database".into(),
                generation: 1,
                capability_contract_digest: digest(50),
                arguments_digest: digest(51),
                effect_class: EffectClass::ReversibleMutation,
            }],
            vec![],
            EffectVerificationPlanV1::new(vec![step]).unwrap(),
            EffectRollbackV1::ExternalTransaction,
        )
        .unwrap();
        let database = resource(
            TransactionResourceKindV1::ExternalDatabase,
            80,
            digest(81),
            TransactionAccessV1::ReadWrite,
        );
        let request = EffectClosureRequestV1::new(&effect, vec![database]).unwrap();
        let manifest = EffectClosureManifestV1::new(
            &request,
            vec![EffectResourceClosureV1 {
                requirement: database,
                isolation: ResourceIsolationModeV1::Transactional,
                restoration: ResourceRestorationModeV1::TransactionRollback,
            }],
        )
        .unwrap();
        let boundary = validate_effect_closure_v1(&request, &manifest).unwrap();
        assert_eq!(boundary.external_resource_count(), 1);
        assert_eq!(boundary.external_restoration_debt_count(), 1);
        assert_ne!(boundary.external_restoration_debt_digest(), DigestV1::ZERO);

        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        initialize_published_root_v1(&paths, snapshot).unwrap();
        let receipt = begin_effect_transaction_v1(paths, binding(&boundary, digest(2)), &boundary)
            .unwrap()
            .abort()
            .unwrap();
        assert_eq!(
            receipt.disposition(),
            TransactionDispositionV1::BaselineRootRecovered
        );
        assert_eq!(
            receipt.restoration_scope(),
            RestorationScopeV1::ProjectJournalRootOnly
        );
        assert_eq!(receipt.external_restoration_debt_count(), 1);
        assert_eq!(
            receipt.external_restoration_debt_digest(),
            boundary.external_restoration_debt_digest()
        );
    }

    #[test]
    fn journal_commit_binds_effect_closure_and_external_inventory() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let old = digest(1);
        let new = digest(2);
        initialize_published_root_v1(&paths, old).unwrap();
        let (_, _, boundary) = closed(old);
        let receipt = begin_effect_transaction_v1(paths, binding(&boundary, new), &boundary)
            .unwrap()
            .commit(&accepted(&program(old, EffectRollbackV1::Journaled)))
            .unwrap();
        assert_eq!(
            receipt.disposition(),
            TransactionDispositionV1::CandidateCommitted
        );
        assert_eq!(
            receipt.restoration_scope(),
            RestorationScopeV1::NotApplicableCandidateCommit
        );
        assert_eq!(receipt.observed_root(), new);
        assert_eq!(receipt.external_resource_count(), 1);
        assert!(receipt.acceptance_digest().is_some());
        assert_eq!(
            receipt.closure_manifest_digest(),
            boundary.manifest_digest()
        );
        assert_ne!(receipt.receipt_digest(), DigestV1::ZERO);
        receipt.canonical_bytes().unwrap();
    }

    #[test]
    fn candidate_commit_requires_matching_zero_cert_acceptance() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let old = digest(1);
        let new = digest(2);
        initialize_published_root_v1(&paths, old).unwrap();
        let (_, _, boundary) = closed(old);
        let transaction =
            begin_effect_transaction_v1(paths.clone(), binding(&boundary, new), &boundary).unwrap();
        let wrong = accepted(&program(old, EffectRollbackV1::SingleAtomic));
        assert_eq!(
            transaction.commit(&wrong).unwrap_err().failure_code(),
            TransactionFailureCodeV1::EffectAcceptanceMismatch
        );
        let receipt =
            recover_effect_transaction_v1(&paths, &binding(&boundary, new), &boundary, None)
                .unwrap();
        assert_eq!(
            receipt.disposition(),
            TransactionDispositionV1::BaselineRootRecovered
        );
    }

    #[test]
    fn committed_recovery_refuses_to_invent_missing_acceptance() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let old = digest(1);
        let new = digest(2);
        initialize_published_root_v1(&paths, old).unwrap();
        let (_, _, boundary) = closed(old);
        let binding = binding(&boundary, new);
        let transaction =
            begin_effect_transaction_v1(paths.clone(), binding.clone(), &boundary).unwrap();
        commit_journal_v1(&transaction.paths, &transaction.cartridge).unwrap();
        drop(transaction);
        assert_eq!(
            recover_effect_transaction_v1(&paths, &binding, &boundary, None)
                .unwrap_err()
                .failure_code(),
            TransactionFailureCodeV1::MissingEffectAcceptance
        );
        let accepted = accepted(&program(old, EffectRollbackV1::Journaled));
        let receipt =
            recover_effect_transaction_v1(&paths, &binding, &boundary, Some(&accepted)).unwrap();
        assert_eq!(
            receipt.disposition(),
            TransactionDispositionV1::CandidateCommitted
        );
        assert_eq!(
            receipt.acceptance_digest(),
            Some(accepted.acceptance_digest())
        );
    }

    #[test]
    fn journal_abort_and_recovery_claim_only_declared_effect_closure() {
        for recover in [false, true] {
            let temp = tempdir().unwrap();
            let paths = paths(temp.path());
            let old = digest(1);
            let new = digest(2);
            initialize_published_root_v1(&paths, old).unwrap();
            let (_, _, boundary) = closed(old);
            let binding = binding(&boundary, new);
            let transaction =
                begin_effect_transaction_v1(paths.clone(), binding.clone(), &boundary).unwrap();
            let receipt = if recover {
                drop(transaction);
                recover_effect_transaction_v1(&paths, &binding, &boundary, None).unwrap()
            } else {
                transaction.abort().unwrap()
            };
            assert_eq!(
                receipt.disposition(),
                TransactionDispositionV1::BaselineRootRecovered
            );
            assert_eq!(
                receipt.restoration_scope(),
                RestorationScopeV1::DeclaredEffectClosure
            );
            assert_eq!(receipt.observed_root(), old);
            assert!(matches!(
                receipt.recovery_outcome(),
                RecoveryOutcomeV1::OldRootAborted
            ));
        }
    }

    #[test]
    fn journal_binding_must_start_at_effect_baseline() {
        let temp = tempdir().unwrap();
        let journal_paths = paths(temp.path());
        initialize_published_root_v1(&journal_paths, digest(9)).unwrap();
        let (_, _, boundary) = closed(digest(1));
        let mismatched = JournalBindingV1::new(
            digest(60),
            digest(61),
            DurableProfileIdV1::PortableStrict,
            digest(9),
            digest(2),
            digest(62),
        );
        let error = begin_effect_transaction_v1(journal_paths, mismatched, &boundary).unwrap_err();
        assert_eq!(
            error.failure_code(),
            TransactionFailureCodeV1::BaselineMismatch
        );
        let substituted = JournalBindingV1::new(
            digest(60),
            digest(61),
            DurableProfileIdV1::PortableStrict,
            boundary.baseline_state(),
            digest(2),
            digest(62),
        );
        assert_eq!(
            begin_effect_transaction_v1(paths(temp.path()), substituted, &boundary)
                .unwrap_err()
                .failure_code(),
            TransactionFailureCodeV1::JournalBindingMismatch
        );
    }

    #[test]
    fn transaction_contract_digest_is_stable() {
        assert_eq!(
            transaction_contract_digest_v1().to_hex(),
            "bd07297dca414b7acdc680d0d5abd7543af92153e14aa0b844925d686889e491"
        );
    }
