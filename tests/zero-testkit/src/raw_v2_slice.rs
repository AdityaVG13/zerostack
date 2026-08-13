//! Deterministic raw V2 vertical slice used as the frozen deoptimization baseline.
//!
//! This is a hub reference path, not a production engine or model benchmark. It
//! passes a full task and workspace through bounded raw-worker v2 frames. It applies
//! one exact code edit in an isolated candidate, verifies it, commits its journal
//! root, charges every observed frame byte to Baseline, and emits a stable receipt.
//! No compression, cache hit, quality gain, native
//! durability, or production model claim is made.

use std::{borrow::Cow, fmt};

use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Value, json};
use zero_abi::{
    ApprovalMetadata, ApprovalState, ArtifactOwnerV1, CallRequest, CausalWorkIrV1, CwirCoverageV1,
    CwirDeterminismV1, CwirEffectSpaceV1, CwirEpistemicProductV1, CwirFreshnessV1, CwirNodeKindV1,
    CwirNodeV1, CwirSoundnessV1, CwirStateAnchorV1, CwirTaskContractV1, CwirVerificationContractV1,
    CwirVerifierClassV1, DEFAULT_MAX_FRAME_BYTES, DigestV1, EffectPredicateV1, EffectProgramV1,
    EffectRollbackV1, EffectTargetV1, EffectVerificationPlanV1, EffectVerificationStepV1,
    EngineIdentity, HandshakeAck, HandshakeRequest, ProtocolLimits, RAW_WORKER_PROTOCOL_VERSION,
    RefOwnership, RevertMetadata, SnapshotIdentity, TypedEffectOperationV1, WorkerBinding,
    WorkerCapabilities, WorkerRequestFrame, WorkerResponseFrame, WorkerResult,
    WorkerResultMetadata, WorkerTrace, assembly_abi_contract_digest_v1, canonical_json,
    cwir_contract_digest_v1, decode_request_frame, decode_response_frame,
    effect_ir_contract_digest_v1, encode_frame, raw_worker_protocol_digest_hex, sha256,
    validate_handshake_request,
};
use zero_cert::{
    CommandId, CompletenessWitness, EffectAcceptedV1, EffectVerificationOutcomeV1,
    EvidenceCertificate, ObjectId, OperatorLock, Provenance, Query, Resolver, SpanRef,
    accept_effect_verification_v1, effect_witness_contract_digest_v1, verify,
};
use zero_gate::{
    EffectClosureManifestV1, EffectClosureRequestV1, EffectResourceClosureV1,
    ResourceIsolationModeV1, ResourceRestorationModeV1, TaskAcceptanceReceipt,
    TaskAcceptanceVerifier, TaskRunEvidence, TaskVerifierError, TransactionAccessV1,
    TransactionDispositionV1, TransactionResourceKindV1, TransactionResourceRequirementV1,
    begin_effect_transaction_v1, begin_task_attempt, effect_journal_binding_v1,
    transaction_contract_digest_v1, validate_effect_closure_v1, verify_task_acceptance,
};
use zero_ledger::{
    CausalCounterUnitV1, CausalWorkChargeV1, CausalWorkClassV1, CausalWorkOutcomeV1,
    CausalWorkReceiptV1, Digest as LedgerDigest, LedgerConfig, ParentCounterIdentityV1,
    ParentCounterObservationV1, ParentCounterWindowV1, ResiduePolicyV1, ResourceGauge, TokenCharge,
    TokenizerIdentity, causal_work_contract_digest_v1,
};
use zero_store::{DurableProfileIdV1, JournalPathsV1, SharedCas, initialize_published_root_v1};

pub const RAW_V2_SLICE_SCHEMA_VERSION_V1: u16 = 1;
pub const RAW_V2_SLICE_MAX_INPUT_BYTES_V1: usize = 64 * 1024;
pub const RAW_V2_REFERENCE_TASK_V1: &str = "replace_exact_42_with_43";

