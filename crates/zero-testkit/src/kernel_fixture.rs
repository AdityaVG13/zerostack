//! Frozen, proof-carrying fixtures for the ZeroKernel conformance boundary.

use std::borrow::Cow;

use tempfile::tempdir;
use zero_abi::{
    raw_worker::EffectClass, sha256, ArtifactOwnerV1, CwirVerifierClassV1, DigestV1,
    DurableProfileIdV1, DurableProfileV1, EffectProgramV1, EffectRollbackV1, EffectTargetV1,
    EffectVerificationPlanV1, EffectVerificationStepV1, TypedEffectOperationV1, ZbfArtifactKindV1,
    ZbfObjectV1,
};
use zero_cert::{
    accept_effect_verification_v1, verify, CompletenessWitness, EffectVerificationOutcomeV1,
    EvidenceCertificate, ObjectId, OperatorLock, Provenance, Query, Resolver, SpanRef,
};
use zero_gate::{
    begin_effect_transaction_v1, candidate_protocol_identity_v1, effect_journal_binding_v1,
    validate_effect_closure_v1, CanonicalArtifactSetV1, ControllerInstruction, ControllerPlan,
    EffectClosureManifestV1, EffectClosureRequestV1, EffectResourceClosureV1,
    ExactNeutralCertificateV1, ExecutionBinding, ExecutionSurface, FrozenBaselineV1, GuardEvidence,
    PeerArtifactInputV1, PeerOwner, PrepareRequest, QualityAdmissionV1, QualityEvidenceV1,
    ResourceIsolationModeV1, ResourceRestorationModeV1, SafetyShieldEvidenceV1,
    SemanticCutEvidenceV1, SnapEvidence, SourceHead, StagedEffect, TransactionAccessV1,
    TransactionClosure, TransactionResourceKindV1, TransactionResourceRequirementV1,
    WorkerEnvelope, TWO_PHASE_SCHEMA_VERSION,
};
use zero_store::{initialize_published_root_v1, JournalPathsV1};

pub struct KernelMutationFixtureV2 {
    pub request: PrepareRequest,
    pub staged_effect: StagedEffect,
    pub transaction_closure: TransactionClosure,
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

fn certificate(bytes: &[u8]) -> EvidenceCertificate<'_> {
    let object = sha256(bytes);
    let span = SpanRef {
        object_id: ObjectId(object),
        object_digest: object,
        byte_start: 0,
        byte_len: bytes.len() as u64,
        span_digest: object,
    };
    EvidenceCertificate {
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
    }
}

fn digest(byte: u8) -> [u8; 32] {
    [byte; 32]
}

fn abi(byte: u8) -> DigestV1 {
    DigestV1::from_bytes(digest(byte))
}

fn effect_program(snapshot: DigestV1) -> Result<EffectProgramV1, String> {
    let target = EffectTargetV1 {
        owner: ArtifactOwnerV1::FsZero,
        target_digest: abi(10),
        required_snapshot: snapshot,
    };
    let verification = EffectVerificationPlanV1::new(vec![EffectVerificationStepV1 {
        verifier_digest: abi(20),
        predicate_digest: abi(21),
        environment_digest: abi(22),
        required_snapshot: snapshot,
        verifier_class: CwirVerifierClassV1::ExactChecker,
    }])
    .map_err(|error| error.to_string())?;
    EffectProgramV1::new(
        snapshot,
        "kernel_conformance_fixture",
        vec![target],
        vec![],
        vec![TypedEffectOperationV1::ReplaceExactFile {
            target: abi(10),
            expected_before: abi(11),
            replacement: abi(12),
        }],
        vec![],
        verification,
        EffectRollbackV1::Journaled,
    )
    .map_err(|error| error.to_string())
}

fn artifact_set(
    assembly_manifest_digest: [u8; 32],
    source_root_digest: [u8; 32],
) -> Result<CanonicalArtifactSetV1, String> {
    let assembly = DigestV1::from_bytes(assembly_manifest_digest);
    let source_root = DigestV1::from_bytes(source_root_digest);
    let profile = DurableProfileV1::portable_strict();
    let specifications = [
        (ArtifactOwnerV1::FsZero, ZbfArtifactKindV1::FsPack, 31),
        (ArtifactOwnerV1::GraphZero, ZbfArtifactKindV1::GraphPack, 32),
        (ArtifactOwnerV1::TokenZero, ZbfArtifactKindV1::TokenPack, 33),
    ];
    let artifacts = specifications
        .into_iter()
        .map(|(owner, kind, producer)| {
            let bytes = ZbfObjectV1::new_leaf(
                kind,
                owner,
                assembly,
                profile,
                source_root,
                abi(producer),
                vec![producer],
            )
            .and_then(|object| object.to_bytes(profile))
            .map_err(|error| error.to_string())?;
            Ok(PeerArtifactInputV1 {
                bytes,
                expected_owner: owner,
                expected_kind: kind,
                expected_producer_contract_digest: digest(producer),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    CanonicalArtifactSetV1::verify(assembly_manifest_digest, source_root_digest, artifacts)
        .map_err(|error| error.to_string())
}

pub fn kernel_mutation_fixture_v2(
    surface: ExecutionSurface,
    assembly_manifest_digest: [u8; 32],
    source_root_digest: [u8; 32],
    source_head: String,
) -> Result<KernelMutationFixtureV2, String> {
    let plan = ControllerPlan {
        instructions: vec![
            ControllerInstruction::Dispatch {
                owner: PeerOwner::ZeroStack,
            },
            ControllerInstruction::Verify,
            ControllerInstruction::StageEffect,
            ControllerInstruction::BufferVisible,
            ControllerInstruction::CloseTransaction,
        ],
    };
    let plan_digest = plan.digest();
    let state_snapshot = abi(13);
    let program = effect_program(state_snapshot)?;
    let evidence_bytes = b"exact kernel conformance evidence";
    let certificate = certificate(evidence_bytes);
    let resident = Resident {
        bytes: evidence_bytes,
    };
    let verified = verify(&certificate, &resident).map_err(|error| error.to_string())?;
    let semantic_cut =
        SemanticCutEvidenceV1::verify_owner_scoped(plan_digest, digest(15), digest(4), &verified)
            .map_err(|error| error.to_string())?;
    let outcome = accept_effect_verification_v1(
        abi(70),
        &program,
        abi(71),
        abi(21),
        state_snapshot,
        abi(20),
        &verified,
    )
    .map_err(|error| error.to_string())?;
    let EffectVerificationOutcomeV1::Accepted(accepted) = outcome else {
        return Err("effect fixture was not accepted".into());
    };
    let action_digest = *accepted.action_digest().as_bytes();
    let acceptance_digest = *accepted.acceptance_digest().as_bytes();

    let project = TransactionResourceRequirementV1 {
        owner: ArtifactOwnerV1::FsZero,
        kind: TransactionResourceKindV1::ProjectFilesystem,
        scope_digest: abi(30),
        baseline_state_digest: state_snapshot,
        access: TransactionAccessV1::ReadWrite,
        authority_digest: abi(32),
    };
    let closure_request =
        EffectClosureRequestV1::new(&program, vec![project]).map_err(|error| error.to_string())?;
    let closure_manifest = EffectClosureManifestV1::new(
        &closure_request,
        vec![EffectResourceClosureV1 {
            requirement: project,
            isolation: ResourceIsolationModeV1::Journaled,
            restoration: ResourceRestorationModeV1::JournalRollback,
        }],
    )
    .map_err(|error| error.to_string())?;
    let boundary = validate_effect_closure_v1(&closure_request, &closure_manifest)
        .map_err(|error| error.to_string())?;
    let directory = tempdir().map_err(|error| error.to_string())?;
    let paths = JournalPathsV1::new(
        directory.path().join("root.json"),
        directory.path().join("journal.json"),
        directory.path().join("cartridge.json"),
        directory.path().join("owner.json"),
        directory.path().join("recovery.json"),
    )
    .map_err(|error| error.to_string())?;
    initialize_published_root_v1(&paths, state_snapshot).map_err(|error| error.to_string())?;
    let candidate_root = abi(10);
    let journal_binding = effect_journal_binding_v1(
        &boundary,
        DigestV1::from_bytes(assembly_manifest_digest),
        DurableProfileIdV1::PortableStrict,
        candidate_root,
        abi(69),
    )
    .map_err(|error| error.to_string())?;
    let transaction_receipt = begin_effect_transaction_v1(paths, journal_binding, &boundary)
        .map_err(|error| error.to_string())?
        .commit(&accepted)
        .map_err(|error| error.to_string())?;
    let transaction_closure =
        TransactionClosure::from_receipt(transaction_receipt).map_err(|error| error.to_string())?;

    let artifacts = artifact_set(assembly_manifest_digest, source_root_digest)?;
    let image_digest = artifacts.image_digest();
    let binding = ExecutionBinding {
        schema_version: TWO_PHASE_SCHEMA_VERSION,
        assembly_manifest_digest,
        source_tree_digest: source_root_digest,
        source_repository_heads: vec![SourceHead {
            repository: "ZeroStack".into(),
            head: source_head,
        }],
        image_digest,
        state_snapshot_digest: *state_snapshot.as_bytes(),
        task_fingerprint_digest: digest(14),
        plan_digest,
        fixed_model_digest: digest(15),
        comparison_identity_digest: digest(4),
        predecessor_receipt_head: digest(5),
    };
    let candidate_identity = DigestV1::from_bytes(candidate_protocol_identity_v1(&binding));
    let quality_certificate = ExactNeutralCertificateV1::verify(
        abi(14),
        abi(4),
        abi(16),
        candidate_identity,
        abi(17),
        abi(17),
        abi(18),
        abi(18),
        abi(19),
        abi(19),
    )
    .map_err(|error| error.to_string())?;
    let quality_admission = QualityAdmissionV1::admit_strict(
        QualityEvidenceV1::ExactNeutral(quality_certificate),
        FrozenBaselineV1::new(abi(16), abi(19), abi(20)).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let request = PrepareRequest {
        binding,
        surface,
        effect_class: EffectClass::ReversibleMutation,
        plan,
        envelope: WorkerEnvelope {
            fuel: 100,
            deadline_ms: 1_000,
            io_bytes: 1_024,
            output_bytes: 128,
            memory_bytes: 16 * 1_024 * 1_024,
            processes: 1,
            risk_units: 10,
            worker_steps: 8,
        },
        evidence: GuardEvidence {
            artifacts,
            semantic_cut,
            snap: SnapEvidence::NotClaimed,
            safety_shield: SafetyShieldEvidenceV1::from_effect_accepted(accepted)
                .map_err(|error| error.to_string())?,
            approval_grant_digest: None,
            irreversible_pre_action_evidence_digest: None,
            performance: quality_admission,
        },
    };
    Ok(KernelMutationFixtureV2 {
        request,
        staged_effect: StagedEffect {
            effect_digest: action_digest,
            effect_class: EffectClass::ReversibleMutation,
            acceptance_digest: Some(acceptance_digest),
            approval_grant_digest: None,
            pre_action_evidence_digest: None,
        },
        transaction_closure,
    })
}