const PROJECT_ROOT_DOMAIN_V1: &[u8] = b"zerostack.raw_v2.project_root.v1\0";
const IDENTITY_DOMAIN_V1: &[u8] = b"zerostack.raw_v2.identity.v1\0";
const STATE_DOMAIN_V1: &[u8] = b"zerostack.raw_v2.state.v1\0";
const TASK_RECEIPT_DOMAIN_V1: &[u8] = b"zerostack.raw_v2.task_receipt.v1\0";
const SLICE_RECEIPT_DOMAIN_V1: &[u8] = b"zerostack.raw_v2.slice_receipt.v1\0";
const SLICE_CONTRACT_DOMAIN_V1: &[u8] = b"zerostack.raw_v2.slice_contract.v1\0";
const EFFECT_CLASS_ID_DOMAIN_V1: &[u8] = b"zerostack.raw_v2.effect_class.v1\0";
const GRAPH_RELATION_DOMAIN_V1: &[u8] = b"zerostack.raw_v2.graph_relation.v1\0";
const DECISION_VIEW_DOMAIN_V1: &[u8] = b"zerostack.raw_v2.decision_view.v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RawV2ExecutionModeV1 {
    UncompressedRawBaseline,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RawV2EvidenceModeV1 {
    DeterministicReference,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RawV2PublicationScopeV1 {
    JournalRootReferenceOnly,
}

/// Frozen hub contract for the uncompressed V2 deoptimization baseline.
pub fn raw_v2_slice_contract_manifest_v1() -> Value {
    json!({
        "causal_work_class": "baseline",
        "candidate_effect": "replace_exact_file",
        "decision_view_token_unit": "byte",
        "token_accounting": "locked_byte_tokenizer_raw_fallback",
        "evidence_mode": "deterministic_reference",
        "execution_mode": "uncompressed_raw_baseline",
        "linked_contracts": {
            "assembly_abi": assembly_abi_contract_digest_v1(),
            "causal_work": causal_work_contract_digest_v1(),
            "cwir": cwir_contract_digest_v1(),
            "effect_ir": effect_ir_contract_digest_v1(),
            "effect_witness": effect_witness_contract_digest_v1(),
            "raw_worker": raw_worker_protocol_digest_hex(),
            "transaction": transaction_contract_digest_v1(),
        },
        "max_input_bytes": RAW_V2_SLICE_MAX_INPUT_BYTES_V1,
        "name": "zerostack.raw_v2_slice.v1",
        "negative_space": [
            "atomic_workspace_publication",
            "cache_hit",
            "compression",
            "native_durability",
            "production_model_measurement",
            "quality_gain",
        ],
        "process_model": "one_shot_no_daemon",
        "snapshot_manifest": "single_file_cas_manifest_v1",
        "publication_scope": "journal_root_reference_only",
        "reference_task": RAW_V2_REFERENCE_TASK_V1,
        "receipt_fields": [
            "assembly_abi_digest",
            "baseline_frame_bytes",
            "baseline_identity_digest",
            "baseline_state_digest",
            "call_request_digest",
            "call_response_digest",
            "candidate_state_digest",
            "causal_work_receipt_digest",
            "cwir_semantic_digest",
            "decision_view_digest",
            "decision_view_tokens",
            "effect_acceptance_digest",
            "effect_action_digest",
            "evidence_certificate_digest",
            "evidence_mode",
            "execution_mode",
            "graph_relation_digest",
            "handshake_request_digest",
            "handshake_response_digest",
            "input_digest",
            "model_output_tokens",
            "output_digest",
            "publication_scope",
            "raw_worker_protocol_digest",
            "receipt_digest",
            "schema_version",
            "slice_contract_digest",
            "state_anchor_digest",
            "task_acceptance_digest",
            "task_digest",
            "token_ledger_digest",
            "transaction_receipt_digest",
        ],
        "schema_version": RAW_V2_SLICE_SCHEMA_VERSION_V1,
        "stages": [
            "exact_snapshot_read",
            "exact_source_relation",
            "exact_decision_view",
            "raw_worker_v2",
            "typed_effect_ir",
            "isolated_candidate",
            "verified_effect_acceptance",
            "transaction_gate",
            "journal_root_commit",
            "complete_receipt",
        ],
    })
}

pub fn raw_v2_slice_contract_digest_v1() -> DigestV1 {
    domain_digest(
        SLICE_CONTRACT_DOMAIN_V1,
        canonical_json(&raw_v2_slice_contract_manifest_v1()).as_bytes(),
    )
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawBaselineIdentityV1 {
    pub model_digest: DigestV1,
    pub decoder_digest: DigestV1,
    pub tokenizer_digest: DigestV1,
    pub query_contract_digest: DigestV1,
    pub action_contract_digest: DigestV1,
    pub backend_digest: DigestV1,
    pub verifier_digest: DigestV1,
    pub verifier_environment_digest: DigestV1,
    pub tool_surface_digest: DigestV1,
}

impl RawBaselineIdentityV1 {
    fn validate(self) -> Result<(), RawV2SliceErrorV1> {
        if [
            self.model_digest,
            self.decoder_digest,
            self.tokenizer_digest,
            self.query_contract_digest,
            self.action_contract_digest,
            self.backend_digest,
            self.verifier_digest,
            self.verifier_environment_digest,
            self.tool_surface_digest,
        ]
        .contains(&DigestV1::ZERO)
        {
            return Err(RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::InvalidIdentity,
                "raw baseline identity contains a zero digest",
            ));
        }
        Ok(())
    }

    pub fn digest(self) -> Result<DigestV1, RawV2SliceErrorV1> {
        self.validate()?;
        digest_value(IDENTITY_DOMAIN_V1, &self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawV2SliceInputV1 {
    pub task: String,
    pub workspace_bytes: Vec<u8>,
    pub graph_index_digest: DigestV1,
    pub toolchain_digest: DigestV1,
    pub runtime_manifest_digest: DigestV1,
    pub capability_surface_digest: DigestV1,
    pub identity: RawBaselineIdentityV1,
}

impl RawV2SliceInputV1 {
    fn validate(&self) -> Result<(), RawV2SliceErrorV1> {
        if self.task != RAW_V2_REFERENCE_TASK_V1 {
            return Err(RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::UnsupportedTask,
                "reference raw V2 slice accepts only replace_exact_42_with_43",
            ));
        }
        if self.workspace_bytes.is_empty()
            || self.workspace_bytes.len() > RAW_V2_SLICE_MAX_INPUT_BYTES_V1
        {
            return Err(RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::InputBounds,
                format!(
                    "workspace bytes must be in 1..={RAW_V2_SLICE_MAX_INPUT_BYTES_V1}, got {}",
                    self.workspace_bytes.len()
                ),
            ));
        }
        if [
            self.graph_index_digest,
            self.toolchain_digest,
            self.runtime_manifest_digest,
            self.capability_surface_digest,
        ]
        .contains(&DigestV1::ZERO)
        {
            return Err(RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::InvalidIdentity,
                "state anchor contains a zero digest",
            ));
        }
        self.identity.validate()?;
        let byte_tokenizer = digest_bytes(b"zerostack.raw_v2.byte_tokenizer.v1");
        if self.identity.tokenizer_digest != byte_tokenizer {
            return Err(RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::InvalidIdentity,
                "reference Decision View requires the frozen one-byte-one-token tokenizer",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawV2SliceReceiptV1 {
    schema_version: u16,
    execution_mode: RawV2ExecutionModeV1,
    evidence_mode: RawV2EvidenceModeV1,
    slice_contract_digest: DigestV1,
    assembly_abi_digest: DigestV1,
    raw_worker_protocol_digest: DigestV1,
    baseline_identity_digest: DigestV1,
    baseline_state_digest: DigestV1,
    state_anchor_digest: DigestV1,
    cwir_semantic_digest: DigestV1,
    graph_relation_digest: DigestV1,
    decision_view_digest: DigestV1,
    decision_view_tokens: u64,
    model_output_tokens: u64,
    token_ledger_digest: DigestV1,
    effect_action_digest: DigestV1,
    effect_acceptance_digest: DigestV1,
    transaction_receipt_digest: DigestV1,
    candidate_state_digest: DigestV1,
    publication_scope: RawV2PublicationScopeV1,
    handshake_request_digest: DigestV1,
    handshake_response_digest: DigestV1,
    call_request_digest: DigestV1,
    call_response_digest: DigestV1,
    task_digest: DigestV1,
    input_digest: DigestV1,
    output_digest: DigestV1,
    evidence_certificate_digest: DigestV1,
    task_acceptance_digest: DigestV1,
    causal_work_receipt_digest: DigestV1,
    baseline_frame_bytes: u64,
    receipt_digest: DigestV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawV2SliceReceiptWireV1 {
    schema_version: u16,
    execution_mode: RawV2ExecutionModeV1,
    evidence_mode: RawV2EvidenceModeV1,
    slice_contract_digest: DigestV1,
    assembly_abi_digest: DigestV1,
    raw_worker_protocol_digest: DigestV1,
    baseline_identity_digest: DigestV1,
    baseline_state_digest: DigestV1,
    state_anchor_digest: DigestV1,
    cwir_semantic_digest: DigestV1,
    graph_relation_digest: DigestV1,
    decision_view_digest: DigestV1,
    decision_view_tokens: u64,
    model_output_tokens: u64,
    token_ledger_digest: DigestV1,
    effect_action_digest: DigestV1,
    effect_acceptance_digest: DigestV1,
    transaction_receipt_digest: DigestV1,
    candidate_state_digest: DigestV1,
    publication_scope: RawV2PublicationScopeV1,
    handshake_request_digest: DigestV1,
    handshake_response_digest: DigestV1,
    call_request_digest: DigestV1,
    call_response_digest: DigestV1,
    task_digest: DigestV1,
    input_digest: DigestV1,
    output_digest: DigestV1,
    evidence_certificate_digest: DigestV1,
    task_acceptance_digest: DigestV1,
    causal_work_receipt_digest: DigestV1,
    baseline_frame_bytes: u64,
    receipt_digest: DigestV1,
}

impl<'de> Deserialize<'de> for RawV2SliceReceiptV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = RawV2SliceReceiptWireV1::deserialize(deserializer)?;
        let receipt = Self::from_wire(wire);
        receipt.validate().map_err(de::Error::custom)?;
        Ok(receipt)
    }
}

impl RawV2SliceReceiptV1 {
    pub const fn execution_mode(&self) -> RawV2ExecutionModeV1 {
        self.execution_mode
    }
    pub const fn input_digest(&self) -> DigestV1 {
        self.input_digest
    }
    pub const fn output_digest(&self) -> DigestV1 {
        self.output_digest
    }
    pub const fn cwir_semantic_digest(&self) -> DigestV1 {
        self.cwir_semantic_digest
    }
    pub const fn baseline_state_digest(&self) -> DigestV1 {
        self.baseline_state_digest
    }
    pub const fn candidate_state_digest(&self) -> DigestV1 {
        self.candidate_state_digest
    }
    pub const fn effect_action_digest(&self) -> DigestV1 {
        self.effect_action_digest
    }
    pub const fn effect_acceptance_digest(&self) -> DigestV1 {
        self.effect_acceptance_digest
    }
    pub const fn transaction_receipt_digest(&self) -> DigestV1 {
        self.transaction_receipt_digest
    }
    pub const fn publication_scope(&self) -> RawV2PublicationScopeV1 {
        self.publication_scope
    }
    pub const fn decision_view_tokens(&self) -> u64 {
        self.decision_view_tokens
    }
    pub const fn model_output_tokens(&self) -> u64 {
        self.model_output_tokens
    }
    pub const fn baseline_frame_bytes(&self) -> u64 {
        self.baseline_frame_bytes
    }
    pub const fn receipt_digest(&self) -> DigestV1 {
        self.receipt_digest
    }

    pub fn validate(&self) -> Result<(), RawV2SliceErrorV1> {
        if self.schema_version != RAW_V2_SLICE_SCHEMA_VERSION_V1
            || self.execution_mode != RawV2ExecutionModeV1::UncompressedRawBaseline
            || self.evidence_mode != RawV2EvidenceModeV1::DeterministicReference
            || self.publication_scope != RawV2PublicationScopeV1::JournalRootReferenceOnly
        {
            return Err(RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::UnsupportedVersion,
                "raw V2 slice receipt version or mode is unsupported",
            ));
        }
        if self.slice_contract_digest != raw_v2_slice_contract_digest_v1() {
            return Err(RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::UnsupportedVersion,
                "raw V2 slice receipt contract digest is not the current frozen baseline",
            ));
        }
        let raw_worker_protocol_digest = DigestV1::from_hex(&raw_worker_protocol_digest_hex())
            .map_err(|error| {
                RawV2SliceErrorV1::new(
                    RawV2SliceFailureCodeV1::ContractBindingMismatch,
                    format!("current raw-worker protocol digest is malformed: {error}"),
                )
            })?;
        if self.assembly_abi_digest != assembly_abi_contract_digest_v1()
            || self.raw_worker_protocol_digest != raw_worker_protocol_digest
            || self.task_digest != digest_bytes(RAW_V2_REFERENCE_TASK_V1.as_bytes())
        {
            return Err(RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::ContractBindingMismatch,
                "receipt assembly, raw-worker, or task identity differs from the frozen slice",
            ));
        }
        if [
            self.slice_contract_digest,
            self.assembly_abi_digest,
            self.raw_worker_protocol_digest,
            self.baseline_identity_digest,
            self.baseline_state_digest,
            self.state_anchor_digest,
            self.cwir_semantic_digest,
            self.graph_relation_digest,
            self.decision_view_digest,
            self.token_ledger_digest,
            self.effect_action_digest,
            self.effect_acceptance_digest,
            self.transaction_receipt_digest,
            self.candidate_state_digest,
            self.handshake_request_digest,
            self.handshake_response_digest,
            self.call_request_digest,
            self.call_response_digest,
            self.task_digest,
            self.input_digest,
            self.output_digest,
            self.evidence_certificate_digest,
            self.task_acceptance_digest,
            self.causal_work_receipt_digest,
        ]
        .contains(&DigestV1::ZERO)
            || self.baseline_frame_bytes == 0
            || self.decision_view_tokens == 0
            || self.model_output_tokens == 0
        {
            return Err(RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::IncompleteReceipt,
                "raw V2 slice receipt has a zero digest or zero work",
            ));
        }
        if self.input_digest == self.output_digest {
            return Err(RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::OutputMismatch,
                "code-edit output digest is unchanged from its input digest",
            ));
        }
        if self.baseline_state_digest != snapshot_root(self.input_digest)
            || self.candidate_state_digest != snapshot_root(self.output_digest)
            || self.baseline_state_digest == self.candidate_state_digest
        {
            return Err(RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::StateBindingMismatch,
                "receipt state roots do not bind the exact baseline and candidate CAS manifests",
            ));
        }
        if self.expected_digest()? != self.receipt_digest {
            return Err(RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::ReceiptDigestMismatch,
                "raw V2 slice receipt digest does not match its canonical body",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, RawV2SliceErrorV1> {
        self.validate()?;
        canonical_serialize(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, RawV2SliceErrorV1> {
        if bytes.len() > DEFAULT_MAX_FRAME_BYTES {
            return Err(RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::InputBounds,
                "raw V2 slice receipt exceeds the 1 MiB frame bound",
            ));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(serialization_error)?;
        if canonical_json(&value).as_bytes() != bytes {
            return Err(RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::NonCanonicalEncoding,
                "raw V2 slice receipt is not exact canonical JSON",
            ));
        }
        let wire: RawV2SliceReceiptWireV1 =
            serde_json::from_value(value).map_err(serialization_error)?;
        let receipt = Self::from_wire(wire);
        receipt.validate()?;
        Ok(receipt)
    }

    fn from_wire(wire: RawV2SliceReceiptWireV1) -> Self {
        Self {
            schema_version: wire.schema_version,
            execution_mode: wire.execution_mode,
            evidence_mode: wire.evidence_mode,
            slice_contract_digest: wire.slice_contract_digest,
            assembly_abi_digest: wire.assembly_abi_digest,
            raw_worker_protocol_digest: wire.raw_worker_protocol_digest,
            baseline_identity_digest: wire.baseline_identity_digest,
            baseline_state_digest: wire.baseline_state_digest,
            state_anchor_digest: wire.state_anchor_digest,
            cwir_semantic_digest: wire.cwir_semantic_digest,
            graph_relation_digest: wire.graph_relation_digest,
            decision_view_digest: wire.decision_view_digest,
            decision_view_tokens: wire.decision_view_tokens,
            model_output_tokens: wire.model_output_tokens,
            token_ledger_digest: wire.token_ledger_digest,
            effect_action_digest: wire.effect_action_digest,
            effect_acceptance_digest: wire.effect_acceptance_digest,
            transaction_receipt_digest: wire.transaction_receipt_digest,
            candidate_state_digest: wire.candidate_state_digest,
            publication_scope: wire.publication_scope,
            handshake_request_digest: wire.handshake_request_digest,
            handshake_response_digest: wire.handshake_response_digest,
            call_request_digest: wire.call_request_digest,
            call_response_digest: wire.call_response_digest,
            task_digest: wire.task_digest,
            input_digest: wire.input_digest,
            output_digest: wire.output_digest,
            evidence_certificate_digest: wire.evidence_certificate_digest,
            task_acceptance_digest: wire.task_acceptance_digest,
            causal_work_receipt_digest: wire.causal_work_receipt_digest,
            baseline_frame_bytes: wire.baseline_frame_bytes,
            receipt_digest: wire.receipt_digest,
        }
    }

    fn expected_digest(&self) -> Result<DigestV1, RawV2SliceErrorV1> {
        let mut value = serde_json::to_value(self).map_err(serialization_error)?;
        value
            .as_object_mut()
            .ok_or_else(|| {
                RawV2SliceErrorV1::new(
                    RawV2SliceFailureCodeV1::SerializationFailure,
                    "slice receipt must serialize as an object",
                )
            })?
            .remove("receipt_digest");
        Ok(domain_digest(
            SLICE_RECEIPT_DOMAIN_V1,
            canonical_json(&value).as_bytes(),
        ))
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct RawV2SliceRunV1 {
    pub output: Vec<u8>,
    pub receipt: RawV2SliceReceiptV1,
}

pub fn reference_raw_v2_input_v1() -> RawV2SliceInputV1 {
    let tagged = |label: &[u8]| DigestV1::from_bytes(sha256(label));
    RawV2SliceInputV1 {
        task: RAW_V2_REFERENCE_TASK_V1.into(),
        workspace_bytes: b"pub fn raw_baseline() -> u8 { 42 }\n".to_vec(),
        graph_index_digest: tagged(b"raw-v2-reference-graph-index"),
        toolchain_digest: tagged(b"raw-v2-reference-toolchain"),
        runtime_manifest_digest: tagged(b"raw-v2-reference-runtime-manifest"),
        capability_surface_digest: tagged(b"raw-v2-reference-capability-surface"),
        identity: RawBaselineIdentityV1 {
            model_digest: tagged(b"raw-v2-reference-model"),
            decoder_digest: tagged(b"raw-v2-reference-decoder"),
            tokenizer_digest: tagged(b"zerostack.raw_v2.byte_tokenizer.v1"),
            query_contract_digest: tagged(b"raw-v2-reference-query"),
            action_contract_digest: tagged(b"raw-v2-reference-action"),
            backend_digest: tagged(b"raw-v2-reference-backend"),
            verifier_digest: tagged(b"raw-v2-reference-verifier"),
            verifier_environment_digest: tagged(b"raw-v2-reference-verifier-environment"),
            tool_surface_digest: tagged(b"raw-v2-reference-tool-surface"),
        },
    }
}

struct ExactEditV1 {
    target_digest: DigestV1,
    candidate_bytes: Vec<u8>,
    candidate_digest: DigestV1,
    candidate_state: DigestV1,
    predicate_digest: DigestV1,
    graph_relation_digest: DigestV1,
    decision_view: Vec<u8>,
    decision_view_digest: DigestV1,
    decision_view_tokens: u64,
}

fn exact_edit(input: &RawV2SliceInputV1) -> Result<ExactEditV1, RawV2SliceErrorV1> {
    const BEFORE: &[u8] = b"pub fn raw_baseline() -> u8 { 42 }";
    const AFTER: &[u8] = b"pub fn raw_baseline() -> u8 { 43 }";
    let positions = input
        .workspace_bytes
        .windows(BEFORE.len())
        .enumerate()
        .filter_map(|(index, bytes)| (bytes == BEFORE).then_some(index))
        .collect::<Vec<_>>();
    if positions.len() != 1 {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::InputDrift,
            format!(
                "reference source must contain one exact raw_baseline fixture, found {}",
                positions.len()
            ),
        ));
    }
    let start = positions[0];
    let mut candidate_bytes = input.workspace_bytes.clone();
    candidate_bytes.splice(start..start + BEFORE.len(), AFTER.iter().copied());
    let target_digest = domain_digest(b"zerostack.raw_v2.target.v1\0", b"src/lib.rs");
    let input_digest = digest_bytes(&input.workspace_bytes);
    let candidate_digest = digest_bytes(&candidate_bytes);
    let candidate_state = snapshot_root(candidate_digest);
    let graph_relation = json!({
        "byte_len": 2,
        "byte_start": start + BEFORE.len() - 4,
        "class": "exact_source_relation",
        "source_digest": input_digest,
        "symbol": "raw_baseline",
        "target_digest": target_digest,
        "value": "42",
    });
    let graph_relation_digest = digest_value(GRAPH_RELATION_DOMAIN_V1, &graph_relation)?;
    let decision_view_value = json!({
        "graph_relation": graph_relation,
        "graph_relation_digest": graph_relation_digest,
        "task": input.task,
        "target": "src/lib.rs",
        "workspace_hex": lower_hex(&input.workspace_bytes),
    });
    let decision_view = canonical_json(&decision_view_value).into_bytes();
    let decision_view_digest = domain_digest(DECISION_VIEW_DOMAIN_V1, &decision_view);
    let decision_view_tokens = u64::try_from(decision_view.len()).map_err(|_| {
        RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::WorkOverflow,
            "decision-view byte-token count does not fit u64",
        )
    })?;
    let predicate_digest = domain_digest(
        b"zerostack.raw_v2.output_predicate.v1\0",
        candidate_digest.as_bytes(),
    );
    Ok(ExactEditV1 {
        target_digest,
        candidate_bytes,
        candidate_digest,
        candidate_state,
        predicate_digest,
        graph_relation_digest,
        decision_view,
        decision_view_digest,
        decision_view_tokens,
    })
}

pub fn run_raw_v2_slice_v1(
    input: &RawV2SliceInputV1,
) -> Result<RawV2SliceRunV1, RawV2SliceErrorV1> {
    input.validate()?;
    let session = tempfile::tempdir().map_err(stage_error("slice_session"))?;
    let cas = SharedCas::open(session.path().join("cas"));
    let baseline_object = cas
        .put(&input.workspace_bytes)
        .map_err(stage_error("cas_put_baseline"))?;
    if cas
        .get_verified(&baseline_object)
        .map_err(stage_error("cas_read_baseline"))?
        != input.workspace_bytes
    {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::EvidenceFailure,
            "CAS exact baseline read differs from the submitted workspace",
        ));
    }
    let input_digest = digest_bytes(&input.workspace_bytes);
    let snapshot = snapshot_root(input_digest);
    let baseline_manifest = snapshot_manifest_bytes(input_digest);
    let baseline_root_object = cas
        .put(&baseline_manifest)
        .map_err(stage_error("cas_put_baseline_manifest"))?;
    if baseline_root_object != snapshot.to_hex()
        || cas
            .get_verified(&baseline_root_object)
            .map_err(stage_error("cas_read_baseline_manifest"))?
            != baseline_manifest
    {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::EvidenceFailure,
            "baseline snapshot root does not name its exact CAS manifest",
        ));
    }
    let edit = exact_edit(input)?;
    let assembly_abi_digest = assembly_abi_contract_digest_v1();
    let raw_worker_protocol_digest = DigestV1::from_hex(&raw_worker_protocol_digest_hex())
        .map_err(|error| {
            RawV2SliceErrorV1::new(RawV2SliceFailureCodeV1::InvalidIdentity, error.to_string())
        })?;
    let task_digest = digest_bytes(input.task.as_bytes());
    let project_root = domain_digest(PROJECT_ROOT_DOMAIN_V1, &input.workspace_bytes);
    let state = CwirStateAnchorV1 {
        project_root,
        fs_snapshot: snapshot,
        graph_indexed_through: input.graph_index_digest,
        toolchain: input.toolchain_digest,
        runtime_manifest: input.runtime_manifest_digest,
        capability_surface: input.capability_surface_digest,
    };
    let state_anchor_digest = digest_value(STATE_DOMAIN_V1, &state)?;
    let cwir = build_raw_cwir(input, &edit, state, task_digest, input_digest)?;

    let (handshake_request_bytes, handshake_response_bytes, binding) =
        reference_handshake(raw_worker_protocol_digest)?;
    let call = WorkerRequestFrame::Call {
        request: CallRequest {
            request_id: "raw-v2-request-1".into(),
            op: "baseline.apply_exact_edit".into(),
            args: json!({
                "decision_view_hex": lower_hex(&edit.decision_view),
                "input_hex": lower_hex(&input.workspace_bytes),
                "snapshot": snapshot,
                "task_hex": lower_hex(input.task.as_bytes()),
            }),
            deadline_unix_ms: Some(30_000),
            trace: reference_trace(raw_worker_protocol_digest),
            approval_grant: None,
            telemetry_request: None,
        },
    };
    let call_request_bytes =
        encode_frame(&call, DEFAULT_MAX_FRAME_BYTES).map_err(stage_error("call_encode"))?;
    let call_response_bytes =
        execute_reference_worker(&call_request_bytes, &binding, input, &edit, snapshot)?;
    let (output, effect) = decode_reference_output(&call_response_bytes, input, &edit)?;
    let candidate_object = cas.put(&output).map_err(stage_error("cas_put_candidate"))?;
    if cas
        .get_verified(&candidate_object)
        .map_err(stage_error("cas_read_candidate"))?
        != output
    {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::EvidenceFailure,
            "CAS exact candidate read differs from the verified output",
        ));
    }
    let candidate_manifest = snapshot_manifest_bytes(digest_bytes(&output));
    let candidate_root_object = cas
        .put(&candidate_manifest)
        .map_err(stage_error("cas_put_candidate_manifest"))?;
    if candidate_root_object != edit.candidate_state.to_hex()
        || cas
            .get_verified(&candidate_root_object)
            .map_err(stage_error("cas_read_candidate_manifest"))?
            != candidate_manifest
    {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::EvidenceFailure,
            "candidate snapshot root does not name its exact CAS manifest",
        ));
    }
    let output_digest = digest_bytes(&output);
    let (token_ledger_digest, model_output_tokens) =
        account_model_tokens(input, &edit, &effect, &output)?;

    let (evidence_certificate_digest, task_receipt, accepted_effect) =
        verify_exact_output_and_task(input, &edit, &cwir, &effect, snapshot, &output)?;
    let transaction_receipt_digest = commit_exact_effect(
        input,
        &edit,
        &effect,
        &accepted_effect,
        snapshot,
        project_root,
        session.path(),
    )?;
    let task_acceptance_digest = digest_task_receipt(&task_receipt)?;
    let baseline_frame_bytes = u64::try_from(
        handshake_request_bytes.len()
            + handshake_response_bytes.len()
            + call_request_bytes.len()
            + call_response_bytes.len(),
    )
    .map_err(|_| {
        RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::WorkOverflow,
            "raw frame byte count does not fit u64",
        )
    })?;
    let causal_work_receipt_digest = charge_raw_baseline(
        assembly_abi_digest,
        raw_worker_protocol_digest,
        input.identity.digest()?,
        digest_bytes(&call_request_bytes),
        baseline_frame_bytes,
    )?;

    let mut receipt = RawV2SliceReceiptV1 {
        schema_version: RAW_V2_SLICE_SCHEMA_VERSION_V1,
        execution_mode: RawV2ExecutionModeV1::UncompressedRawBaseline,
        evidence_mode: RawV2EvidenceModeV1::DeterministicReference,
        slice_contract_digest: raw_v2_slice_contract_digest_v1(),
        assembly_abi_digest,
        raw_worker_protocol_digest,
        baseline_identity_digest: input.identity.digest()?,
        baseline_state_digest: snapshot,
        state_anchor_digest,
        cwir_semantic_digest: cwir.semantic_digest(),
        graph_relation_digest: edit.graph_relation_digest,
        decision_view_digest: edit.decision_view_digest,
        decision_view_tokens: edit.decision_view_tokens,
        model_output_tokens,
        token_ledger_digest,
        effect_action_digest: effect.action_digest(),
        effect_acceptance_digest: accepted_effect.acceptance_digest(),
        transaction_receipt_digest,
        candidate_state_digest: edit.candidate_state,
        publication_scope: RawV2PublicationScopeV1::JournalRootReferenceOnly,
        handshake_request_digest: digest_bytes(&handshake_request_bytes),
        handshake_response_digest: digest_bytes(&handshake_response_bytes),
        call_request_digest: digest_bytes(&call_request_bytes),
        call_response_digest: digest_bytes(&call_response_bytes),
        task_digest,
        input_digest,
        output_digest,
        evidence_certificate_digest,
        task_acceptance_digest,
        causal_work_receipt_digest,
        baseline_frame_bytes,
        receipt_digest: DigestV1::ZERO,
    };
    receipt.receipt_digest = receipt.expected_digest()?;
    receipt.validate()?;
    Ok(RawV2SliceRunV1 { output, receipt })
}

fn build_raw_cwir(
    input: &RawV2SliceInputV1,
    edit: &ExactEditV1,
    state: CwirStateAnchorV1,
    task_digest: DigestV1,
    input_digest: DigestV1,
) -> Result<CausalWorkIrV1, RawV2SliceErrorV1> {
    let task = CwirTaskContractV1::new(&input.task, task_digest, state.fs_snapshot)
        .map_err(stage_error("cwir_task"))?;
    let exact = |authority| CwirEpistemicProductV1 {
        authority,
        soundness: CwirSoundnessV1::Exact,
        coverage: CwirCoverageV1::Complete,
        freshness: CwirFreshnessV1::Current,
        determinism: CwirDeterminismV1::Deterministic,
    };
    let state_node = CwirNodeV1::new(
        CwirNodeKindV1::State,
        input_digest,
        Some(state.fs_snapshot),
        true,
        exact(ArtifactOwnerV1::FsZero),
        vec![],
    )
    .map_err(stage_error("cwir_state"))?;
    let graph_node = CwirNodeV1::new(
        CwirNodeKindV1::Claim,
        edit.graph_relation_digest,
        Some(state.fs_snapshot),
        true,
        exact(ArtifactOwnerV1::GraphZero),
        vec![state_node.id],
    )
    .map_err(stage_error("cwir_graph_relation"))?;
    let decision_node = CwirNodeV1::new(
        CwirNodeKindV1::Contract,
        edit.decision_view_digest,
        Some(state.fs_snapshot),
        true,
        exact(ArtifactOwnerV1::TokenZero),
        vec![graph_node.id],
    )
    .map_err(stage_error("cwir_decision_view"))?;
    CausalWorkIrV1::new(
        task,
        state,
        vec![state_node, graph_node, decision_node],
        vec![],
        vec![],
        CwirEffectSpaceV1::new(
            vec![domain_digest(
                EFFECT_CLASS_ID_DOMAIN_V1,
                b"replace_exact_file",
            )],
            vec![],
        )
        .map_err(stage_error("cwir_effect_space"))?,
        CwirVerificationContractV1 {
            verifier_digest: input.identity.verifier_digest,
            predicate_digest: edit.predicate_digest,
            scope_digest: state.fs_snapshot,
            class: CwirVerifierClassV1::ExactChecker,
        },
        vec![],
    )
    .map_err(stage_error("cwir"))
}

fn build_effect(
    input: &RawV2SliceInputV1,
    edit: &ExactEditV1,
    snapshot: DigestV1,
) -> Result<EffectProgramV1, RawV2SliceErrorV1> {
    let target = EffectTargetV1 {
        owner: ArtifactOwnerV1::FsZero,
        target_digest: edit.target_digest,
        required_snapshot: snapshot,
    };
    let precondition = EffectPredicateV1 {
        predicate_digest: digest_bytes(&input.workspace_bytes),
        scope_digest: edit.target_digest,
        required_snapshot: snapshot,
    };
    let verification = EffectVerificationStepV1 {
        verifier_digest: input.identity.verifier_digest,
        predicate_digest: edit.predicate_digest,
        environment_digest: input.identity.verifier_environment_digest,
        required_snapshot: snapshot,
        verifier_class: CwirVerifierClassV1::ExactChecker,
    };
    EffectProgramV1::new(
        snapshot,
        "replace_exact_42_with_43",
        vec![target],
        vec![precondition],
        vec![TypedEffectOperationV1::ReplaceExactFile {
            target: edit.target_digest,
            expected_before: digest_bytes(&input.workspace_bytes),
            replacement: edit.candidate_digest,
        }],
        vec![],
        EffectVerificationPlanV1::new(vec![verification]).map_err(stage_error("effect_plan"))?,
        EffectRollbackV1::WorkspaceClone,
    )
    .map_err(stage_error("effect"))
}

fn commit_exact_effect(
    input: &RawV2SliceInputV1,
    edit: &ExactEditV1,
    effect: &EffectProgramV1,
    accepted: &EffectAcceptedV1,
    snapshot: DigestV1,
    project_root: DigestV1,
    session_root: &std::path::Path,
) -> Result<DigestV1, RawV2SliceErrorV1> {
    let resource = TransactionResourceRequirementV1 {
        owner: ArtifactOwnerV1::FsZero,
        kind: TransactionResourceKindV1::ProjectFilesystem,
        scope_digest: project_root,
        baseline_state_digest: snapshot,
        access: TransactionAccessV1::ReadWrite,
        authority_digest: assembly_abi_contract_digest_v1(),
    };
    let request = EffectClosureRequestV1::new(effect, vec![resource])
        .map_err(stage_error("transaction_request"))?;
    let manifest = EffectClosureManifestV1::new(
        &request,
        vec![EffectResourceClosureV1 {
            requirement: resource,
            // The reference candidate is fully materialized before commit. This
            // journal root is not a claim of atomic multi-file publication.
            isolation: ResourceIsolationModeV1::Buffered,
            restoration: ResourceRestorationModeV1::NotNeeded,
        }],
    )
    .map_err(stage_error("transaction_manifest"))?;
    let boundary =
        validate_effect_closure_v1(&request, &manifest).map_err(stage_error("transaction_gate"))?;
    let paths = JournalPathsV1::new(
        session_root.join("root.json"),
        session_root.join("journal.json"),
        session_root.join("cartridge.json"),
        session_root.join("owner-death.json"),
        session_root.join("recovery.json"),
    )
    .map_err(stage_error("journal_paths"))?;
    initialize_published_root_v1(&paths, snapshot).map_err(stage_error("journal_initialize"))?;
    let binding = effect_journal_binding_v1(
        &boundary,
        assembly_abi_contract_digest_v1(),
        DurableProfileIdV1::PortableStrict,
        edit.candidate_state,
        input.identity.digest()?,
    )
    .map_err(stage_error("journal_binding"))?;
    let receipt = begin_effect_transaction_v1(paths, binding, &boundary)
        .map_err(stage_error("transaction_begin"))?
        .commit(accepted)
        .map_err(stage_error("transaction_commit"))?;
    if receipt.disposition() != TransactionDispositionV1::CandidateCommitted
        || receipt.observed_root() != edit.candidate_state
        || receipt.acceptance_digest() != Some(accepted.acceptance_digest())
    {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::TransactionFailure,
            "effect transaction did not commit the accepted candidate journal root",
        ));
    }
    receipt
        .canonical_bytes()
        .map_err(stage_error("transaction_receipt"))?;
    Ok(receipt.receipt_digest())
}

fn reference_handshake(
    protocol_digest: DigestV1,
) -> Result<(Vec<u8>, Vec<u8>, WorkerBinding), RawV2SliceErrorV1> {
    let contract = assembly_abi_contract_digest_v1().to_hex();
    let registry = DigestV1::from_bytes(sha256(b"raw-v2-reference-registry")).to_hex();
    let binding = WorkerBinding {
        engine: EngineIdentity::FsZero,
        root: "/raw-v2-reference".into(),
        session_id: "raw-v2-reference-session".into(),
        worker_revision: "raw-v2-reference-worker-v1".into(),
        semantic_contract_version: "1".into(),
        semantic_contract_digest: contract.clone(),
        operation_registry_digest: registry.clone(),
        ref_scheme: "fz".into(),
    };
    let request = HandshakeRequest {
        protocol_version: RAW_WORKER_PROTOCOL_VERSION.into(),
        root: binding.root.clone(),
        session_id: binding.session_id.clone(),
        expected_engine: EngineIdentity::FsZero,
        expected_worker_revision: Some(binding.worker_revision.clone()),
        expected_contract_digest: contract,
        expected_registry_digest: Some(registry),
    };
    validate_handshake_request(&request, &binding).map_err(stage_error("handshake_binding"))?;
    let request_frame = WorkerRequestFrame::Handshake {
        request: request.clone(),
    };
    let request_bytes = encode_frame(&request_frame, DEFAULT_MAX_FRAME_BYTES)
        .map_err(stage_error("handshake_encode"))?;
    let decoded = decode_request_frame(&request_bytes, DEFAULT_MAX_FRAME_BYTES)
        .map_err(stage_error("handshake_decode"))?;
    if decoded != request_frame {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::WorkerProtocol,
            "handshake frame changed across encode/decode",
        ));
    }
    let response = WorkerResponseFrame::HandshakeAck {
        ack: HandshakeAck {
            protocol_version: RAW_WORKER_PROTOCOL_VERSION.into(),
            binding: binding.clone(),
            capabilities: WorkerCapabilities {
                cancellation: true,
                deadlines: true,
                approvals: false,
                revert: false,
                snapshots: true,
            },
            limits: ProtocolLimits::default(),
            protocol_digest: protocol_digest.to_hex(),
        },
    };
    let response_bytes = encode_frame(&response, DEFAULT_MAX_FRAME_BYTES)
        .map_err(stage_error("handshake_response"))?;
    if decode_response_frame(&response_bytes, DEFAULT_MAX_FRAME_BYTES)
        .map_err(stage_error("handshake_response_decode"))?
        != response
    {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::WorkerProtocol,
            "handshake response changed across encode/decode",
        ));
    }
    Ok((request_bytes, response_bytes, binding))
}

fn reference_trace(protocol_digest: DigestV1) -> WorkerTrace {
    WorkerTrace {
        runtime_id: "raw-v2-reference-runtime".into(),
        cell_id: "raw-v2-reference-cell".into(),
        request_id: "raw-v2-request-1".into(),
        trace_id: "raw-v2-reference-trace".into(),
        parent_span_id: None,
        worker_revision: "raw-v2-reference-worker-v1".into(),
        contract_digest: protocol_digest.to_hex(),
    }
}

fn execute_reference_worker(
    request_bytes: &[u8],
    binding: &WorkerBinding,
    input: &RawV2SliceInputV1,
    edit: &ExactEditV1,
    snapshot: DigestV1,
) -> Result<Vec<u8>, RawV2SliceErrorV1> {
    let WorkerRequestFrame::Call { request } =
        decode_request_frame(request_bytes, DEFAULT_MAX_FRAME_BYTES)
            .map_err(stage_error("worker_decode"))?
    else {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::WorkerProtocol,
            "reference worker received a non-call frame",
        ));
    };
    if request.op != "baseline.apply_exact_edit" || request.deadline_expired(29_999) {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::WorkerProtocol,
            "reference worker operation or deadline is invalid",
        ));
    }
    let args = request.args.as_object().ok_or_else(|| {
        RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::WorkerProtocol,
            "reference worker args must be an object",
        )
    })?;
    if args.len() != 4 {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::WorkerProtocol,
            "reference worker args contain unknown or missing fields",
        ));
    }
    let decision_view = decode_hex_field(args, "decision_view_hex")?;
    let raw_input = decode_hex_field(args, "input_hex")?;
    let task = decode_hex_field(args, "task_hex")?;
    let requested_snapshot: DigestV1 =
        serde_json::from_value(args.get("snapshot").cloned().ok_or_else(|| {
            RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::WorkerProtocol,
                "reference worker snapshot is missing",
            )
        })?)
        .map_err(serialization_error)?;
    if decision_view != edit.decision_view
        || raw_input != input.workspace_bytes
        || task != RAW_V2_REFERENCE_TASK_V1.as_bytes()
        || requested_snapshot != snapshot
    {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::OutputMismatch,
            "reference worker decision view, input, task, or snapshot binding differs",
        ));
    }
    let effect = build_effect(input, edit, snapshot)?;
    let effect_bytes = effect
        .canonical_bytes()
        .map_err(stage_error("worker_effect_encode"))?;
    let response = WorkerResponseFrame::Result {
        request_id: request.request_id.clone(),
        result: WorkerResult {
            value: json!({
                "candidate_hex": lower_hex(&edit.candidate_bytes),
                "effect_program_hex": lower_hex(&effect_bytes),
                "output_digest": edit.candidate_digest,
                "task": RAW_V2_REFERENCE_TASK_V1,
            }),
            metadata: WorkerResultMetadata {
                effect: zero_abi::EffectClass::ReversibleMutation,
                approval: ApprovalMetadata {
                    state: ApprovalState::NotRequired,
                    approval_id: None,
                    policy: None,
                },
                revert: RevertMetadata {
                    supported: true,
                    journal_id: Some(edit.candidate_state.to_hex()),
                    rollback_op: Some("restore_workspace_clone".into()),
                },
                ownership: RefOwnership {
                    engine: EngineIdentity::FsZero,
                    session_id: binding.session_id.clone(),
                    refs: vec![format!("fz://blob/{}", edit.candidate_digest.to_hex())],
                    snapshot: Some(SnapshotIdentity {
                        kind: "candidate_overlay".into(),
                        id: edit.candidate_state.to_hex(),
                        digest: Some(edit.candidate_state.to_hex()),
                    }),
                },
                trace: request.trace,
            },
        },
        engine_timeline: None,
        worker_token_accounting: None,
    };
    encode_frame(&response, DEFAULT_MAX_FRAME_BYTES).map_err(stage_error("worker_encode"))
}

fn decode_reference_output(
    response_bytes: &[u8],
    input: &RawV2SliceInputV1,
    edit: &ExactEditV1,
) -> Result<(Vec<u8>, EffectProgramV1), RawV2SliceErrorV1> {
    let WorkerResponseFrame::Result {
        request_id, result, ..
    } = decode_response_frame(response_bytes, DEFAULT_MAX_FRAME_BYTES)
        .map_err(stage_error("response_decode"))?
    else {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::WorkerProtocol,
            "reference worker did not return a result frame",
        ));
    };
    if request_id != "raw-v2-request-1"
        || result.metadata.effect != zero_abi::EffectClass::ReversibleMutation
        || result.metadata.ownership.engine != EngineIdentity::FsZero
        || !result.metadata.revert.supported
    {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::WorkerProtocol,
            "reference response metadata differs from the frozen raw route",
        ));
    }
    let value = result.value.as_object().ok_or_else(|| {
        RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::WorkerProtocol,
            "reference response value must be an object",
        )
    })?;
    if value.len() != 4
        || value.get("task").and_then(Value::as_str) != Some(RAW_V2_REFERENCE_TASK_V1)
    {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::WorkerProtocol,
            "reference response value has unknown fields or task",
        ));
    }
    let output = decode_hex_field(value, "candidate_hex")?;
    let effect_bytes = decode_hex_field(value, "effect_program_hex")?;
    let effect = EffectProgramV1::from_canonical_bytes(&effect_bytes)
        .map_err(stage_error("response_effect_decode"))?;
    let declared: DigestV1 =
        serde_json::from_value(value.get("output_digest").cloned().ok_or_else(|| {
            RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::WorkerProtocol,
                "reference response output digest is missing",
            )
        })?)
        .map_err(serialization_error)?;
    let expected_effect = build_effect(
        input,
        edit,
        snapshot_root(digest_bytes(&input.workspace_bytes)),
    )?;
    if output != edit.candidate_bytes
        || declared != digest_bytes(&output)
        || effect != expected_effect
    {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::OutputMismatch,
            "reference candidate or typed Effect IR differs from the exact code edit",
        ));
    }
    Ok((output, effect))
}

struct SliceResolver<'a> {
    bytes: &'a [u8],
}

impl Resolver for SliceResolver<'_> {
    fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
        (sha256(self.bytes) == object_id.0).then_some(self.bytes)
    }
    fn trusted_operator_version<'a>(&'a self, id: &str) -> Option<&'a str> {
        (id == "raw-v2-exact-output").then_some("1")
    }
    fn trusted_parser_version<'a>(&'a self, id: &str) -> Option<&'a str> {
        (id == "raw-v2-bytes").then_some("1")
    }
    fn trusted_index_version<'a>(&'a self, id: &str) -> Option<&'a str> {
        (id == "raw-v2-cas").then_some("1")
    }
}

struct ExactTaskVerifier {
    task_id: u64,
    verifier_environment_digest: [u8; 32],
    journal_id: [u8; 32],
}

impl TaskAcceptanceVerifier for ExactTaskVerifier {
    fn verify_run(&self, evidence: &TaskRunEvidence) -> Result<(), TaskVerifierError> {
        if evidence.task_id() == self.task_id
            && evidence.verifier() == CommandId(1)
            && evidence.verifier_environment_digest() == &self.verifier_environment_digest
            && evidence.journal_id() == &self.journal_id
        {
            Ok(())
        } else {
            Err(TaskVerifierError::UntrustedRunEvidence)
        }
    }
}

fn verify_exact_output_and_task(
    input: &RawV2SliceInputV1,
    edit: &ExactEditV1,
    cwir: &CausalWorkIrV1,
    effect: &EffectProgramV1,
    snapshot: DigestV1,
    output: &[u8],
) -> Result<(DigestV1, TaskAcceptanceReceipt, EffectAcceptedV1), RawV2SliceErrorV1> {
    if output != edit.candidate_bytes {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::OutputMismatch,
            "verified candidate differs from the isolated exact edit",
        ));
    }
    let object = sha256(output);
    let span = SpanRef {
        object_id: ObjectId(object),
        object_digest: object,
        byte_start: 0,
        byte_len: output.len() as u64,
        span_digest: object,
    };
    let certificate = EvidenceCertificate {
        query: Query::ReadSpan(span.clone()),
        spans: vec![span],
        payload: Cow::Borrowed(output),
        provenance: Provenance {
            parser_id: "raw-v2-bytes".into(),
            parser_version: "1".into(),
            index_id: "raw-v2-cas".into(),
            index_version: "1".into(),
            operator_id: "raw-v2-exact-output".into(),
            operator_version: "1".into(),
        },
        completeness: CompletenessWitness::ReadSpan {
            operator: OperatorLock {
                operator_id: "raw-v2-exact-output".into(),
                operator_version: "1".into(),
            },
        },
        input_token_cost: edit.decision_view_tokens,
        backend_work_units: output.len() as u64,
    };
    let resolver = SliceResolver { bytes: output };
    let verified_evidence = verify(&certificate, &resolver).map_err(stage_error("zero_cert"))?;
    if verified_evidence.payload() != output {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::EvidenceFailure,
            "zero-cert verified payload differs from the candidate output",
        ));
    }
    let accepted = match accept_effect_verification_v1(
        assembly_abi_contract_digest_v1(),
        effect,
        cwir.semantic_digest(),
        edit.predicate_digest,
        snapshot,
        input.identity.verifier_digest,
        &verified_evidence,
    )
    .map_err(stage_error("effect_acceptance"))?
    {
        EffectVerificationOutcomeV1::Accepted(accepted) => accepted,
        EffectVerificationOutcomeV1::Rejected(_) | EffectVerificationOutcomeV1::Incomplete(_) => {
            return Err(RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::EvidenceFailure,
                "exact candidate evidence did not produce effect acceptance",
            ));
        }
    };
    let certificate_digest = DigestV1::from_bytes(
        certificate
            .canonical_digest()
            .map_err(stage_error("certificate_digest"))?,
    );
    let task_digest = digest_bytes(input.task.as_bytes());
    let task_id = u64::from_be_bytes(task_digest.as_bytes()[..8].try_into().map_err(|_| {
        RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::EvidenceFailure,
            "task digest prefix is unavailable",
        )
    })?);
    let journal_id = domain_digest(
        b"zerostack.raw_v2.task_journal.v1\0",
        effect.action_digest().as_bytes(),
    );
    let evidence = TaskRunEvidence::new(
        task_id,
        CommandId(1),
        *input.identity.verifier_environment_digest.as_bytes(),
        0,
        vec![*edit.candidate_digest.as_bytes()],
        vec![*digest_bytes(output).as_bytes()],
        *journal_id.as_bytes(),
        output.len() as u64,
    );
    let attempt = begin_task_attempt(zero_abi::EffectClass::ReversibleMutation, evidence)
        .map_err(stage_error("task_attempt"))?;
    let verifier = ExactTaskVerifier {
        task_id,
        verifier_environment_digest: *input.identity.verifier_environment_digest.as_bytes(),
        journal_id: *journal_id.as_bytes(),
    };
    let verified_task = verify_task_acceptance(&verifier, attempt).map_err(|error| {
        RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::TaskAcceptanceFailure,
            format!("task verification failed: {:?}", error.reason()),
        )
    })?;
    Ok((certificate_digest, verified_task.into_receipt(), accepted))
}

fn digest_task_receipt(receipt: &TaskAcceptanceReceipt) -> Result<DigestV1, RawV2SliceErrorV1> {
    digest_value(
        TASK_RECEIPT_DOMAIN_V1,
        &json!({
            "attempt_cost": receipt.attempt_cost(),
            "exit_code": receipt.exit_code(),
            "expected_artifact_digests": receipt.expected_artifact_digests(),
            "journal_id": receipt.journal_id(),
            "observed_artifact_digests": receipt.observed_artifact_digests(),
            "outcome": format!("{:?}", receipt.outcome()).to_lowercase(),
            "task_id": receipt.task_id(),
            "verifier": receipt.verifier().0,
            "verifier_environment_digest": receipt.verifier_environment_digest(),
        }),
    )
}

fn account_model_tokens(
    input: &RawV2SliceInputV1,
    edit: &ExactEditV1,
    effect: &EffectProgramV1,
    output: &[u8],
) -> Result<(DigestV1, u64), RawV2SliceErrorV1> {
    let input_tokens = edit.decision_view_tokens;
    let effect_bytes = effect
        .canonical_bytes()
        .map_err(stage_error("token_effect_bytes"))?;
    let model_output_tokens = u64::try_from(effect_bytes.len())
        .ok()
        .and_then(|effect_len| {
            u64::try_from(output.len())
                .ok()
                .and_then(|output_len| effect_len.checked_add(output_len))
        })
        .ok_or_else(|| {
            RawV2SliceErrorV1::new(
                RawV2SliceFailureCodeV1::WorkOverflow,
                "reference model output byte-token count overflowed u64",
            )
        })?;
    let tokenizer = TokenizerIdentity::new(
        "zerostack-byte-v1",
        LedgerDigest(*input.identity.tokenizer_digest.as_bytes()),
    );
    let mut gauge = ResourceGauge::new(LedgerConfig::new(tokenizer.clone()));
    gauge
        .charge(
            &tokenizer,
            &TokenCharge {
                raw_input_tokens: input_tokens,
                input_tokens,
                fallback_tokens: input_tokens,
                model_output_tokens,
                model_calls: 1,
                ..TokenCharge::default()
            },
        )
        .map_err(stage_error("token_charge"))?;
    if gauge
        .ledger()
        .check_accounting_complete()
        .map_err(stage_error("token_conservation"))?
        != input_tokens
        || gauge.ledger().raw_input_tokens != input_tokens
        || gauge.ledger().fallback_tokens != input_tokens
        || gauge.ledger().model_output_tokens != model_output_tokens
        || gauge.ledger().model_calls != 1
    {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::TokenAccountingFailure,
            "raw Decision View token ledger did not conserve its locked byte-token coordinate",
        ));
    }
    Ok((
        digest_value(b"zerostack.raw_v2.token_ledger.v1\0", gauge.ledger())?,
        model_output_tokens,
    ))
}

fn charge_raw_baseline(
    assembly_abi_digest: DigestV1,
    raw_worker_protocol_digest: DigestV1,
    baseline_identity_digest: DigestV1,
    work_unit_id: DigestV1,
    baseline_frame_bytes: u64,
) -> Result<DigestV1, RawV2SliceErrorV1> {
    let identity = ParentCounterIdentityV1 {
        counter_id: "raw_v2_frame_bytes".into(),
        unit: CausalCounterUnitV1::Bytes,
        boundary_digest: raw_worker_protocol_digest,
        adapter_digest: baseline_identity_digest,
        platform_profile_digest: assembly_abi_digest,
    };
    let outcome = CausalWorkReceiptV1::build(
        assembly_abi_digest,
        ParentCounterObservationV1::Measured {
            window: ParentCounterWindowV1 {
                identity,
                start: 0,
                end: baseline_frame_bytes,
            },
        },
        vec![CausalWorkChargeV1 {
            work_unit_id,
            class: CausalWorkClassV1::Baseline,
            amount: baseline_frame_bytes,
        }],
        ResiduePolicyV1::RejectUnclassified,
    )
    .map_err(stage_error("causal_work"))?;
    let CausalWorkOutcomeV1::Measured { receipt } = outcome else {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::CausalWorkFailure,
            "measured raw frame bytes produced an unmeasured outcome",
        ));
    };
    receipt
        .validate()
        .map_err(stage_error("causal_work_validate"))?;
    Ok(receipt.receipt_digest)
}

fn decode_hex_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Vec<u8>, RawV2SliceErrorV1> {
    let value = object.get(field).and_then(Value::as_str).ok_or_else(|| {
        RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::WorkerProtocol,
            format!("reference worker {field} is missing or not a string"),
        )
    })?;
    decode_lower_hex(value)
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn decode_lower_hex(value: &str) -> Result<Vec<u8>, RawV2SliceErrorV1> {
    if !value.len().is_multiple_of(2)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::WorkerProtocol,
            "raw bytes must use even-length lowercase hexadecimal",
        ));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let nibble = |byte: u8| match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                _ => unreachable!(),
            };
            Ok((nibble(pair[0]) << 4) | nibble(pair[1]))
        })
        .collect()
}

fn digest_bytes(bytes: &[u8]) -> DigestV1 {
    DigestV1::from_bytes(sha256(bytes))
}

fn snapshot_manifest_bytes(file_digest: DigestV1) -> Vec<u8> {
    canonical_json(&json!({
        "files": [{
            "content_digest": file_digest,
            "path": "src/lib.rs",
        }],
        "schema_version": 1,
    }))
    .into_bytes()
}

fn snapshot_root(file_digest: DigestV1) -> DigestV1 {
    digest_bytes(&snapshot_manifest_bytes(file_digest))
}

fn domain_digest(domain: &[u8], bytes: &[u8]) -> DigestV1 {
    let mut value = Vec::with_capacity(domain.len() + 8 + bytes.len());
    value.extend_from_slice(domain);
    value.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    value.extend_from_slice(bytes);
    digest_bytes(&value)
}

fn digest_value<T: Serialize>(domain: &[u8], value: &T) -> Result<DigestV1, RawV2SliceErrorV1> {
    let value = serde_json::to_value(value).map_err(serialization_error)?;
    Ok(domain_digest(domain, canonical_json(&value).as_bytes()))
}

fn canonical_serialize<T: Serialize>(value: &T) -> Result<Vec<u8>, RawV2SliceErrorV1> {
    let value = serde_json::to_value(value).map_err(serialization_error)?;
    Ok(canonical_json(&value).into_bytes())
}

fn serialization_error(error: serde_json::Error) -> RawV2SliceErrorV1 {
    RawV2SliceErrorV1::new(
        RawV2SliceFailureCodeV1::SerializationFailure,
        error.to_string(),
    )
}

fn stage_error<E: fmt::Display>(stage: &'static str) -> impl FnOnce(E) -> RawV2SliceErrorV1 {
    move |error| {
        RawV2SliceErrorV1::new(
            RawV2SliceFailureCodeV1::StageFailure,
            format!("{stage}: {error}"),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RawV2SliceFailureCodeV1 {
    UnsupportedVersion,
    UnsupportedTask,
    InputBounds,
    InvalidIdentity,
    ContractBindingMismatch,
    StateBindingMismatch,
    InputDrift,
    TransactionFailure,
    WorkerProtocol,
    OutputMismatch,
    EvidenceFailure,
    TaskAcceptanceFailure,
    CausalWorkFailure,
    TokenAccountingFailure,
    WorkOverflow,
    IncompleteReceipt,
    ReceiptDigestMismatch,
    NonCanonicalEncoding,
    SerializationFailure,
    StageFailure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawV2SliceErrorV1 {
    pub code: RawV2SliceFailureCodeV1,
    pub detail: String,
}

impl RawV2SliceErrorV1 {
    fn new(code: RawV2SliceFailureCodeV1, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub const fn failure_code(&self) -> RawV2SliceFailureCodeV1 {
        self.code
    }
}

impl fmt::Display for RawV2SliceErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.detail)
    }
}
impl std::error::Error for RawV2SliceErrorV1 {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_v2_reference_slice_runs_uncompressed_end_to_end() {
        let input = reference_raw_v2_input_v1();
        let run = run_raw_v2_slice_v1(&input).unwrap();
        assert_eq!(run.output, b"pub fn raw_baseline() -> u8 { 43 }\n");
        assert_eq!(
            run.receipt.execution_mode(),
            RawV2ExecutionModeV1::UncompressedRawBaseline
        );
        assert_ne!(run.receipt.input_digest(), run.receipt.output_digest());
        assert_ne!(run.receipt.cwir_semantic_digest(), DigestV1::ZERO);
        assert_ne!(
            run.receipt.baseline_state_digest(),
            run.receipt.candidate_state_digest()
        );
        assert_ne!(run.receipt.effect_action_digest(), DigestV1::ZERO);
        assert_ne!(run.receipt.effect_acceptance_digest(), DigestV1::ZERO);
        assert_ne!(run.receipt.transaction_receipt_digest(), DigestV1::ZERO);
        assert_eq!(
            run.receipt.publication_scope(),
            RawV2PublicationScopeV1::JournalRootReferenceOnly
        );
        assert!(run.receipt.decision_view_tokens() > input.workspace_bytes.len() as u64);
        assert!(run.receipt.model_output_tokens() > run.output.len() as u64);
        assert!(run.receipt.baseline_frame_bytes() > input.workspace_bytes.len() as u64);
        let bytes = run.receipt.canonical_bytes().unwrap();
        assert_eq!(
            RawV2SliceReceiptV1::from_canonical_bytes(&bytes).unwrap(),
            run.receipt
        );
        let frozen = include_str!("../../../conformance/models/raw-v2-baseline-v1.json");
        assert_eq!(bytes, frozen.trim_end_matches('\n').as_bytes());
        let receipt_keys = serde_json::to_value(&run.receipt)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let manifest_fields = raw_v2_slice_contract_manifest_v1()["receipt_fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(manifest_fields, receipt_keys);
        assert_eq!(
            raw_v2_slice_contract_digest_v1().to_hex(),
            "cbb1d09fe7b1322b1147212b16270dd7a80be5661608070e56b3ffe4eab79434"
        );
    }

    #[test]
    fn raw_v2_receipt_tamper_and_noncanonical_bytes_fail_closed() {
        let run = run_raw_v2_slice_v1(&reference_raw_v2_input_v1()).unwrap();
        let mut value = serde_json::to_value(&run.receipt).unwrap();
        value["output_digest"] = value["input_digest"].clone();
        let tampered = canonical_json(&value).into_bytes();
        assert_eq!(
            RawV2SliceReceiptV1::from_canonical_bytes(&tampered)
                .unwrap_err()
                .failure_code(),
            RawV2SliceFailureCodeV1::OutputMismatch
        );
        let mut state_tamper = serde_json::to_value(&run.receipt).unwrap();
        state_tamper["baseline_state_digest"] = state_tamper["candidate_state_digest"].clone();
        assert_eq!(
            RawV2SliceReceiptV1::from_canonical_bytes(canonical_json(&state_tamper).as_bytes())
                .unwrap_err()
                .failure_code(),
            RawV2SliceFailureCodeV1::StateBindingMismatch
        );
        let mut whitespace = run.receipt.canonical_bytes().unwrap();
        whitespace.push(b'\n');
        assert_eq!(
            RawV2SliceReceiptV1::from_canonical_bytes(&whitespace)
                .unwrap_err()
                .failure_code(),
            RawV2SliceFailureCodeV1::NonCanonicalEncoding
        );
    }

    #[test]
    fn raw_v2_reference_slice_rejects_task_and_input_drift() {
        let mut input = reference_raw_v2_input_v1();
        input.task = "future_task".into();
        assert_eq!(
            run_raw_v2_slice_v1(&input).unwrap_err().failure_code(),
            RawV2SliceFailureCodeV1::UnsupportedTask
        );
        let mut input = reference_raw_v2_input_v1();
        input.workspace_bytes = b"pub fn other() -> u8 { 42 }\n".to_vec();
        assert_eq!(
            run_raw_v2_slice_v1(&input).unwrap_err().failure_code(),
            RawV2SliceFailureCodeV1::InputDrift
        );
        let mut input = reference_raw_v2_input_v1();
        input.workspace_bytes = vec![0; RAW_V2_SLICE_MAX_INPUT_BYTES_V1 + 1];
        assert_eq!(
            run_raw_v2_slice_v1(&input).unwrap_err().failure_code(),
            RawV2SliceFailureCodeV1::InputBounds
        );
    }
}
