//! ZeroBench-R integration harness.
//!
//! The harness freezes a paired same-model experiment before execution, keeps
//! all failed and incomplete runs, validates complete causal-work receipts, and
//! computes release gates with checked integer arithmetic. A benchmark report is
//! evidence, not runtime publication authority or an unlabeled savings claim.

use std::{collections::BTreeSet, error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zero_abi::{DigestV1, canonical_json};
use zero_cert::{CompletenessWitness, Query, VerifiedEvidence};
use zero_gate::{
    BaselineExecutionReceiptV1, QualityAdmissionV1, QualityEvidenceClassV1, QualitySelectionV1,
};
use zero_ledger::{
    CausalClassTotalsV1, CausalCounterUnitV1, CausalWorkReceiptV1, ParentCounterIdentityV1,
    causal_work_contract_digest_v1,
};

pub const ZERO_BENCH_R_CONTRACT_VERSION_V1: u16 = 1;
pub const ZERO_BENCH_R_RUN_SCHEMA_VERSION_V1: &str = "zerobench-r-run/v1";
pub const ZERO_BENCH_R_REPORT_SCHEMA_VERSION_V1: &str = "zerobench-r-report/v1";
pub const ZERO_BENCH_R_MAX_CASES_V1: usize = 65_536;
pub const ZERO_BENCH_R_MAX_COORDINATES_V1: usize = 32;
pub const ZERO_BENCH_R_MAX_CANONICAL_BYTES_V1: usize = 4 * 1_048_576;
pub const ZERO_BENCH_R_MAX_ID_BYTES_V1: usize = 256;
pub const ZERO_BENCH_R_PPM_SCALE_V1: u64 = 1_000_000;
pub const ZERO_BENCH_R_ALPHA_PPM_V1: u64 = 50_000;
pub const ZERO_BENCH_R_AMPLIFY_LCB_PPM_V1: i64 = 50_000;
pub const ZERO_BENCH_R_RELATIVE_GAIN_PPM_V1: u64 = 100_000;
pub const ZERO_BENCH_R_NOVEL_TOKEN_REDUCTION_PPM_V1: i64 = 500_000;
pub const ZERO_BENCH_R_TIME_REDUCTION_PPM_V1: i64 = 200_000;

pub const ZERO_BENCH_R_RUN_SCHEMA_SHA256_V1: &str =
    "7d204a731a4465b31591085f22523c8015887dfd65466bec6bd3f1edd8cb20e9";
pub const ZERO_BENCH_R_REPORT_SCHEMA_SHA256_V1: &str =
    "118b1ce492f697ccc5fe8208541cf7f53261bc8f393be5010e7a0f8f4838539b";

const CASE_DOMAIN_V1: &[u8] = b"zerostack.zerobench.case.v1\0";
const PIN_DOMAIN_V1: &[u8] = b"zerostack.zerobench.adapter_pin.v1\0";
const IDENTITY_DOMAIN_V1: &[u8] = b"zerostack.zerobench.comparison_identity.v1\0";
const REGISTRATION_DOMAIN_V1: &[u8] = b"zerostack.zerobench.registration.v1\0";
const RUN_ORDER_DOMAIN_V1: &[u8] = b"zerostack.zerobench.run_order.v1\0";
const STOPPING_RULE_DOMAIN_V1: &[u8] = b"zerostack.zerobench.stopping_rule.v1\0";
const EXCLUSION_POLICY_DOMAIN_V1: &[u8] = b"zerostack.zerobench.exclusion_policy.v1\0";
const RUN_DOMAIN_V1: &[u8] = b"zerostack.zerobench.run.v1\0";
const RUN_AUTHORITY_DOMAIN_V1: &[u8] = b"zerostack.zerobench.run_authority.v1\0";
const REPORT_DOMAIN_V1: &[u8] = b"zerostack.zerobench.report.v1\0";
// Shared with zero-gate quality admissions so preregistration binds one verifier.
const VERIFIER_DOMAIN_V1: &[u8] = b"zerostack.quality.verifier.v1\0";
const PROTECTED_OUTCOME_DOMAIN_V1: &[u8] = b"zerostack.zerobench.protected_outcome.v1\0";
const RESOURCE_IDENTITY_DOMAIN_V1: &[u8] = b"zerostack.zerobench.resource_identity.v1\0";
const CONTRACT_DOMAIN_V1: &[u8] = b"zerostack.zerobench.contract.v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZeroBenchArmV1 {
    Raw,
    Rtk,
    ExternalZtk,
    Headroom,
    Leanctx,
    ContextMode,
    Empryo,
    RaccRCandidate,
    RaccRStrict,
    RaccRAmplify,
}

impl ZeroBenchArmV1 {
    pub const ALL: [Self; 10] = [
        Self::Raw,
        Self::Rtk,
        Self::ExternalZtk,
        Self::Headroom,
        Self::Leanctx,
        Self::ContextMode,
        Self::Empryo,
        Self::RaccRCandidate,
        Self::RaccRStrict,
        Self::RaccRAmplify,
    ];

    pub const fn is_racc_guarded(self) -> bool {
        matches!(self, Self::RaccRStrict | Self::RaccRAmplify)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZeroBenchTaskStratumV1 {
    NamedSymbolLocalBug,
    FailingTestDiagnosis,
    StackTraceLocalization,
    CrossFileRefactor,
    DynamicDispatchConfiguration,
    GeneratedCodeSchemaMigration,
    BroadArchitectureQuestion,
    ExactRepositoryFact,
    LongBuildTestOutputDiagnosis,
    AmbiguousTaskRequiringEvidence,
    RepeatedPreparedTaskFamily,
    ResumedReasoningContinuation,
    AdversarialOmittedEvidence,
    TransactionRollbackFault,
    MultiRepositoryChange,
}

impl ZeroBenchTaskStratumV1 {
    pub const ALL: [Self; 15] = [
        Self::NamedSymbolLocalBug,
        Self::FailingTestDiagnosis,
        Self::StackTraceLocalization,
        Self::CrossFileRefactor,
        Self::DynamicDispatchConfiguration,
        Self::GeneratedCodeSchemaMigration,
        Self::BroadArchitectureQuestion,
        Self::ExactRepositoryFact,
        Self::LongBuildTestOutputDiagnosis,
        Self::AmbiguousTaskRequiringEvidence,
        Self::RepeatedPreparedTaskFamily,
        Self::ResumedReasoningContinuation,
        Self::AdversarialOmittedEvidence,
        Self::TransactionRollbackFault,
        Self::MultiRepositoryChange,
    ];
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZeroBenchIntentV1 {
    Smoke,
    Release,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplicationClassV1 {
    None,
    Reproducible,
    Independent,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OmissionReasonV1 {
    Legal,
    Technical,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatusV1 {
    Complete,
    Failed,
    Incomplete,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationEvidenceClassV1 {
    RawBaseline,
    ExactNeutral,
    PointwiseDominance,
    ScopedClassDominance,
    DistributionalOnly,
    Unidentified,
    UnguardedCandidate,
    ExternalSystem,
}

impl PublicationEvidenceClassV1 {
    const fn permits_strict_candidate(self) -> bool {
        matches!(
            self,
            Self::ExactNeutral | Self::PointwiseDominance | Self::ScopedClassDominance
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningStateStatusV1 {
    Exact,
    Scoped,
    Approximate,
    CleanRestart,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "availability", content = "value", rename_all = "snake_case")]
pub enum MeasuredOrUnavailableU64V1 {
    Measured(u64),
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseVerdictV1 {
    Pass,
    Fail,
    Incomplete,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterPinV1 {
    pub pinned_revision_digest: DigestV1,
    pub install_procedure_digest: DigestV1,
    pub configuration_digest: DigestV1,
    pub model_tool_integration_digest: DigestV1,
    pub indexing_policy_digest: DigestV1,
    pub command_surface_digest: DigestV1,
    pub cache_policy_digest: DigestV1,
    pub fallback_policy_digest: DigestV1,
    pub metrics_extractor_digest: DigestV1,
    pub license_status_digest: DigestV1,
    pub secondary_model_identity_digest: Option<DigestV1>,
}

impl AdapterPinV1 {
    pub fn validate(&self) -> Result<(), ZeroBenchErrorV1> {
        require_nonzero(
            "adapter pin",
            &[
                self.pinned_revision_digest,
                self.install_procedure_digest,
                self.configuration_digest,
                self.model_tool_integration_digest,
                self.indexing_policy_digest,
                self.command_surface_digest,
                self.cache_policy_digest,
                self.fallback_policy_digest,
                self.metrics_extractor_digest,
                self.license_status_digest,
            ],
        )?;
        if self.secondary_model_identity_digest == Some(DigestV1::ZERO) {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::ZeroDigest,
                "secondary model identity cannot be zero",
            ));
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<DigestV1, ZeroBenchErrorV1> {
        self.validate()?;
        digest_serializable(PIN_DOMAIN_V1, self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArmRegistrationV1 {
    Included {
        pin: Box<AdapterPinV1>,
    },
    Omitted {
        reason: OmissionReasonV1,
        evidence_digest: DigestV1,
    },
}

impl ArmRegistrationV1 {
    fn validate(&self) -> Result<(), ZeroBenchErrorV1> {
        match self {
            Self::Included { pin } => pin.validate(),
            Self::Omitted {
                evidence_digest, ..
            } if *evidence_digest != DigestV1::ZERO => Ok(()),
            Self::Omitted { .. } => Err(zero_bench_error(
                ZeroBenchFailureCodeV1::ZeroDigest,
                "arm omission needs legal or technical evidence",
            )),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredArmV1 {
    pub arm: ZeroBenchArmV1,
    pub registration: ArmRegistrationV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkComparisonIdentityV1 {
    pub model_identity_digest: DigestV1,
    pub backend_identity_digest: DigestV1,
    pub decoder_seed_policy_digest: DigestV1,
    pub reasoning_contract_digest: DigestV1,
    pub output_headroom_digest: DigestV1,
    pub tool_authority_sandbox_digest: DigestV1,
    pub repository_set_digest: DigestV1,
    pub protected_predicate_digest: DigestV1,
    pub verifier_identity_digest: DigestV1,
    pub hardware_network_class_digest: DigestV1,
    pub timeout_resource_limits_digest: DigestV1,
    pub fallback_rights_digest: DigestV1,
    pub setup_amortization_horizon_digest: DigestV1,
    pub assembly_manifest_digest: DigestV1,
}

impl BenchmarkComparisonIdentityV1 {
    pub fn validate(&self) -> Result<(), ZeroBenchErrorV1> {
        require_nonzero(
            "benchmark comparison identity",
            &[
                self.model_identity_digest,
                self.backend_identity_digest,
                self.decoder_seed_policy_digest,
                self.reasoning_contract_digest,
                self.output_headroom_digest,
                self.tool_authority_sandbox_digest,
                self.repository_set_digest,
                self.protected_predicate_digest,
                self.verifier_identity_digest,
                self.hardware_network_class_digest,
                self.timeout_resource_limits_digest,
                self.fallback_rights_digest,
                self.setup_amortization_horizon_digest,
                self.assembly_manifest_digest,
            ],
        )
    }

    pub fn digest(&self) -> Result<DigestV1, ZeroBenchErrorV1> {
        self.validate()?;
        digest_serializable(IDENTITY_DOMAIN_V1, self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkCaseV1 {
    pub task_identity_digest: DigestV1,
    pub repository_root_digest: DigestV1,
    pub stratum: ZeroBenchTaskStratumV1,
    pub trial_index: u32,
    pub seed: Option<u64>,
    pub protected_predicate_digest: DigestV1,
    pub verifier_identity_digest: DigestV1,
    pub case_digest: DigestV1,
}

impl BenchmarkCaseV1 {
    pub fn new(
        task_identity_digest: DigestV1,
        repository_root_digest: DigestV1,
        stratum: ZeroBenchTaskStratumV1,
        trial_index: u32,
        seed: Option<u64>,
        protected_predicate_digest: DigestV1,
        verifier_identity_digest: DigestV1,
    ) -> Result<Self, ZeroBenchErrorV1> {
        let mut case = Self {
            task_identity_digest,
            repository_root_digest,
            stratum,
            trial_index,
            seed,
            protected_predicate_digest,
            verifier_identity_digest,
            case_digest: DigestV1::ZERO,
        };
        case.case_digest = case.expected_digest()?;
        case.validate()?;
        Ok(case)
    }

    pub fn validate(&self) -> Result<(), ZeroBenchErrorV1> {
        require_nonzero(
            "benchmark case",
            &[
                self.task_identity_digest,
                self.repository_root_digest,
                self.protected_predicate_digest,
                self.verifier_identity_digest,
                self.case_digest,
            ],
        )?;
        if self.case_digest != self.expected_digest()? {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::DigestMismatch,
                "benchmark case digest mismatch",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<DigestV1, ZeroBenchErrorV1> {
        Ok(digest_value(
            CASE_DOMAIN_V1,
            &json!({
                "protected_predicate_digest": self.protected_predicate_digest,
                "repository_root_digest": self.repository_root_digest,
                "seed": self.seed,
                "stratum": self.stratum,
                "task_identity_digest": self.task_identity_digest,
                "trial_index": self.trial_index,
                "verifier_identity_digest": self.verifier_identity_digest,
            }),
        ))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredRunSlotV1 {
    pub execution_order_index: u64,
    pub arm: ZeroBenchArmV1,
    pub case_digest: DigestV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroBenchRegistrationV1 {
    pub contract_version: u16,
    pub intent: ZeroBenchIntentV1,
    pub benchmark_identity_digest: DigestV1,
    pub task_corpus_digest: DigestV1,
    pub primary_endpoint_digest: DigestV1,
    pub stopping_rule_digest: DigestV1,
    pub randomization_digest: DigestV1,
    pub arm_order_seed: u64,
    pub exclusion_policy_digest: DigestV1,
    pub contamination_audit_digest: DigestV1,
    pub comparison_identity: BenchmarkComparisonIdentityV1,
    pub protected_resource_coordinates: Vec<ParentCounterIdentityV1>,
    pub novel_causal_token_coordinate_digest: DigestV1,
    pub cases: Vec<BenchmarkCaseV1>,
    pub arms: Vec<RegisteredArmV1>,
    pub run_slots: Vec<RegisteredRunSlotV1>,
    pub replication_class: ReplicationClassV1,
    pub replication_identity_digest: Option<DigestV1>,
    pub alpha_ppm: u64,
    pub amplify_lcb_threshold_ppm: i64,
    pub relative_gain_threshold_ppm: u64,
    pub novel_token_reduction_threshold_ppm: i64,
    pub time_reduction_threshold_ppm: i64,
    pub registration_digest: DigestV1,
}

impl ZeroBenchRegistrationV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        intent: ZeroBenchIntentV1,
        benchmark_identity_digest: DigestV1,
        task_corpus_digest: DigestV1,
        primary_endpoint_digest: DigestV1,
        arm_order_seed: u64,
        contamination_audit_digest: DigestV1,
        comparison_identity: BenchmarkComparisonIdentityV1,
        mut protected_resource_coordinates: Vec<ParentCounterIdentityV1>,
        novel_causal_token_coordinate_digest: DigestV1,
        mut cases: Vec<BenchmarkCaseV1>,
        mut arms: Vec<RegisteredArmV1>,
        replication_class: ReplicationClassV1,
        replication_identity_digest: Option<DigestV1>,
    ) -> Result<Self, ZeroBenchErrorV1> {
        protected_resource_coordinates =
            sorted_resource_identities(protected_resource_coordinates)?;
        cases.sort_by_key(|case| case.case_digest);
        arms.sort_by_key(|arm| arm.arm);
        let run_slots = registered_run_slots(arm_order_seed, &cases, &arms)?;
        let stopping_rule_digest = digest_value(
            STOPPING_RULE_DOMAIN_V1,
            &json!({
                "mode": "fixed_registered_run_matrix",
                "run_count": run_slots.len(),
            }),
        );
        let randomization_digest = digest_value(
            RUN_ORDER_DOMAIN_V1,
            &json!({
                "algorithm": "sha256_case_seed_arm_sort_v1",
                "arm_order_seed": arm_order_seed,
            }),
        );
        let exclusion_policy_digest = digest_value(
            EXCLUSION_POLICY_DOMAIN_V1,
            &json!({"policy": "retain_all_registered_failed_incomplete_and_complete_runs"}),
        );
        let mut registration = Self {
            contract_version: ZERO_BENCH_R_CONTRACT_VERSION_V1,
            intent,
            benchmark_identity_digest,
            task_corpus_digest,
            primary_endpoint_digest,
            stopping_rule_digest,
            randomization_digest,
            arm_order_seed,
            exclusion_policy_digest,
            contamination_audit_digest,
            comparison_identity,
            protected_resource_coordinates,
            novel_causal_token_coordinate_digest,
            cases,
            arms,
            run_slots,
            replication_class,
            replication_identity_digest,
            alpha_ppm: ZERO_BENCH_R_ALPHA_PPM_V1,
            amplify_lcb_threshold_ppm: ZERO_BENCH_R_AMPLIFY_LCB_PPM_V1,
            relative_gain_threshold_ppm: ZERO_BENCH_R_RELATIVE_GAIN_PPM_V1,
            novel_token_reduction_threshold_ppm: ZERO_BENCH_R_NOVEL_TOKEN_REDUCTION_PPM_V1,
            time_reduction_threshold_ppm: ZERO_BENCH_R_TIME_REDUCTION_PPM_V1,
            registration_digest: DigestV1::ZERO,
        };
        registration.registration_digest = registration.expected_digest()?;
        registration.validate()?;
        Ok(registration)
    }

    pub fn validate(&self) -> Result<(), ZeroBenchErrorV1> {
        if self.contract_version != ZERO_BENCH_R_CONTRACT_VERSION_V1 {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::SchemaVersionMismatch,
                "ZeroBench registration contract version mismatch",
            ));
        }
        require_nonzero(
            "ZeroBench registration",
            &[
                self.benchmark_identity_digest,
                self.task_corpus_digest,
                self.primary_endpoint_digest,
                self.stopping_rule_digest,
                self.randomization_digest,
                self.exclusion_policy_digest,
                self.contamination_audit_digest,
                self.novel_causal_token_coordinate_digest,
                self.registration_digest,
            ],
        )?;
        self.comparison_identity.validate()?;
        if self.alpha_ppm != ZERO_BENCH_R_ALPHA_PPM_V1
            || self.amplify_lcb_threshold_ppm != ZERO_BENCH_R_AMPLIFY_LCB_PPM_V1
            || self.relative_gain_threshold_ppm != ZERO_BENCH_R_RELATIVE_GAIN_PPM_V1
            || self.novel_token_reduction_threshold_ppm != ZERO_BENCH_R_NOVEL_TOKEN_REDUCTION_PPM_V1
            || self.time_reduction_threshold_ppm != ZERO_BENCH_R_TIME_REDUCTION_PPM_V1
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::ThresholdMismatch,
                "release thresholds differ from the frozen ZeroBench-R v1 endpoint",
            ));
        }
        validate_resource_identities(
            &self.protected_resource_coordinates,
            self.novel_causal_token_coordinate_digest,
        )?;
        if self.intent == ZeroBenchIntentV1::Release
            && [
                CausalCounterUnitV1::Tokens,
                CausalCounterUnitV1::Bytes,
                CausalCounterUnitV1::Calls,
                CausalCounterUnitV1::CpuNanoseconds,
                CausalCounterUnitV1::WallNanoseconds,
                CausalCounterUnitV1::AllocatedBytes,
                CausalCounterUnitV1::IoBytes,
            ]
            .into_iter()
            .any(|unit| {
                !self
                    .protected_resource_coordinates
                    .iter()
                    .any(|identity| identity.unit == unit)
            })
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::IncompleteMeasuredWork,
                "release registration lacks a native token/byte/call/CPU/wall/allocation/I/O coordinate",
            ));
        }
        validate_cases(self.intent, &self.cases)?;
        if self.cases.iter().any(|case| {
            case.protected_predicate_digest != self.comparison_identity.protected_predicate_digest
                || case.verifier_identity_digest
                    != self.comparison_identity.verifier_identity_digest
        }) {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidRegistration,
                "case verifier/protected predicate differs from locked comparison identity",
            ));
        }
        validate_arms(&self.arms)?;
        let expected_slots = registered_run_slots(self.arm_order_seed, &self.cases, &self.arms)?;
        let expected_stopping = digest_value(
            STOPPING_RULE_DOMAIN_V1,
            &json!({
                "mode": "fixed_registered_run_matrix",
                "run_count": expected_slots.len(),
            }),
        );
        let expected_randomization = digest_value(
            RUN_ORDER_DOMAIN_V1,
            &json!({
                "algorithm": "sha256_case_seed_arm_sort_v1",
                "arm_order_seed": self.arm_order_seed,
            }),
        );
        let expected_exclusion = digest_value(
            EXCLUSION_POLICY_DOMAIN_V1,
            &json!({"policy": "retain_all_registered_failed_incomplete_and_complete_runs"}),
        );
        if self.run_slots != expected_slots
            || self.stopping_rule_digest != expected_stopping
            || self.randomization_digest != expected_randomization
            || self.exclusion_policy_digest != expected_exclusion
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidRegistration,
                "fixed stopping, arm randomization, exclusion, or run-slot plan was altered",
            ));
        }
        match (self.replication_class, self.replication_identity_digest) {
            (ReplicationClassV1::None, None) => {}
            (ReplicationClassV1::Reproducible | ReplicationClassV1::Independent, Some(digest))
                if digest != DigestV1::ZERO => {}
            _ => {
                return Err(zero_bench_error(
                    ZeroBenchFailureCodeV1::ReplicationMismatch,
                    "replication class and identity are inconsistent",
                ));
            }
        }
        if self.registration_digest != self.expected_digest()? {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::DigestMismatch,
                "ZeroBench registration digest mismatch",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ZeroBenchErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ZeroBenchErrorV1> {
        if bytes.len() > ZERO_BENCH_R_MAX_CANONICAL_BYTES_V1 {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::CanonicalPayloadTooLarge,
                "ZeroBench registration exceeds the canonical byte bound",
            ));
        }
        let registration: Self = serde_json::from_slice(bytes).map_err(json_error)?;
        registration.validate()?;
        if registration.canonical_bytes()? != bytes {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::NonCanonicalEncoding,
                "ZeroBench registration is not canonical sorted-key JSON",
            ));
        }
        Ok(registration)
    }

    fn expected_digest(&self) -> Result<DigestV1, ZeroBenchErrorV1> {
        Ok(digest_value(
            REGISTRATION_DOMAIN_V1,
            &json!({
                "alpha_ppm": self.alpha_ppm,
                "amplify_lcb_threshold_ppm": self.amplify_lcb_threshold_ppm,
                "arms": self.arms,
                "benchmark_identity_digest": self.benchmark_identity_digest,
                "cases": self.cases,
                "comparison_identity": self.comparison_identity,
                "contamination_audit_digest": self.contamination_audit_digest,
                "contract_version": self.contract_version,
                "exclusion_policy_digest": self.exclusion_policy_digest,
                "intent": self.intent,
                "novel_causal_token_coordinate_digest": self.novel_causal_token_coordinate_digest,
                "novel_token_reduction_threshold_ppm": self.novel_token_reduction_threshold_ppm,
                "primary_endpoint_digest": self.primary_endpoint_digest,
                "protected_resource_coordinates": self.protected_resource_coordinates,
                "randomization_digest": self.randomization_digest,
                "relative_gain_threshold_ppm": self.relative_gain_threshold_ppm,
                "replication_class": self.replication_class,
                "replication_identity_digest": self.replication_identity_digest,
                "stopping_rule_digest": self.stopping_rule_digest,
                "task_corpus_digest": self.task_corpus_digest,
                "time_reduction_threshold_ppm": self.time_reduction_threshold_ppm,
            }),
        ))
    }

    fn case(&self, digest: DigestV1) -> Option<&BenchmarkCaseV1> {
        self.cases
            .binary_search_by_key(&digest, |case| case.case_digest)
            .ok()
            .map(|index| &self.cases[index])
    }

    fn run_slot(&self, arm: ZeroBenchArmV1, case_digest: DigestV1) -> Option<&RegisteredRunSlotV1> {
        self.run_slots
            .iter()
            .find(|slot| slot.arm == arm && slot.case_digest == case_digest)
    }

    fn included_pin(&self, arm: ZeroBenchArmV1) -> Option<&AdapterPinV1> {
        self.arms
            .binary_search_by_key(&arm, |entry| entry.arm)
            .ok()
            .and_then(|index| match &self.arms[index].registration {
                ArmRegistrationV1::Included { pin } => Some(pin.as_ref()),
                ArmRegistrationV1::Omitted { .. } => None,
            })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningMetricsV1 {
    pub hidden_reasoning_tokens: MeasuredOrUnavailableU64V1,
    pub visible_output_tokens: u64,
    pub reasoning_turns_before_first_candidate: u32,
    pub semantic_reasoning_turns_before_first_candidate: u32,
    pub orchestration_turns_before_first_candidate: u32,
    pub incomplete_during_reasoning: bool,
    pub reasoning_state_status: ReasoningStateStatusV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CacheMetricsV1 {
    pub logical_input_tokens: u64,
    pub provider_uncached_input_tokens: u64,
    pub provider_reported_cached_input_tokens: Option<u64>,
    pub provider_cache_eligible: bool,
    pub byte_identical_prefix_eligible: bool,
    pub exact_local_artifact_reuse: u64,
    pub exact_reasoning_continuation_reuse: bool,
    pub context_occupancy_tokens: u64,
}

impl CacheMetricsV1 {
    fn validate(&self) -> Result<(), ZeroBenchErrorV1> {
        if self.provider_uncached_input_tokens > self.logical_input_tokens
            || self
                .provider_reported_cached_input_tokens
                .is_some_and(|cached| cached > self.logical_input_tokens)
            || self.context_occupancy_tokens < self.logical_input_tokens
            || (!self.provider_cache_eligible
                && self.provider_reported_cached_input_tokens.unwrap_or(0) != 0)
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidRunMetrics,
                "cache/input counters are arithmetically inconsistent",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentMetricsV1 {
    pub correct_locus_at_1: bool,
    pub exact_read_at_1: bool,
    pub verified_effect_at_1: bool,
    pub model_calls: u32,
    pub tool_calls: u32,
    pub time_to_first_relevant_byte_micros: u64,
    pub time_to_first_candidate_micros: u64,
    pub time_to_verified_effect_micros: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SystemMetricsV1 {
    pub indexing_preparation_micros: u64,
    pub incremental_update_micros: Option<u64>,
    pub peak_rss_bytes: u64,
    pub disk_footprint_bytes: u64,
    pub stale_artifact_count: u64,
    pub crash_recovery_correct: Option<bool>,
    pub suite_processes_at_rest: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeWorkAmountV1 {
    pub identity: ParentCounterIdentityV1,
    pub class_totals: CausalClassTotalsV1,
    pub observed_total: u64,
    pub receipt_digest: DigestV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroBenchRunClaimV1 {
    pub schema_version: String,
    pub registration_digest: DigestV1,
    pub comparison_identity_digest: DigestV1,
    pub model_identity_digest: DigestV1,
    pub reasoning_contract_digest: DigestV1,
    pub arm: ZeroBenchArmV1,
    pub adapter_pin_digest: DigestV1,
    pub secondary_model_identity_digest: Option<DigestV1>,
    pub case_digest: DigestV1,
    pub task_identity_digest: DigestV1,
    pub repository_root_digest: DigestV1,
    pub task_stratum: ZeroBenchTaskStratumV1,
    pub trial_index: u32,
    pub seed: Option<u64>,
    pub execution_order_index: u64,
    pub status: RunStatusV1,
    pub failure_or_incomplete_digest: Option<DigestV1>,
    pub candidate_success_before_guard: Option<bool>,
    pub published_success: Option<bool>,
    pub candidate_published: bool,
    pub publication_evidence_class: PublicationEvidenceClassV1,
    pub quality_admission_digest: Option<DigestV1>,
    pub baseline_paired_outcome_digest: Option<DigestV1>,
    pub candidate_protected_outcome_digest: Option<DigestV1>,
    pub deoptimization_execution_receipt_digest: Option<DigestV1>,
    pub fallback_used: bool,
    pub reasoning: ReasoningMetricsV1,
    pub cache: CacheMetricsV1,
    pub agent: AgentMetricsV1,
    pub system: SystemMetricsV1,
    pub measured_work: Vec<NativeWorkAmountV1>,
    pub novel_causal_tokens: u64,
    pub verifier_identity_digest: DigestV1,
}

impl ZeroBenchRunClaimV1 {
    pub fn validate(&self) -> Result<(), ZeroBenchErrorV1> {
        if self.schema_version != ZERO_BENCH_R_RUN_SCHEMA_VERSION_V1 {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::SchemaVersionMismatch,
                "ZeroBench run schema version mismatch",
            ));
        }
        require_nonzero(
            "ZeroBench run claim",
            &[
                self.registration_digest,
                self.comparison_identity_digest,
                self.model_identity_digest,
                self.reasoning_contract_digest,
                self.adapter_pin_digest,
                self.case_digest,
                self.task_identity_digest,
                self.repository_root_digest,
                self.verifier_identity_digest,
            ],
        )?;
        if self.failure_or_incomplete_digest == Some(DigestV1::ZERO) {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::ZeroDigest,
                "run failure/incomplete digest cannot be zero",
            ));
        }
        if self.secondary_model_identity_digest == Some(DigestV1::ZERO) {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::ZeroDigest,
                "run secondary model identity cannot be zero",
            ));
        }
        if [
            self.quality_admission_digest,
            self.baseline_paired_outcome_digest,
            self.candidate_protected_outcome_digest,
            self.deoptimization_execution_receipt_digest,
        ]
        .into_iter()
        .flatten()
        .any(|digest| digest == DigestV1::ZERO)
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::ZeroDigest,
                "run quality/deoptimization binding cannot be zero",
            ));
        }
        match self.status {
            RunStatusV1::Complete => {
                if self.failure_or_incomplete_digest.is_some() || self.published_success.is_none() {
                    return Err(zero_bench_error(
                        ZeroBenchFailureCodeV1::InvalidRunStatus,
                        "complete run has a failure marker or lacks a published outcome",
                    ));
                }
            }
            RunStatusV1::Failed | RunStatusV1::Incomplete => {
                if self.failure_or_incomplete_digest.is_none()
                    || self.published_success.is_some()
                    || self.candidate_published
                {
                    return Err(zero_bench_error(
                        ZeroBenchFailureCodeV1::InvalidRunStatus,
                        "failed/incomplete run must retain a failure digest and publish nothing",
                    ));
                }
            }
        }
        if self.status != RunStatusV1::Complete
            && self.arm.is_racc_guarded()
            && self.publication_evidence_class != PublicationEvidenceClassV1::Unidentified
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidPublicationEvidence,
                "failed/incomplete strict runs cannot claim quality authority",
            ));
        }
        if self.status == RunStatusV1::Complete
            && self.arm != ZeroBenchArmV1::Raw
            && self.candidate_success_before_guard.is_none()
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidRunStatus,
                "complete non-raw run must report the pre-guard candidate outcome",
            ));
        }
        if self.candidate_published && self.candidate_success_before_guard != self.published_success
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidPublicationEvidence,
                "published candidate outcome differs from its pre-guard outcome",
            ));
        }
        if self.arm == ZeroBenchArmV1::Raw {
            if self.candidate_success_before_guard.is_some()
                || self.candidate_published
                || self.publication_evidence_class != PublicationEvidenceClassV1::RawBaseline
            {
                return Err(zero_bench_error(
                    ZeroBenchFailureCodeV1::InvalidPublicationEvidence,
                    "raw arm cannot be labeled as a candidate publication",
                ));
            }
        } else if self.arm.is_racc_guarded()
            && self.candidate_published
            && !self.publication_evidence_class.permits_strict_candidate()
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidPublicationEvidence,
                "strict/amplify candidate publication needs individual exact, pointwise, or scoped quality authority",
            ));
        }
        let has_quality_bindings = self.quality_admission_digest.is_some()
            && self.baseline_paired_outcome_digest.is_some();
        if self.arm.is_racc_guarded() && self.status == RunStatusV1::Complete {
            if !has_quality_bindings
                || (self.candidate_published
                    && (self.fallback_used
                        || self.deoptimization_execution_receipt_digest.is_some()
                        || self.candidate_protected_outcome_digest.is_none()))
                || (!self.candidate_published
                    && (!self.fallback_used
                        || self.deoptimization_execution_receipt_digest.is_none()))
            {
                return Err(zero_bench_error(
                    ZeroBenchFailureCodeV1::InvalidPublicationEvidence,
                    "complete strict/amplify result lacks coherent quality and final-selection authority",
                ));
            }
        } else if has_quality_bindings
            || self.quality_admission_digest.is_some()
            || self.baseline_paired_outcome_digest.is_some()
            || self.candidate_protected_outcome_digest.is_some()
            || self.deoptimization_execution_receipt_digest.is_some()
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidPublicationEvidence,
                "quality/deoptimization authority is invalid outside complete strict/amplify runs",
            ));
        }
        self.cache.validate()?;
        if self.cache.exact_reasoning_continuation_reuse
            && self.reasoning.reasoning_state_status != ReasoningStateStatusV1::Exact
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidRunMetrics,
                "exact reasoning continuation reuse needs exact reasoning state",
            ));
        }
        if self.arm.is_racc_guarded()
            && self.candidate_published
            && self.reasoning.incomplete_during_reasoning
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidPublicationEvidence,
                "strict/amplify cannot publish an incomplete reasoning candidate",
            ));
        }
        if self
            .reasoning
            .semantic_reasoning_turns_before_first_candidate
            .checked_add(self.reasoning.orchestration_turns_before_first_candidate)
            != Some(self.reasoning.reasoning_turns_before_first_candidate)
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidRunMetrics,
                "semantic and orchestration turns must exactly classify pre-candidate reasoning",
            ));
        }
        if self.agent.time_to_first_relevant_byte_micros > self.agent.time_to_first_candidate_micros
            || self.agent.time_to_first_candidate_micros > self.agent.time_to_verified_effect_micros
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidRunMetrics,
                "agent milestone times are out of causal order",
            ));
        }
        if (self.agent.exact_read_at_1 && !self.agent.correct_locus_at_1)
            || (self.agent.verified_effect_at_1 && !self.agent.exact_read_at_1)
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidRunMetrics,
                "agent @1 metrics violate locus/read/effect implication order",
            ));
        }
        if self.task_stratum == ZeroBenchTaskStratumV1::TransactionRollbackFault
            && self.system.crash_recovery_correct.is_none()
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidRunMetrics,
                "transaction/rollback cases must measure crash-recovery correctness",
            ));
        }
        if self.arm.is_racc_guarded()
            && self.candidate_published
            && (self.system.stale_artifact_count != 0
                || self.system.crash_recovery_correct == Some(false))
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidPublicationEvidence,
                "strict/amplify cannot publish with stale artifacts or failed recovery",
            ));
        }
        if self.arm.is_racc_guarded()
            && self.status == RunStatusV1::Complete
            && !self
                .measured_work
                .iter()
                .any(|work| work.class_totals.verification > 0)
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::IncompleteMeasuredWork,
                "complete strict/amplify run does not charge quality verification work",
            ));
        }
        if self.fallback_used
            && (!self
                .measured_work
                .iter()
                .any(|work| work.class_totals.baseline > 0)
                || !self
                    .measured_work
                    .iter()
                    .any(|work| work.class_totals.fallback > 0))
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::IncompleteMeasuredWork,
                "fallback selection does not charge baseline and fallback work",
            ));
        }
        if self.measured_work.is_empty()
            || self.measured_work.len() > ZERO_BENCH_R_MAX_COORDINATES_V1
            || !strictly_sorted_work(&self.measured_work)
            || self
                .measured_work
                .iter()
                .any(|work| work.receipt_digest == DigestV1::ZERO)
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::IncompleteMeasuredWork,
                "run needs one uniquely sorted causal-work receipt per protected native coordinate",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ZeroBenchErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<DigestV1, ZeroBenchErrorV1> {
        self.validate()?;
        digest_serializable(RUN_DOMAIN_V1, self)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroBenchVerifiedRunRecordV1 {
    pub contract_version: u16,
    pub claim: ZeroBenchRunClaimV1,
    pub claim_digest: DigestV1,
    pub evidence_digest: DigestV1,
    pub authority_digest: DigestV1,
}

impl ZeroBenchVerifiedRunRecordV1 {
    pub fn validate(&self) -> Result<(), ZeroBenchErrorV1> {
        if self.contract_version != ZERO_BENCH_R_CONTRACT_VERSION_V1 {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::SchemaVersionMismatch,
                "verified ZeroBench run contract version mismatch",
            ));
        }
        self.claim.validate()?;
        if self.claim_digest != self.claim.digest()? {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::DigestMismatch,
                "verified run claim digest mismatch",
            ));
        }
        require_nonzero(
            "verified ZeroBench run",
            &[self.evidence_digest, self.authority_digest],
        )?;
        if self.authority_digest != run_authority_digest(self)? {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::DigestMismatch,
                "verified run authority digest mismatch",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ZeroBenchErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ZeroBenchErrorV1> {
        if bytes.len() > ZERO_BENCH_R_MAX_CANONICAL_BYTES_V1 {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::CanonicalPayloadTooLarge,
                "verified ZeroBench run exceeds the canonical byte bound",
            ));
        }
        let record: Self = serde_json::from_slice(bytes).map_err(json_error)?;
        record.validate()?;
        if record.canonical_bytes()? != bytes {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::NonCanonicalEncoding,
                "verified ZeroBench run is not canonical sorted-key JSON",
            ));
        }
        Ok(record)
    }
}

/// Opaque intake for one exact run result. Replay records cannot reconstruct it.
pub struct VerifiedZeroBenchRunV1 {
    record: ZeroBenchVerifiedRunRecordV1,
}

impl VerifiedZeroBenchRunV1 {
    pub fn record(&self) -> &ZeroBenchVerifiedRunRecordV1 {
        &self.record
    }
}

fn publication_class_from_quality(class: QualityEvidenceClassV1) -> PublicationEvidenceClassV1 {
    match class {
        QualityEvidenceClassV1::ExactNeutral => PublicationEvidenceClassV1::ExactNeutral,
        QualityEvidenceClassV1::PointwiseDominance => {
            PublicationEvidenceClassV1::PointwiseDominance
        }
        QualityEvidenceClassV1::ScopedClassDominance => {
            PublicationEvidenceClassV1::ScopedClassDominance
        }
        QualityEvidenceClassV1::Distributional => PublicationEvidenceClassV1::DistributionalOnly,
        QualityEvidenceClassV1::Unidentified => PublicationEvidenceClassV1::Unidentified,
    }
}

#[derive(Clone, Copy)]
struct GuardedQualityBindingsV1 {
    quality_admission_digest: Option<DigestV1>,
    baseline_paired_outcome_digest: Option<DigestV1>,
    candidate_protected_outcome_digest: Option<DigestV1>,
    deoptimization_execution_receipt_digest: Option<DigestV1>,
}

#[allow(clippy::too_many_arguments)]
fn guarded_quality_bindings(
    registration: &ZeroBenchRegistrationV1,
    arm: ZeroBenchArmV1,
    case: &BenchmarkCaseV1,
    pin: &AdapterPinV1,
    status: RunStatusV1,
    candidate_success_before_guard: Option<bool>,
    published_success: Option<bool>,
    candidate_published: bool,
    publication_evidence_class: PublicationEvidenceClassV1,
    fallback_used: bool,
    quality_admission: Option<&QualityAdmissionV1>,
    deoptimization_execution: Option<&BaselineExecutionReceiptV1>,
) -> Result<GuardedQualityBindingsV1, ZeroBenchErrorV1> {
    let empty = GuardedQualityBindingsV1 {
        quality_admission_digest: None,
        baseline_paired_outcome_digest: None,
        candidate_protected_outcome_digest: None,
        deoptimization_execution_receipt_digest: None,
    };
    if !arm.is_racc_guarded() || status != RunStatusV1::Complete {
        if quality_admission.is_some() || deoptimization_execution.is_some() {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidPublicationEvidence,
                "only complete strict/amplify runs may carry quality/deoptimization authority",
            ));
        }
        return Ok(empty);
    }
    let admission = quality_admission.ok_or_else(|| {
        zero_bench_error(
            ZeroBenchFailureCodeV1::InvalidPublicationEvidence,
            "complete strict/amplify run lacks opaque quality admission",
        )
    })?;
    admission.validate().map_err(|error| {
        zero_bench_error(
            ZeroBenchFailureCodeV1::InvalidPublicationEvidence,
            format!("quality admission is invalid: {error}"),
        )
    })?;
    let record = admission.record();
    let comparison_identity_digest = registration.comparison_identity.digest()?;
    let raw_baseline_identity_digest = registration
        .included_pin(ZeroBenchArmV1::Raw)
        .ok_or_else(|| {
            zero_bench_error(
                ZeroBenchFailureCodeV1::MissingRequiredArm,
                "strict quality binding lacks registered raw baseline",
            )
        })?
        .digest()?;
    if record.scope_digest != case.case_digest
        || record.comparison_identity_digest != comparison_identity_digest
        || record.raw_baseline_identity_digest != raw_baseline_identity_digest
        || record.candidate_identity_digest != Some(pin.digest()?)
        || record.protected_predicate_digest != case.protected_predicate_digest
        || record.verifier_identity_digest != case.verifier_identity_digest
        || publication_class_from_quality(record.evidence_class) != publication_evidence_class
    {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::RunBindingMismatch,
            "quality admission differs from preregistered task, arm, predicate, verifier, or identity",
        ));
    }
    let candidate_outcome =
        candidate_success_before_guard.map(zero_bench_protected_outcome_digest_v1);
    if record.candidate_outcome_digest.is_some()
        && record.candidate_outcome_digest != candidate_outcome
    {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::RunBindingMismatch,
            "quality admission candidate outcome differs from benchmark outcome",
        ));
    }
    let deoptimization_execution_receipt_digest = if candidate_published {
        if fallback_used
            || record.selection != QualitySelectionV1::Candidate
            || record.candidate_outcome_digest.is_some_and(|digest| {
                Some(digest) != published_success.map(zero_bench_protected_outcome_digest_v1)
            })
            || deoptimization_execution.is_some()
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidPublicationEvidence,
                "candidate publication conflicts with quality selection, outcome, or fallback",
            ));
        }
        None
    } else {
        let deoptimization = deoptimization_execution.ok_or_else(|| {
            zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidPublicationEvidence,
                "strict/amplify baseline selection lacks exact deoptimization execution receipt",
            )
        })?;
        deoptimization.validate().map_err(|error| {
            zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidPublicationEvidence,
                format!("deoptimization execution receipt is invalid: {error}"),
            )
        })?;
        if !fallback_used
            || record.selection != QualitySelectionV1::FrozenBaseline
            || published_success.map(zero_bench_protected_outcome_digest_v1)
                != Some(record.baseline_outcome_digest)
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidPublicationEvidence,
                "baseline publication conflicts with quality selection or protected outcome",
            ));
        }
        Some(deoptimization.receipt_digest())
    };
    Ok(GuardedQualityBindingsV1 {
        quality_admission_digest: Some(admission.digest()),
        baseline_paired_outcome_digest: Some(record.baseline_outcome_digest),
        candidate_protected_outcome_digest: record.candidate_outcome_digest,
        deoptimization_execution_receipt_digest,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn zero_bench_run_claim_v1(
    registration: &ZeroBenchRegistrationV1,
    arm: ZeroBenchArmV1,
    case_digest: DigestV1,
    status: RunStatusV1,
    failure_or_incomplete_digest: Option<DigestV1>,
    candidate_success_before_guard: Option<bool>,
    published_success: Option<bool>,
    candidate_published: bool,
    publication_evidence_class: PublicationEvidenceClassV1,
    fallback_used: bool,
    reasoning: ReasoningMetricsV1,
    cache: CacheMetricsV1,
    agent: AgentMetricsV1,
    system: SystemMetricsV1,
    quality_admission: Option<&QualityAdmissionV1>,
    deoptimization_execution: Option<&BaselineExecutionReceiptV1>,
    work_receipts: &[CausalWorkReceiptV1],
    verifier_identity_digest: DigestV1,
) -> Result<ZeroBenchRunClaimV1, ZeroBenchErrorV1> {
    registration.validate()?;
    let pin = registration.included_pin(arm).ok_or_else(|| {
        zero_bench_error(
            ZeroBenchFailureCodeV1::ArmUnavailable,
            "run arm is omitted or not registered",
        )
    })?;
    let case = registration.case(case_digest).ok_or_else(|| {
        zero_bench_error(
            ZeroBenchFailureCodeV1::UnknownCase,
            "run case is not preregistered",
        )
    })?;
    let run_slot = registration.run_slot(arm, case_digest).ok_or_else(|| {
        zero_bench_error(
            ZeroBenchFailureCodeV1::UnknownCase,
            "run arm/case pair lacks a preregistered execution slot",
        )
    })?;
    if verifier_identity_digest != case.verifier_identity_digest {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::VerifierIdentityMismatch,
            "run verifier identity differs from preregistered case verifier",
        ));
    }
    let quality_bindings = guarded_quality_bindings(
        registration,
        arm,
        case,
        pin,
        status,
        candidate_success_before_guard,
        published_success,
        candidate_published,
        publication_evidence_class,
        fallback_used,
        quality_admission,
        deoptimization_execution,
    )?;
    let measured_work = measured_work(
        registration,
        work_receipts,
        registration.comparison_identity.assembly_manifest_digest,
    )?;
    let novel_causal_tokens = measured_work
        .iter()
        .find(|work| {
            resource_identity_digest(&work.identity).ok()
                == Some(registration.novel_causal_token_coordinate_digest)
        })
        .map(|work| work.observed_total)
        .ok_or_else(|| {
            zero_bench_error(
                ZeroBenchFailureCodeV1::ResourceCoordinateMismatch,
                "novel causal token coordinate is absent from measured work",
            )
        })?;
    let claim = ZeroBenchRunClaimV1 {
        schema_version: ZERO_BENCH_R_RUN_SCHEMA_VERSION_V1.into(),
        registration_digest: registration.registration_digest,
        comparison_identity_digest: registration.comparison_identity.digest()?,
        model_identity_digest: registration.comparison_identity.model_identity_digest,
        reasoning_contract_digest: registration.comparison_identity.reasoning_contract_digest,
        arm,
        adapter_pin_digest: pin.digest()?,
        secondary_model_identity_digest: pin.secondary_model_identity_digest,
        case_digest,
        task_identity_digest: case.task_identity_digest,
        repository_root_digest: case.repository_root_digest,
        task_stratum: case.stratum,
        trial_index: case.trial_index,
        seed: case.seed,
        execution_order_index: run_slot.execution_order_index,
        status,
        failure_or_incomplete_digest,
        candidate_success_before_guard,
        published_success,
        candidate_published,
        publication_evidence_class,
        quality_admission_digest: quality_bindings.quality_admission_digest,
        baseline_paired_outcome_digest: quality_bindings.baseline_paired_outcome_digest,
        candidate_protected_outcome_digest: quality_bindings.candidate_protected_outcome_digest,
        deoptimization_execution_receipt_digest: quality_bindings
            .deoptimization_execution_receipt_digest,
        fallback_used,
        reasoning,
        cache,
        agent,
        system,
        measured_work,
        novel_causal_tokens,
        verifier_identity_digest,
    };
    claim.validate()?;
    Ok(claim)
}

pub fn verify_zero_bench_run_v1(
    registration: &ZeroBenchRegistrationV1,
    claim: ZeroBenchRunClaimV1,
    quality_admission: Option<&QualityAdmissionV1>,
    deoptimization_execution: Option<&BaselineExecutionReceiptV1>,
    work_receipts: &[CausalWorkReceiptV1],
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<VerifiedZeroBenchRunV1, ZeroBenchErrorV1> {
    registration.validate()?;
    claim.validate()?;
    let rebuilt = zero_bench_run_claim_v1(
        registration,
        claim.arm,
        claim.case_digest,
        claim.status,
        claim.failure_or_incomplete_digest,
        claim.candidate_success_before_guard,
        claim.published_success,
        claim.candidate_published,
        claim.publication_evidence_class,
        claim.fallback_used,
        claim.reasoning,
        claim.cache,
        claim.agent,
        claim.system,
        quality_admission,
        deoptimization_execution,
        work_receipts,
        claim.verifier_identity_digest,
    )?;
    if rebuilt != claim {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::RunBindingMismatch,
            "run claim does not match preregistration and causal-work receipts",
        ));
    }
    verify_exact_run_payload(&claim.canonical_bytes()?, claim.status, evidence)?;
    if verifier_identity(evidence) != claim.verifier_identity_digest {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::VerifierIdentityMismatch,
            "run verifier identity does not match the claim",
        ));
    }
    let mut record = ZeroBenchVerifiedRunRecordV1 {
        contract_version: ZERO_BENCH_R_CONTRACT_VERSION_V1,
        claim_digest: claim.digest()?,
        evidence_digest: verified_evidence_digest(evidence)?,
        claim,
        authority_digest: DigestV1::ZERO,
    };
    record.authority_digest = run_authority_digest(&record)?;
    record.validate()?;
    Ok(VerifiedZeroBenchRunV1 { record })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateNativeWorkAmountV1 {
    pub identity: ParentCounterIdentityV1,
    pub class_totals: CausalClassTotalsV1,
    pub observed_total: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateWorkV1 {
    pub coordinates: Vec<AggregateNativeWorkAmountV1>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateTelemetryV1 {
    pub hidden_reasoning_tokens_measured_total: u64,
    pub hidden_reasoning_unavailable_runs: u64,
    pub visible_output_tokens_total: u64,
    pub reasoning_turns_total: u64,
    pub semantic_reasoning_turns_total: u64,
    pub orchestration_turns_total: u64,
    pub logical_input_tokens_total: u64,
    pub provider_uncached_input_tokens_total: u64,
    pub provider_reported_cached_input_tokens_total: u64,
    pub provider_cache_telemetry_unavailable_runs: u64,
    pub provider_cache_eligible_runs: u64,
    pub byte_identical_prefix_eligible_runs: u64,
    pub exact_local_artifact_reuse_total: u64,
    pub exact_reasoning_continuation_reuse_runs: u64,
    pub context_occupancy_tokens_total: u64,
    pub model_calls_total: u64,
    pub tool_calls_total: u64,
    pub indexing_preparation_micros_total: u64,
    pub incremental_update_micros_total: u64,
    pub incremental_update_unavailable_runs: u64,
    pub peak_rss_bytes_max: u64,
    pub disk_footprint_bytes_max: u64,
    pub stale_artifact_count_total: u64,
    pub crash_recovery_correct_runs: u64,
    pub crash_recovery_failed_runs: u64,
    pub crash_recovery_unavailable_runs: u64,
    pub suite_processes_at_rest_max: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroBenchStratumReportV1 {
    pub stratum: ZeroBenchTaskStratumV1,
    pub complete_runs: u64,
    pub failed_runs: u64,
    pub incomplete_runs: u64,
    pub paired_complete_runs: u64,
    pub raw_successes: u64,
    pub published_successes: u64,
    pub rescues_n01: u64,
    pub regressions_n10: u64,
    pub conservative_hoeffding_lcb_ppm: Option<i64>,
    pub relative_gain_ppm: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroBenchRepositoryReportV1 {
    pub repository_root_digest: DigestV1,
    pub complete_runs: u64,
    pub failed_runs: u64,
    pub incomplete_runs: u64,
    pub paired_complete_runs: u64,
    pub raw_successes: u64,
    pub published_successes: u64,
    pub rescues_n01: u64,
    pub regressions_n10: u64,
    pub conservative_hoeffding_lcb_ppm: Option<i64>,
    pub relative_gain_ppm: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroBenchArmReportV1 {
    pub arm: ZeroBenchArmV1,
    pub omitted: bool,
    pub complete_runs: u64,
    pub failed_runs: u64,
    pub incomplete_runs: u64,
    pub paired_complete_runs: u64,
    pub raw_successes: u64,
    pub candidate_successes_before_guard: u64,
    pub published_successes: u64,
    pub rescues_n01: u64,
    pub regressions_n10: u64,
    pub fallback_runs: u64,
    pub conservative_hoeffding_lcb_ppm: Option<i64>,
    pub relative_gain_ppm: Option<u64>,
    pub lower_median_novel_token_reduction_ppm: Option<i64>,
    pub lower_median_time_reduction_ppm: Option<i64>,
    pub strata: Vec<ZeroBenchStratumReportV1>,
    pub repositories: Vec<ZeroBenchRepositoryReportV1>,
    pub aggregate_work: Option<AggregateWorkV1>,
    pub aggregate_telemetry: Option<AggregateTelemetryV1>,
    pub raw_paired_aggregate_work: Option<AggregateWorkV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroBenchReportRecordV1 {
    pub schema_version: String,
    pub contract_version: u16,
    pub registration_digest: DigestV1,
    pub arms: Vec<ZeroBenchArmReportV1>,
    pub strict_verdict: ReleaseVerdictV1,
    pub amplify_verdict: ReleaseVerdictV1,
    pub all_required_arms_included: bool,
    pub best_in_class_claim_authorized: bool,
    pub report_digest: DigestV1,
}

impl ZeroBenchReportRecordV1 {
    pub fn validate(&self) -> Result<(), ZeroBenchErrorV1> {
        if self.schema_version != ZERO_BENCH_R_REPORT_SCHEMA_VERSION_V1
            || self.contract_version != ZERO_BENCH_R_CONTRACT_VERSION_V1
            || self.registration_digest == DigestV1::ZERO
            || self.arms.len() != ZeroBenchArmV1::ALL.len()
            || !strictly_sorted_arm_reports(&self.arms)
            || self.best_in_class_claim_authorized
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidReport,
                "ZeroBench report version, arm set, or claim boundary is invalid",
            ));
        }
        for (expected, arm) in ZeroBenchArmV1::ALL.iter().zip(&self.arms) {
            if expected != &arm.arm {
                return Err(zero_bench_error(
                    ZeroBenchFailureCodeV1::InvalidReport,
                    "ZeroBench arm reports are missing or duplicated",
                ));
            }
            validate_arm_report_shape(arm)?;
        }
        if self.report_digest != self.expected_digest()? {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::DigestMismatch,
                "ZeroBench report digest mismatch",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ZeroBenchErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ZeroBenchErrorV1> {
        if bytes.len() > ZERO_BENCH_R_MAX_CANONICAL_BYTES_V1 {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::CanonicalPayloadTooLarge,
                "ZeroBench report exceeds the canonical byte bound",
            ));
        }
        let report: Self = serde_json::from_slice(bytes).map_err(json_error)?;
        report.validate()?;
        if report.canonical_bytes()? != bytes {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::NonCanonicalEncoding,
                "ZeroBench report is not canonical sorted-key JSON",
            ));
        }
        Ok(report)
    }

    fn expected_digest(&self) -> Result<DigestV1, ZeroBenchErrorV1> {
        Ok(digest_value(
            REPORT_DOMAIN_V1,
            &json!({
                "all_required_arms_included": self.all_required_arms_included,
                "amplify_verdict": self.amplify_verdict,
                "arms": self.arms,
                "best_in_class_claim_authorized": self.best_in_class_claim_authorized,
                "contract_version": self.contract_version,
                "registration_digest": self.registration_digest,
                "schema_version": self.schema_version,
                "strict_verdict": self.strict_verdict,
            }),
        ))
    }
}

/// Opaque compiled benchmark evidence. Its replay record does not mint claims.
pub struct CompiledZeroBenchReportV1 {
    record: ZeroBenchReportRecordV1,
}

impl CompiledZeroBenchReportV1 {
    pub fn record(&self) -> &ZeroBenchReportRecordV1 {
        &self.record
    }

    pub const fn strict_verdict(&self) -> ReleaseVerdictV1 {
        self.record.strict_verdict
    }

    pub const fn amplify_verdict(&self) -> ReleaseVerdictV1 {
        self.record.amplify_verdict
    }

    pub const fn permits_runtime_publication(&self) -> bool {
        false
    }

    pub const fn permits_best_in_class_claim(&self) -> bool {
        false
    }
}

pub fn compile_zero_bench_report_v1(
    registration: &ZeroBenchRegistrationV1,
    runs: Vec<VerifiedZeroBenchRunV1>,
) -> Result<CompiledZeroBenchReportV1, ZeroBenchErrorV1> {
    registration.validate()?;
    let records = complete_run_matrix(registration, runs)?;
    let raw_runs = records
        .iter()
        .filter(|record| record.claim.arm == ZeroBenchArmV1::Raw)
        .collect::<Vec<_>>();
    let all_required_arms_included = registration
        .arms
        .iter()
        .all(|entry| matches!(entry.registration, ArmRegistrationV1::Included { .. }));
    let mut arms = Vec::with_capacity(ZeroBenchArmV1::ALL.len());
    for arm in ZeroBenchArmV1::ALL {
        let omitted = registration.included_pin(arm).is_none();
        if omitted {
            arms.push(empty_arm_report(arm, true));
            continue;
        }
        let arm_runs = records
            .iter()
            .filter(|record| record.claim.arm == arm)
            .collect::<Vec<_>>();
        arms.push(compile_arm_report(registration, arm, &raw_runs, &arm_runs)?);
    }
    arms.sort_by_key(|report| report.arm);
    let strict = arms
        .iter()
        .find(|report| report.arm == ZeroBenchArmV1::RaccRStrict)
        .ok_or_else(|| {
            zero_bench_error(ZeroBenchFailureCodeV1::InvalidReport, "strict arm missing")
        })?;
    let amplify = arms
        .iter()
        .find(|report| report.arm == ZeroBenchArmV1::RaccRAmplify)
        .ok_or_else(|| {
            zero_bench_error(ZeroBenchFailureCodeV1::InvalidReport, "amplify arm missing")
        })?;
    let strict_verdict = strict_verdict(registration, strict);
    let amplify_verdict = amplify_verdict(registration, strict_verdict, amplify)?;
    let mut report = ZeroBenchReportRecordV1 {
        schema_version: ZERO_BENCH_R_REPORT_SCHEMA_VERSION_V1.into(),
        contract_version: ZERO_BENCH_R_CONTRACT_VERSION_V1,
        registration_digest: registration.registration_digest,
        arms,
        strict_verdict,
        amplify_verdict,
        all_required_arms_included,
        best_in_class_claim_authorized: false,
        report_digest: DigestV1::ZERO,
    };
    report.report_digest = report.expected_digest()?;
    report.validate()?;
    Ok(CompiledZeroBenchReportV1 { record: report })
}

pub fn zero_bench_r_contract_manifest_v1() -> Value {
    json!({
        "arms": ZeroBenchArmV1::ALL,
        "canonical_encoding": "sorted_key_json_no_whitespace",
        "candidate_arm_mapping": "formal_RACC_R_CANDIDATE_equals_uncontracted_ZeroStack_V3_design_note_arm",
        "confidence": "integer_ppm_conservative_hoeffding_ln20_upper_bound_3",
        "contract_version": ZERO_BENCH_R_CONTRACT_VERSION_V1,
        "failed_and_incomplete_runs": "exact_nonzero_exit_receipts_retained_and_release_incomplete",
        "run_order": "fixed_matrix_with_sha256_case_seed_arm_permutation",
        "system_metrics": [
            "indexing_preparation_micros", "incremental_update_micros", "peak_rss_bytes",
            "disk_footprint_bytes", "stale_artifact_count", "crash_recovery_correct",
            "suite_processes_at_rest",
        ],
        "stratification": "per_repository_and_per_formal_task_stratum_with_paired_counts_and_bounds",
        "linked_contracts": {"causal_work": causal_work_contract_digest_v1()},
        "negative_space": [
            "provider_eligibility_as_cache_hit",
            "token_savings_without_paired_quality",
            "unmeasured_work_as_zero",
            "omitted_failed_run",
            "floating_competitor_revision",
            "distributional_evidence_as_individual_publication",
            "benchmark_report_as_runtime_publication_authority",
            "automatic_best_in_class_claim",
        ],
        "published_report_schema_sha256": ZERO_BENCH_R_REPORT_SCHEMA_SHA256_V1,
        "published_run_schema_sha256": ZERO_BENCH_R_RUN_SCHEMA_SHA256_V1,
        "release_thresholds_ppm": {
            "alpha": ZERO_BENCH_R_ALPHA_PPM_V1,
            "amplify_lcb": ZERO_BENCH_R_AMPLIFY_LCB_PPM_V1,
            "novel_tokens_lower_median": ZERO_BENCH_R_NOVEL_TOKEN_REDUCTION_PPM_V1,
            "relative_gain": ZERO_BENCH_R_RELATIVE_GAIN_PPM_V1,
            "time_lower_median": ZERO_BENCH_R_TIME_REDUCTION_PPM_V1,
        },
        "required_task_strata": ZeroBenchTaskStratumV1::ALL,
        "release_native_units": [
            "tokens", "bytes", "calls", "cpu_nanoseconds", "wall_nanoseconds",
            "allocated_bytes", "io_bytes",
        ],
        "strict_publication": "opaque_quality_admission_plus_exact_deoptimization_receipt_for_fallback",
        "work_accounting": "native_coordinates_with_candidate_verification_comparison_baseline_fallback_restoration_prewarm_residue_classes",
    })
}

pub fn zero_bench_r_contract_digest_v1() -> DigestV1 {
    digest_value(CONTRACT_DOMAIN_V1, &zero_bench_r_contract_manifest_v1())
}

pub fn zero_bench_protected_outcome_digest_v1(success: bool) -> DigestV1 {
    digest_value(
        PROTECTED_OUTCOME_DOMAIN_V1,
        &json!({"protected_predicate_passed": success}),
    )
}

fn compile_arm_report(
    registration: &ZeroBenchRegistrationV1,
    arm: ZeroBenchArmV1,
    raw_runs: &[&ZeroBenchVerifiedRunRecordV1],
    arm_runs: &[&ZeroBenchVerifiedRunRecordV1],
) -> Result<ZeroBenchArmReportV1, ZeroBenchErrorV1> {
    let complete_runs = count_status(arm_runs, RunStatusV1::Complete);
    let failed_runs = count_status(arm_runs, RunStatusV1::Failed);
    let incomplete_runs = count_status(arm_runs, RunStatusV1::Incomplete);
    let fallback_runs = arm_runs
        .iter()
        .filter(|run| run.claim.fallback_used)
        .count() as u64;
    let candidate_successes_before_guard = arm_runs
        .iter()
        .filter(|run| run.claim.candidate_success_before_guard == Some(true))
        .count() as u64;
    let mut pairs = Vec::new();
    for arm_run in arm_runs {
        if arm_run.claim.status != RunStatusV1::Complete {
            continue;
        }
        if let Some(raw) = raw_runs.iter().find(|raw| {
            raw.claim.case_digest == arm_run.claim.case_digest
                && raw.claim.status == RunStatusV1::Complete
        }) {
            if arm.is_racc_guarded()
                && arm_run.claim.baseline_paired_outcome_digest
                    != raw
                        .claim
                        .published_success
                        .map(zero_bench_protected_outcome_digest_v1)
            {
                return Err(zero_bench_error(
                    ZeroBenchFailureCodeV1::RunBindingMismatch,
                    "strict/amplify quality admission binds a different paired raw outcome",
                ));
            }
            pairs.push((*raw, *arm_run));
        }
    }
    let raw_successes = pairs
        .iter()
        .filter(|(raw, _)| raw.claim.published_success == Some(true))
        .count() as u64;
    let published_successes = pairs
        .iter()
        .filter(|(_, candidate)| candidate.claim.published_success == Some(true))
        .count() as u64;
    let rescues_n01 = pairs
        .iter()
        .filter(|(raw, candidate)| {
            raw.claim.published_success == Some(false)
                && candidate.claim.published_success == Some(true)
        })
        .count() as u64;
    let regressions_n10 = pairs
        .iter()
        .filter(|(raw, candidate)| {
            raw.claim.published_success == Some(true)
                && candidate.claim.published_success == Some(false)
        })
        .count() as u64;
    let lcb = if pairs.is_empty() || regressions_n10 != 0 {
        None
    } else {
        Some(conservative_hoeffding_lcb_ppm(
            rescues_n01,
            pairs.len() as u64,
        )?)
    };
    let relative_gain = if raw_successes == 0 || regressions_n10 != 0 {
        None
    } else {
        Some(checked_ratio_ppm_u64(rescues_n01, raw_successes)?)
    };
    let token_reductions = pairs
        .iter()
        .filter_map(|(raw, candidate)| {
            paired_reduction_ppm(
                raw.claim.novel_causal_tokens,
                candidate.claim.novel_causal_tokens,
            )
            .transpose()
        })
        .collect::<Result<Vec<_>, ZeroBenchErrorV1>>()?;
    let time_reductions = pairs
        .iter()
        .filter_map(|(raw, candidate)| {
            paired_reduction_ppm(
                raw.claim.agent.time_to_verified_effect_micros,
                candidate.claim.agent.time_to_verified_effect_micros,
            )
            .transpose()
        })
        .collect::<Result<Vec<_>, ZeroBenchErrorV1>>()?;
    let candidate_aggregate_work = aggregate_work(registration, pairs.iter().map(|(_, run)| *run))?;
    let raw_paired_aggregate_work =
        aggregate_work(registration, pairs.iter().map(|(run, _)| *run))?;
    let mut strata = Vec::with_capacity(ZeroBenchTaskStratumV1::ALL.len());
    for stratum in ZeroBenchTaskStratumV1::ALL {
        let counts = grouped_counts(registration, arm_runs, &pairs, |case| {
            case.stratum == stratum
        })?;
        let (group_lcb, group_relative_gain) = grouped_statistics(counts)?;
        strata.push(ZeroBenchStratumReportV1 {
            stratum,
            complete_runs: counts.complete_runs,
            failed_runs: counts.failed_runs,
            incomplete_runs: counts.incomplete_runs,
            paired_complete_runs: counts.paired_complete_runs,
            raw_successes: counts.raw_successes,
            published_successes: counts.published_successes,
            rescues_n01: counts.rescues_n01,
            regressions_n10: counts.regressions_n10,
            conservative_hoeffding_lcb_ppm: group_lcb,
            relative_gain_ppm: group_relative_gain,
        });
    }
    let repository_roots = registration
        .cases
        .iter()
        .map(|case| case.repository_root_digest)
        .collect::<BTreeSet<_>>();
    let mut repositories = Vec::with_capacity(repository_roots.len());
    for repository_root_digest in repository_roots {
        let counts = grouped_counts(registration, arm_runs, &pairs, |case| {
            case.repository_root_digest == repository_root_digest
        })?;
        let (group_lcb, group_relative_gain) = grouped_statistics(counts)?;
        repositories.push(ZeroBenchRepositoryReportV1 {
            repository_root_digest,
            complete_runs: counts.complete_runs,
            failed_runs: counts.failed_runs,
            incomplete_runs: counts.incomplete_runs,
            paired_complete_runs: counts.paired_complete_runs,
            raw_successes: counts.raw_successes,
            published_successes: counts.published_successes,
            rescues_n01: counts.rescues_n01,
            regressions_n10: counts.regressions_n10,
            conservative_hoeffding_lcb_ppm: group_lcb,
            relative_gain_ppm: group_relative_gain,
        });
    }
    Ok(ZeroBenchArmReportV1 {
        arm,
        omitted: false,
        complete_runs,
        failed_runs,
        incomplete_runs,
        paired_complete_runs: pairs.len() as u64,
        raw_successes,
        candidate_successes_before_guard,
        published_successes,
        rescues_n01,
        regressions_n10,
        fallback_runs,
        conservative_hoeffding_lcb_ppm: lcb,
        relative_gain_ppm: relative_gain,
        lower_median_novel_token_reduction_ppm: lower_median(token_reductions),
        lower_median_time_reduction_ppm: lower_median(time_reductions),
        strata,
        repositories,
        aggregate_work: Some(candidate_aggregate_work),
        aggregate_telemetry: Some(aggregate_telemetry(arm_runs.iter().copied())?),
        raw_paired_aggregate_work: Some(raw_paired_aggregate_work),
    })
}

#[derive(Clone, Copy)]
struct GroupCountsV1 {
    complete_runs: u64,
    failed_runs: u64,
    incomplete_runs: u64,
    paired_complete_runs: u64,
    raw_successes: u64,
    published_successes: u64,
    rescues_n01: u64,
    regressions_n10: u64,
}

fn grouped_counts(
    registration: &ZeroBenchRegistrationV1,
    arm_runs: &[&ZeroBenchVerifiedRunRecordV1],
    pairs: &[(&ZeroBenchVerifiedRunRecordV1, &ZeroBenchVerifiedRunRecordV1)],
    predicate: impl Fn(&BenchmarkCaseV1) -> bool,
) -> Result<GroupCountsV1, ZeroBenchErrorV1> {
    let matches_case = |digest| {
        registration.case(digest).map(&predicate).ok_or_else(|| {
            zero_bench_error(
                ZeroBenchFailureCodeV1::UnknownCase,
                "grouped report references an unregistered case",
            )
        })
    };
    let mut counts = GroupCountsV1 {
        complete_runs: 0,
        failed_runs: 0,
        incomplete_runs: 0,
        paired_complete_runs: 0,
        raw_successes: 0,
        published_successes: 0,
        rescues_n01: 0,
        regressions_n10: 0,
    };
    for run in arm_runs {
        if !matches_case(run.claim.case_digest)? {
            continue;
        }
        match run.claim.status {
            RunStatusV1::Complete => counts.complete_runs += 1,
            RunStatusV1::Failed => counts.failed_runs += 1,
            RunStatusV1::Incomplete => counts.incomplete_runs += 1,
        }
    }
    for (raw, candidate) in pairs {
        if !matches_case(candidate.claim.case_digest)? {
            continue;
        }
        counts.paired_complete_runs += 1;
        let raw_success = raw.claim.published_success == Some(true);
        let candidate_success = candidate.claim.published_success == Some(true);
        counts.raw_successes += u64::from(raw_success);
        counts.published_successes += u64::from(candidate_success);
        counts.rescues_n01 += u64::from(!raw_success && candidate_success);
        counts.regressions_n10 += u64::from(raw_success && !candidate_success);
    }
    Ok(counts)
}

fn grouped_statistics(
    counts: GroupCountsV1,
) -> Result<(Option<i64>, Option<u64>), ZeroBenchErrorV1> {
    if counts.paired_complete_runs == 0 || counts.regressions_n10 != 0 {
        return Ok((None, None));
    }
    let lcb = Some(conservative_hoeffding_lcb_ppm(
        counts.rescues_n01,
        counts.paired_complete_runs,
    )?);
    let relative = if counts.raw_successes == 0 {
        None
    } else {
        Some(checked_ratio_ppm_u64(
            counts.rescues_n01,
            counts.raw_successes,
        )?)
    };
    Ok((lcb, relative))
}

fn strict_verdict(
    registration: &ZeroBenchRegistrationV1,
    report: &ZeroBenchArmReportV1,
) -> ReleaseVerdictV1 {
    if registration.intent != ZeroBenchIntentV1::Release
        || report.omitted
        || report.failed_runs != 0
        || report.incomplete_runs != 0
        || report.paired_complete_runs != registration.cases.len() as u64
    {
        ReleaseVerdictV1::Incomplete
    } else if report.regressions_n10 == 0 {
        ReleaseVerdictV1::Pass
    } else {
        ReleaseVerdictV1::Fail
    }
}

fn amplify_verdict(
    registration: &ZeroBenchRegistrationV1,
    strict: ReleaseVerdictV1,
    report: &ZeroBenchArmReportV1,
) -> Result<ReleaseVerdictV1, ZeroBenchErrorV1> {
    if strict != ReleaseVerdictV1::Pass
        || registration.intent != ZeroBenchIntentV1::Release
        || report.omitted
        || report.failed_runs != 0
        || report.incomplete_runs != 0
        || report.paired_complete_runs != registration.cases.len() as u64
        || registration.replication_class == ReplicationClassV1::None
    {
        return Ok(ReleaseVerdictV1::Incomplete);
    }
    let lcb_pass = report
        .conservative_hoeffding_lcb_ppm
        .is_some_and(|value| value >= registration.amplify_lcb_threshold_ppm);
    let relative_pass = report
        .relative_gain_ppm
        .is_none_or(|value| value >= registration.relative_gain_threshold_ppm);
    let token_pass = report
        .lower_median_novel_token_reduction_ppm
        .is_some_and(|value| value >= registration.novel_token_reduction_threshold_ppm);
    let time_pass = report
        .lower_median_time_reduction_ppm
        .is_some_and(|value| value >= registration.time_reduction_threshold_ppm);
    let work_pass = match (&report.aggregate_work, &report.raw_paired_aggregate_work) {
        (Some(candidate), Some(raw)) => no_resource_worse_and_one_better(candidate, raw)?,
        _ => false,
    };
    Ok(
        if report.regressions_n10 == 0
            && lcb_pass
            && relative_pass
            && token_pass
            && time_pass
            && work_pass
        {
            ReleaseVerdictV1::Pass
        } else {
            ReleaseVerdictV1::Fail
        },
    )
}

fn complete_run_matrix(
    registration: &ZeroBenchRegistrationV1,
    runs: Vec<VerifiedZeroBenchRunV1>,
) -> Result<Vec<ZeroBenchVerifiedRunRecordV1>, ZeroBenchErrorV1> {
    let mut records = runs.into_iter().map(|run| run.record).collect::<Vec<_>>();
    records.sort_by_key(|record| (record.claim.arm, record.claim.case_digest));
    for record in &records {
        record.validate()?;
        if record.claim.registration_digest != registration.registration_digest {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::RunBindingMismatch,
                "run binds another preregistration",
            ));
        }
    }
    let mut expected = Vec::new();
    for arm in &registration.arms {
        if matches!(arm.registration, ArmRegistrationV1::Included { .. }) {
            for case in &registration.cases {
                expected.push((arm.arm, case.case_digest));
            }
        }
    }
    expected.sort();
    let actual = records
        .iter()
        .map(|record| (record.claim.arm, record.claim.case_digest))
        .collect::<Vec<_>>();
    if actual != expected {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::IncompleteRunMatrix,
            "run matrix has a missing, duplicate, omitted-arm, or unregistered result",
        ));
    }
    Ok(records)
}

fn measured_work(
    registration: &ZeroBenchRegistrationV1,
    receipts: &[CausalWorkReceiptV1],
    assembly_manifest_digest: DigestV1,
) -> Result<Vec<NativeWorkAmountV1>, ZeroBenchErrorV1> {
    if receipts.len() != registration.protected_resource_coordinates.len() {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::IncompleteMeasuredWork,
            "every protected native coordinate needs one causal-work receipt",
        ));
    }
    let mut work = Vec::with_capacity(receipts.len());
    for receipt in receipts {
        receipt.validate().map_err(causal_error)?;
        if receipt.assembly_manifest_digest != assembly_manifest_digest {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::RunBindingMismatch,
                "causal-work receipt binds another assembly manifest",
            ));
        }
        work.push(NativeWorkAmountV1 {
            identity: receipt.measurement.identity.clone(),
            class_totals: receipt.class_totals.clone(),
            observed_total: receipt.observed_total,
            receipt_digest: receipt.receipt_digest,
        });
    }
    let mut keyed_work = work
        .into_iter()
        .map(|entry| Ok((resource_identity_digest(&entry.identity)?, entry)))
        .collect::<Result<Vec<_>, ZeroBenchErrorV1>>()?;
    keyed_work.sort_by_key(|(digest, _)| *digest);
    let work = keyed_work
        .into_iter()
        .map(|(_, entry)| entry)
        .collect::<Vec<_>>();
    if work
        .iter()
        .zip(&registration.protected_resource_coordinates)
        .any(|(entry, expected)| entry.identity != *expected)
    {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::ResourceCoordinateMismatch,
            "causal-work receipts differ from preregistered native coordinates",
        ));
    }
    Ok(work)
}

fn aggregate_work<'a>(
    registration: &ZeroBenchRegistrationV1,
    runs: impl Iterator<Item = &'a ZeroBenchVerifiedRunRecordV1>,
) -> Result<AggregateWorkV1, ZeroBenchErrorV1> {
    let mut totals = registration
        .protected_resource_coordinates
        .iter()
        .cloned()
        .map(|identity| AggregateNativeWorkAmountV1 {
            identity,
            class_totals: CausalClassTotalsV1::default(),
            observed_total: 0,
        })
        .collect::<Vec<_>>();
    for run in runs {
        for (total, observed) in totals.iter_mut().zip(&run.claim.measured_work) {
            if total.identity != observed.identity {
                return Err(zero_bench_error(
                    ZeroBenchFailureCodeV1::ResourceCoordinateMismatch,
                    "run resource coordinate order differs during aggregation",
                ));
            }
            total.class_totals =
                checked_add_class_totals(&total.class_totals, &observed.class_totals)?;
            total.observed_total = total
                .observed_total
                .checked_add(observed.observed_total)
                .ok_or_else(|| {
                    zero_bench_error(
                        ZeroBenchFailureCodeV1::ArithmeticOverflow,
                        "aggregate native work overflowed",
                    )
                })?;
        }
    }
    Ok(AggregateWorkV1 {
        coordinates: totals,
    })
}

fn aggregate_telemetry<'a>(
    runs: impl Iterator<Item = &'a ZeroBenchVerifiedRunRecordV1>,
) -> Result<AggregateTelemetryV1, ZeroBenchErrorV1> {
    let mut total = AggregateTelemetryV1::default();
    let add = |target: &mut u64, value: u64| -> Result<(), ZeroBenchErrorV1> {
        *target = target.checked_add(value).ok_or_else(|| {
            zero_bench_error(
                ZeroBenchFailureCodeV1::ArithmeticOverflow,
                "aggregate benchmark telemetry overflowed",
            )
        })?;
        Ok(())
    };
    for run in runs {
        match run.claim.reasoning.hidden_reasoning_tokens {
            MeasuredOrUnavailableU64V1::Measured(value) => {
                add(&mut total.hidden_reasoning_tokens_measured_total, value)?;
            }
            MeasuredOrUnavailableU64V1::Unavailable => {
                add(&mut total.hidden_reasoning_unavailable_runs, 1)?;
            }
        }
        add(
            &mut total.visible_output_tokens_total,
            run.claim.reasoning.visible_output_tokens,
        )?;
        add(
            &mut total.reasoning_turns_total,
            u64::from(run.claim.reasoning.reasoning_turns_before_first_candidate),
        )?;
        add(
            &mut total.semantic_reasoning_turns_total,
            u64::from(
                run.claim
                    .reasoning
                    .semantic_reasoning_turns_before_first_candidate,
            ),
        )?;
        add(
            &mut total.orchestration_turns_total,
            u64::from(
                run.claim
                    .reasoning
                    .orchestration_turns_before_first_candidate,
            ),
        )?;
        add(
            &mut total.logical_input_tokens_total,
            run.claim.cache.logical_input_tokens,
        )?;
        add(
            &mut total.provider_uncached_input_tokens_total,
            run.claim.cache.provider_uncached_input_tokens,
        )?;
        if let Some(value) = run.claim.cache.provider_reported_cached_input_tokens {
            add(
                &mut total.provider_reported_cached_input_tokens_total,
                value,
            )?;
        } else {
            add(&mut total.provider_cache_telemetry_unavailable_runs, 1)?;
        }
        add(
            &mut total.provider_cache_eligible_runs,
            u64::from(run.claim.cache.provider_cache_eligible),
        )?;
        add(
            &mut total.byte_identical_prefix_eligible_runs,
            u64::from(run.claim.cache.byte_identical_prefix_eligible),
        )?;
        add(
            &mut total.exact_local_artifact_reuse_total,
            run.claim.cache.exact_local_artifact_reuse,
        )?;
        add(
            &mut total.exact_reasoning_continuation_reuse_runs,
            u64::from(run.claim.cache.exact_reasoning_continuation_reuse),
        )?;
        add(
            &mut total.context_occupancy_tokens_total,
            run.claim.cache.context_occupancy_tokens,
        )?;
        add(
            &mut total.model_calls_total,
            u64::from(run.claim.agent.model_calls),
        )?;
        add(
            &mut total.tool_calls_total,
            u64::from(run.claim.agent.tool_calls),
        )?;
        add(
            &mut total.indexing_preparation_micros_total,
            run.claim.system.indexing_preparation_micros,
        )?;
        if let Some(value) = run.claim.system.incremental_update_micros {
            add(&mut total.incremental_update_micros_total, value)?;
        } else {
            add(&mut total.incremental_update_unavailable_runs, 1)?;
        }
        total.peak_rss_bytes_max = total
            .peak_rss_bytes_max
            .max(run.claim.system.peak_rss_bytes);
        total.disk_footprint_bytes_max = total
            .disk_footprint_bytes_max
            .max(run.claim.system.disk_footprint_bytes);
        add(
            &mut total.stale_artifact_count_total,
            run.claim.system.stale_artifact_count,
        )?;
        match run.claim.system.crash_recovery_correct {
            Some(true) => add(&mut total.crash_recovery_correct_runs, 1)?,
            Some(false) => add(&mut total.crash_recovery_failed_runs, 1)?,
            None => add(&mut total.crash_recovery_unavailable_runs, 1)?,
        }
        total.suite_processes_at_rest_max = total
            .suite_processes_at_rest_max
            .max(run.claim.system.suite_processes_at_rest);
    }
    Ok(total)
}

fn checked_add_class_totals(
    left: &CausalClassTotalsV1,
    right: &CausalClassTotalsV1,
) -> Result<CausalClassTotalsV1, ZeroBenchErrorV1> {
    let add = |left: u64, right: u64| {
        left.checked_add(right).ok_or_else(|| {
            zero_bench_error(
                ZeroBenchFailureCodeV1::ArithmeticOverflow,
                "aggregate causal-work class overflowed",
            )
        })
    };
    Ok(CausalClassTotalsV1 {
        candidate: add(left.candidate, right.candidate)?,
        verification: add(left.verification, right.verification)?,
        comparison: add(left.comparison, right.comparison)?,
        baseline: add(left.baseline, right.baseline)?,
        fallback: add(left.fallback, right.fallback)?,
        restoration: add(left.restoration, right.restoration)?,
        prewarm: add(left.prewarm, right.prewarm)?,
        residue: add(left.residue, right.residue)?,
    })
}

fn no_resource_worse_and_one_better(
    candidate: &AggregateWorkV1,
    raw: &AggregateWorkV1,
) -> Result<bool, ZeroBenchErrorV1> {
    if candidate.coordinates.len() != raw.coordinates.len() || candidate.coordinates.is_empty() {
        return Ok(false);
    }
    let mut better = false;
    for (candidate, raw) in candidate.coordinates.iter().zip(&raw.coordinates) {
        if candidate.identity != raw.identity || candidate.observed_total > raw.observed_total {
            return Ok(false);
        }
        better |= candidate.observed_total < raw.observed_total;
    }
    Ok(better)
}

fn conservative_hoeffding_lcb_ppm(rescues: u64, pairs: u64) -> Result<i64, ZeroBenchErrorV1> {
    if pairs == 0 || rescues > pairs {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::InvalidStatistics,
            "Hoeffding input counts are invalid",
        ));
    }
    let mean = checked_ratio_ppm_u64(rescues, pairs)?;
    let numerator = 3_u128
        .checked_mul(u128::from(ZERO_BENCH_R_PPM_SCALE_V1))
        .and_then(|value| value.checked_mul(u128::from(ZERO_BENCH_R_PPM_SCALE_V1)))
        .ok_or_else(arithmetic_error)?;
    let denominator = 2_u128
        .checked_mul(u128::from(pairs))
        .ok_or_else(arithmetic_error)?;
    let squared_penalty = ceil_div_u128(numerator, denominator)?;
    let penalty = ceil_sqrt_u128(squared_penalty)?;
    let mean = i64::try_from(mean).map_err(|_| arithmetic_error())?;
    let penalty = i64::try_from(penalty).map_err(|_| arithmetic_error())?;
    Ok(mean.saturating_sub(penalty))
}

fn checked_ratio_ppm_u64(numerator: u64, denominator: u64) -> Result<u64, ZeroBenchErrorV1> {
    if denominator == 0 {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::InvalidStatistics,
            "ratio denominator is zero",
        ));
    }
    let value = u128::from(numerator)
        .checked_mul(u128::from(ZERO_BENCH_R_PPM_SCALE_V1))
        .ok_or_else(arithmetic_error)?
        / u128::from(denominator);
    u64::try_from(value).map_err(|_| arithmetic_error())
}

fn paired_reduction_ppm(raw: u64, candidate: u64) -> Result<Option<i64>, ZeroBenchErrorV1> {
    if raw == 0 {
        return Ok(None);
    }
    let delta = i128::from(raw) - i128::from(candidate);
    let scaled = delta
        .checked_mul(i128::from(ZERO_BENCH_R_PPM_SCALE_V1))
        .ok_or_else(arithmetic_error)?
        / i128::from(raw);
    Ok(Some(i64::try_from(scaled).map_err(|_| arithmetic_error())?))
}

fn lower_median(mut values: Vec<i64>) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(values[(values.len() - 1) / 2])
}

fn ceil_div_u128(numerator: u128, denominator: u128) -> Result<u128, ZeroBenchErrorV1> {
    numerator
        .checked_add(denominator.checked_sub(1).ok_or_else(arithmetic_error)?)
        .ok_or_else(arithmetic_error)
        .map(|value| value / denominator)
}

fn ceil_sqrt_u128(value: u128) -> Result<u128, ZeroBenchErrorV1> {
    if value <= 1 {
        return Ok(value);
    }
    let mut low = 1_u128;
    let mut high = value;
    while low < high {
        let mid = low + (high - low) / 2;
        if mid >= ceil_div_u128(value, mid)? {
            high = mid;
        } else {
            low = mid.checked_add(1).ok_or_else(arithmetic_error)?;
        }
    }
    Ok(low)
}

fn sorted_resource_identities(
    identities: Vec<ParentCounterIdentityV1>,
) -> Result<Vec<ParentCounterIdentityV1>, ZeroBenchErrorV1> {
    let mut keyed = identities
        .into_iter()
        .map(|identity| Ok((resource_identity_digest(&identity)?, identity)))
        .collect::<Result<Vec<_>, ZeroBenchErrorV1>>()?;
    keyed.sort_by_key(|(digest, _)| *digest);
    Ok(keyed.into_iter().map(|(_, identity)| identity).collect())
}

fn validate_resource_identities(
    identities: &[ParentCounterIdentityV1],
    novel_token_digest: DigestV1,
) -> Result<(), ZeroBenchErrorV1> {
    if identities.is_empty() || identities.len() > ZERO_BENCH_R_MAX_COORDINATES_V1 {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::InvalidRegistration,
            "protected resource coordinate set must contain 1..=32 identities",
        ));
    }
    let mut previous = None;
    let mut contains_novel_tokens = false;
    for identity in identities {
        validate_counter_identity(identity)?;
        let digest = resource_identity_digest(identity)?;
        if previous.is_some_and(|prior| prior >= digest) {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidRegistration,
                "resource identities must be uniquely sorted",
            ));
        }
        contains_novel_tokens |=
            digest == novel_token_digest && identity.unit == CausalCounterUnitV1::Tokens;
        previous = Some(digest);
    }
    if !contains_novel_tokens {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::ResourceCoordinateMismatch,
            "novel causal token coordinate is absent or is not a token counter",
        ));
    }
    Ok(())
}

fn registered_run_slots(
    arm_order_seed: u64,
    cases: &[BenchmarkCaseV1],
    arms: &[RegisteredArmV1],
) -> Result<Vec<RegisteredRunSlotV1>, ZeroBenchErrorV1> {
    let mut slots = Vec::new();
    for case in cases {
        let mut randomized_arms = arms
            .iter()
            .filter(|entry| matches!(entry.registration, ArmRegistrationV1::Included { .. }))
            .map(|entry| {
                (
                    digest_value(
                        RUN_ORDER_DOMAIN_V1,
                        &json!({
                            "arm": entry.arm,
                            "arm_order_seed": arm_order_seed,
                            "case_digest": case.case_digest,
                        }),
                    ),
                    entry.arm,
                )
            })
            .collect::<Vec<_>>();
        randomized_arms.sort();
        for (_, arm) in randomized_arms {
            let execution_order_index = u64::try_from(slots.len()).map_err(|_| {
                zero_bench_error(
                    ZeroBenchFailureCodeV1::ArithmeticOverflow,
                    "registered run order exceeds u64",
                )
            })?;
            slots.push(RegisteredRunSlotV1 {
                execution_order_index,
                arm,
                case_digest: case.case_digest,
            });
        }
    }
    Ok(slots)
}

fn validate_cases(
    intent: ZeroBenchIntentV1,
    cases: &[BenchmarkCaseV1],
) -> Result<(), ZeroBenchErrorV1> {
    if cases.is_empty() || cases.len() > ZERO_BENCH_R_MAX_CASES_V1 {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::InvalidRegistration,
            "benchmark case set is empty or exceeds its bound",
        ));
    }
    let mut previous = None;
    let mut strata = BTreeSet::new();
    for case in cases {
        case.validate()?;
        if previous.is_some_and(|prior| prior >= case.case_digest) {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidRegistration,
                "benchmark cases must be uniquely sorted",
            ));
        }
        strata.insert(case.stratum);
        previous = Some(case.case_digest);
    }
    if intent == ZeroBenchIntentV1::Release {
        if ZeroBenchTaskStratumV1::ALL
            .iter()
            .any(|stratum| !strata.contains(stratum))
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::MissingTaskStratum,
                "release preregistration does not cover all required task strata",
            ));
        }
        let tasks = cases
            .iter()
            .map(|case| case.task_identity_digest)
            .collect::<BTreeSet<_>>();
        for task in tasks {
            let task_cases = cases
                .iter()
                .filter(|case| case.task_identity_digest == task)
                .collect::<Vec<_>>();
            let mut trial_indices = task_cases
                .iter()
                .map(|case| case.trial_index)
                .collect::<Vec<_>>();
            let seeds = task_cases
                .iter()
                .map(|case| case.seed)
                .collect::<Option<BTreeSet<_>>>();
            trial_indices.sort_unstable();
            let contiguous_trials = trial_indices
                .iter()
                .enumerate()
                .all(|(index, trial)| *trial == index as u32);
            if task_cases.len() < 2
                || !contiguous_trials
                || seeds
                    .as_ref()
                    .is_none_or(|values| values.len() != task_cases.len())
            {
                return Err(zero_bench_error(
                    ZeroBenchFailureCodeV1::InvalidRegistration,
                    "release tasks need two or more contiguous trials with distinct explicit seeds",
                ));
            }
        }
    }
    Ok(())
}

fn validate_arms(arms: &[RegisteredArmV1]) -> Result<(), ZeroBenchErrorV1> {
    if arms.len() != ZeroBenchArmV1::ALL.len() {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::MissingRequiredArm,
            "registration needs every required arm as included or evidenced omission",
        ));
    }
    for (expected, actual) in ZeroBenchArmV1::ALL.iter().zip(arms) {
        if expected != &actual.arm {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::MissingRequiredArm,
                "required arms are duplicated, missing, or out of canonical order",
            ));
        }
        actual.registration.validate()?;
        if matches!(
            actual.arm,
            ZeroBenchArmV1::Raw
                | ZeroBenchArmV1::RaccRCandidate
                | ZeroBenchArmV1::RaccRStrict
                | ZeroBenchArmV1::RaccRAmplify
        ) && matches!(
            &actual.registration,
            ArmRegistrationV1::Included { pin }
                if pin.secondary_model_identity_digest.is_some()
        ) {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidRegistration,
                "raw and RACC-R same-model arms cannot hide a secondary semantic model",
            ));
        }
    }
    if arms.iter().any(|entry| {
        matches!(
            entry.arm,
            ZeroBenchArmV1::Raw | ZeroBenchArmV1::RaccRStrict | ZeroBenchArmV1::RaccRAmplify
        ) && matches!(entry.registration, ArmRegistrationV1::Omitted { .. })
    }) {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::MissingRequiredArm,
            "raw, RACC-R strict, and RACC-R amplify cannot be omitted from this integration harness",
        ));
    }
    Ok(())
}

fn validate_counter_identity(identity: &ParentCounterIdentityV1) -> Result<(), ZeroBenchErrorV1> {
    if identity.counter_id.is_empty()
        || identity.counter_id.len() > ZERO_BENCH_R_MAX_ID_BYTES_V1
        || identity.boundary_digest == DigestV1::ZERO
        || identity.adapter_digest == DigestV1::ZERO
        || identity.platform_profile_digest == DigestV1::ZERO
    {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::ResourceCoordinateMismatch,
            "native counter identity is empty, oversized, or incomplete",
        ));
    }
    Ok(())
}

fn resource_identity_digest(
    identity: &ParentCounterIdentityV1,
) -> Result<DigestV1, ZeroBenchErrorV1> {
    validate_counter_identity(identity)?;
    digest_serializable(RESOURCE_IDENTITY_DOMAIN_V1, identity)
}

fn strictly_sorted_work(work: &[NativeWorkAmountV1]) -> bool {
    work.windows(2).all(|window| {
        match (
            resource_identity_digest(&window[0].identity),
            resource_identity_digest(&window[1].identity),
        ) {
            (Ok(left), Ok(right)) => left < right,
            _ => false,
        }
    }) && work
        .iter()
        .all(|entry| validate_counter_identity(&entry.identity).is_ok())
}

fn validate_arm_report_shape(report: &ZeroBenchArmReportV1) -> Result<(), ZeroBenchErrorV1> {
    if report.omitted {
        if report.complete_runs != 0
            || report.failed_runs != 0
            || report.incomplete_runs != 0
            || report.paired_complete_runs != 0
            || report.raw_successes != 0
            || report.candidate_successes_before_guard != 0
            || report.published_successes != 0
            || report.rescues_n01 != 0
            || report.regressions_n10 != 0
            || report.fallback_runs != 0
            || report.conservative_hoeffding_lcb_ppm.is_some()
            || report.relative_gain_ppm.is_some()
            || report.lower_median_novel_token_reduction_ppm.is_some()
            || report.lower_median_time_reduction_ppm.is_some()
            || !report.strata.is_empty()
            || !report.repositories.is_empty()
            || report.aggregate_work.is_some()
            || report.aggregate_telemetry.is_some()
            || report.raw_paired_aggregate_work.is_some()
        {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::InvalidReport,
                "omitted arm carries measured or stratified results",
            ));
        }
        return Ok(());
    }
    if report.aggregate_telemetry.is_none() {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::InvalidReport,
            "included arm lacks separate reasoning/cache/agent/system telemetry",
        ));
    }
    if report.strata.len() != ZeroBenchTaskStratumV1::ALL.len()
        || ZeroBenchTaskStratumV1::ALL
            .iter()
            .zip(&report.strata)
            .any(|(expected, actual)| expected != &actual.stratum)
        || report.repositories.is_empty()
        || report
            .repositories
            .iter()
            .any(|repository| repository.repository_root_digest == DigestV1::ZERO)
        || report
            .repositories
            .windows(2)
            .any(|window| window[0].repository_root_digest >= window[1].repository_root_digest)
    {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::InvalidReport,
            "included arm lacks canonical stratum or repository reports",
        ));
    }
    let (Some(candidate), Some(raw)) = (&report.aggregate_work, &report.raw_paired_aggregate_work)
    else {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::InvalidReport,
            "included arm lacks paired aggregate work",
        ));
    };
    if candidate.coordinates.is_empty()
        || candidate.coordinates.len() != raw.coordinates.len()
        || candidate
            .coordinates
            .iter()
            .zip(&raw.coordinates)
            .any(|(left, right)| left.identity != right.identity)
    {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::InvalidReport,
            "paired aggregate work coordinates differ or are empty",
        ));
    }
    Ok(())
}

fn strictly_sorted_arm_reports(reports: &[ZeroBenchArmReportV1]) -> bool {
    reports
        .windows(2)
        .all(|window| window[0].arm < window[1].arm)
}

fn count_status(runs: &[&ZeroBenchVerifiedRunRecordV1], status: RunStatusV1) -> u64 {
    runs.iter().filter(|run| run.claim.status == status).count() as u64
}

fn empty_arm_report(arm: ZeroBenchArmV1, omitted: bool) -> ZeroBenchArmReportV1 {
    ZeroBenchArmReportV1 {
        arm,
        omitted,
        complete_runs: 0,
        failed_runs: 0,
        incomplete_runs: 0,
        paired_complete_runs: 0,
        raw_successes: 0,
        candidate_successes_before_guard: 0,
        published_successes: 0,
        rescues_n01: 0,
        regressions_n10: 0,
        fallback_runs: 0,
        conservative_hoeffding_lcb_ppm: None,
        relative_gain_ppm: None,
        lower_median_novel_token_reduction_ppm: None,
        lower_median_time_reduction_ppm: None,
        strata: Vec::new(),
        repositories: Vec::new(),
        aggregate_work: None,
        aggregate_telemetry: None,
        raw_paired_aggregate_work: None,
    }
}

fn verify_exact_run_payload(
    expected: &[u8],
    status: RunStatusV1,
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<(), ZeroBenchErrorV1> {
    let exit_code = match (evidence.query(), &evidence.certificate().completeness) {
        (Query::BuildReceipt { .. }, CompletenessWitness::BuildReceipt { exit_code, .. })
        | (Query::TestTrace { .. }, CompletenessWitness::TestTrace { exit_code, .. }) => *exit_code,
        _ => {
            return Err(zero_bench_error(
                ZeroBenchFailureCodeV1::UnsupportedEvidenceClass,
                "ZeroBench run authority needs an exact build/test result receipt",
            ));
        }
    };
    if (status == RunStatusV1::Complete) != (exit_code == 0) {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::InvalidRunStatus,
            "run status disagrees with the exact verifier exit status",
        ));
    }
    if evidence.payload() != expected {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::EvidencePayloadMismatch,
            "run verifier payload differs from canonical run claim bytes",
        ));
    }
    Ok(())
}

fn verifier_identity(evidence: &VerifiedEvidence<'_, '_>) -> DigestV1 {
    let provenance = &evidence.certificate().provenance;
    digest_value(
        VERIFIER_DOMAIN_V1,
        &json!({
            "index_id": provenance.index_id,
            "index_version": provenance.index_version,
            "operator_id": provenance.operator_id,
            "operator_version": provenance.operator_version,
            "parser_id": provenance.parser_id,
            "parser_version": provenance.parser_version,
        }),
    )
}

fn verified_evidence_digest(
    evidence: &VerifiedEvidence<'_, '_>,
) -> Result<DigestV1, ZeroBenchErrorV1> {
    let certificate = evidence.certificate();
    Ok(digest_value(
        RUN_AUTHORITY_DOMAIN_V1,
        &json!({
            "completeness": certificate.completeness,
            "payload_sha256": DigestV1::from_bytes(zero_abi::sha256(evidence.payload())),
            "provenance": certificate.provenance,
            "query": certificate.query,
            "spans": certificate.spans,
        }),
    ))
}

fn run_authority_digest(
    record: &ZeroBenchVerifiedRunRecordV1,
) -> Result<DigestV1, ZeroBenchErrorV1> {
    Ok(digest_value(
        RUN_AUTHORITY_DOMAIN_V1,
        &json!({
            "claim_digest": record.claim_digest,
            "contract_version": record.contract_version,
            "evidence_digest": record.evidence_digest,
        }),
    ))
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ZeroBenchErrorV1> {
    let value = serde_json::to_value(value).map_err(json_error)?;
    Ok(canonical_json(&value).into_bytes())
}

fn digest_serializable<T: Serialize>(
    domain: &[u8],
    value: &T,
) -> Result<DigestV1, ZeroBenchErrorV1> {
    Ok(domain_digest(domain, &canonical_bytes(value)?))
}

fn digest_value(domain: &[u8], value: &Value) -> DigestV1 {
    domain_digest(domain, canonical_json(value).as_bytes())
}

fn domain_digest(domain: &[u8], bytes: &[u8]) -> DigestV1 {
    let mut input = Vec::with_capacity(domain.len() + bytes.len());
    input.extend_from_slice(domain);
    input.extend_from_slice(bytes);
    DigestV1::from_bytes(zero_abi::sha256(&input))
}

fn require_nonzero(label: &'static str, values: &[DigestV1]) -> Result<(), ZeroBenchErrorV1> {
    if values.contains(&DigestV1::ZERO) {
        return Err(zero_bench_error(
            ZeroBenchFailureCodeV1::ZeroDigest,
            format!("{label} contains a zero digest"),
        ));
    }
    Ok(())
}

fn arithmetic_error() -> ZeroBenchErrorV1 {
    zero_bench_error(
        ZeroBenchFailureCodeV1::ArithmeticOverflow,
        "ZeroBench checked integer arithmetic overflowed",
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZeroBenchFailureCodeV1 {
    SchemaVersionMismatch,
    ZeroDigest,
    DigestMismatch,
    InvalidRegistration,
    MissingRequiredArm,
    MissingTaskStratum,
    ThresholdMismatch,
    ReplicationMismatch,
    ArmUnavailable,
    UnknownCase,
    InvalidRunStatus,
    InvalidPublicationEvidence,
    InvalidRunMetrics,
    IncompleteMeasuredWork,
    ResourceCoordinateMismatch,
    RunBindingMismatch,
    UnsupportedEvidenceClass,
    EvidencePayloadMismatch,
    VerifierIdentityMismatch,
    IncompleteRunMatrix,
    InvalidStatistics,
    ArithmeticOverflow,
    InvalidReport,
    CanonicalPayloadTooLarge,
    NonCanonicalEncoding,
    InvalidJson,
    DependencyFailure,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ZeroBenchErrorV1 {
    code: ZeroBenchFailureCodeV1,
    detail: String,
}

impl ZeroBenchErrorV1 {
    pub const fn failure_code(&self) -> ZeroBenchFailureCodeV1 {
        self.code
    }
}

impl fmt::Display for ZeroBenchErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ZeroBench-R {:?}: {}", self.code, self.detail)
    }
}

impl Error for ZeroBenchErrorV1 {}

fn zero_bench_error(code: ZeroBenchFailureCodeV1, detail: impl Into<String>) -> ZeroBenchErrorV1 {
    ZeroBenchErrorV1 {
        code,
        detail: detail.into(),
    }
}

fn json_error(error: serde_json::Error) -> ZeroBenchErrorV1 {
    zero_bench_error(ZeroBenchFailureCodeV1::InvalidJson, error.to_string())
}

fn causal_error(error: zero_ledger::CausalWorkErrorV1) -> ZeroBenchErrorV1 {
    zero_bench_error(ZeroBenchFailureCodeV1::DependencyFailure, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{borrow::Cow, collections::BTreeSet};
    use zero_cert::{
        EvidenceCertificate, ObjectId, OperatorLock, Provenance, Resolver, SpanRef, TestId, verify,
    };
    use zero_gate::{
        FrozenBaselineV1, MetricOrderV1, PointwiseDominanceCertificateV1, ProtectedMetricV1,
        QualityEvidenceV1, QualityPairV1,
    };
    use zero_ledger::{
        CausalWorkChargeV1, CausalWorkClassV1, CausalWorkOutcomeV1, ParentCounterObservationV1,
        ParentCounterWindowV1, ResiduePolicyV1,
    };

    fn d(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    struct Resident {
        bytes: Vec<u8>,
    }

    impl Resolver for Resident {
        fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
            (object_id.0 == zero_abi::sha256(&self.bytes)).then_some(&self.bytes)
        }
        fn trusted_operator_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "zerobench-verifier").then_some("1")
        }
        fn trusted_parser_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "zerobench-parser").then_some("1")
        }
        fn trusted_index_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "zerobench-index").then_some("1")
        }
    }

    fn evidence(bytes: &[u8]) -> (EvidenceCertificate<'static>, Resident) {
        evidence_with_exit(bytes, 0)
    }

    fn evidence_with_exit(
        bytes: &[u8],
        exit_code: i32,
    ) -> (EvidenceCertificate<'static>, Resident) {
        let digest = zero_abi::sha256(bytes);
        let span = SpanRef {
            object_id: ObjectId(digest),
            object_digest: digest,
            byte_start: 0,
            byte_len: bytes.len() as u64,
            span_digest: digest,
        };
        (
            EvidenceCertificate {
                query: Query::TestTrace { test: TestId(77) },
                spans: vec![span],
                payload: Cow::Owned(bytes.to_vec()),
                provenance: Provenance {
                    parser_id: "zerobench-parser".into(),
                    parser_version: "1".into(),
                    index_id: "zerobench-index".into(),
                    index_version: "1".into(),
                    operator_id: "zerobench-verifier".into(),
                    operator_version: "1".into(),
                },
                completeness: CompletenessWitness::TestTrace {
                    operator: OperatorLock {
                        operator_id: "zerobench-verifier".into(),
                        operator_version: "1".into(),
                    },
                    test: TestId(77),
                    exit_code,
                    trace_digest: digest,
                },
                input_token_cost: 0,
                backend_work_units: 1,
            },
            Resident {
                bytes: bytes.to_vec(),
            },
        )
    }

    fn result_verifier_identity() -> DigestV1 {
        let (certificate, resident) = evidence(b"identity");
        let verified = verify(&certificate, &resident).unwrap();
        verifier_identity(&verified)
    }

    fn counter(id: &str, unit: CausalCounterUnitV1, byte: u8) -> ParentCounterIdentityV1 {
        ParentCounterIdentityV1 {
            counter_id: id.into(),
            unit,
            boundary_digest: d(byte),
            adapter_digest: d(byte.wrapping_add(1)),
            platform_profile_digest: d(byte.wrapping_add(2)),
        }
    }

    fn identity() -> BenchmarkComparisonIdentityV1 {
        BenchmarkComparisonIdentityV1 {
            model_identity_digest: d(1),
            backend_identity_digest: d(2),
            decoder_seed_policy_digest: d(3),
            reasoning_contract_digest: d(4),
            output_headroom_digest: d(5),
            tool_authority_sandbox_digest: d(6),
            repository_set_digest: d(7),
            protected_predicate_digest: d(8),
            verifier_identity_digest: result_verifier_identity(),
            hardware_network_class_digest: d(10),
            timeout_resource_limits_digest: d(11),
            fallback_rights_digest: d(12),
            setup_amortization_horizon_digest: d(13),
            assembly_manifest_digest: d(14),
        }
    }

    fn pin(byte: u8) -> AdapterPinV1 {
        AdapterPinV1 {
            pinned_revision_digest: d(byte),
            install_procedure_digest: d(byte.wrapping_add(1)),
            configuration_digest: d(byte.wrapping_add(2)),
            model_tool_integration_digest: d(byte.wrapping_add(3)),
            indexing_policy_digest: d(byte.wrapping_add(4)),
            command_surface_digest: d(byte.wrapping_add(5)),
            cache_policy_digest: d(byte.wrapping_add(6)),
            fallback_policy_digest: d(byte.wrapping_add(7)),
            metrics_extractor_digest: d(byte.wrapping_add(8)),
            license_status_digest: d(byte.wrapping_add(9)),
            secondary_model_identity_digest: None,
        }
    }

    fn cases() -> Vec<BenchmarkCaseV1> {
        let mut cases = Vec::new();
        for (index, stratum) in ZeroBenchTaskStratumV1::ALL.into_iter().enumerate() {
            for trial in 0..2 {
                cases.push(
                    BenchmarkCaseV1::new(
                        d(40 + index as u8),
                        d(70 + (index % 2) as u8),
                        stratum,
                        trial,
                        Some(1_000 + trial as u64),
                        d(8),
                        result_verifier_identity(),
                    )
                    .unwrap(),
                );
            }
        }
        cases
    }

    fn arms() -> Vec<RegisteredArmV1> {
        ZeroBenchArmV1::ALL
            .into_iter()
            .enumerate()
            .map(|(index, arm)| RegisteredArmV1 {
                arm,
                registration: ArmRegistrationV1::Included {
                    pin: Box::new(pin(150 + index as u8 * 10)),
                },
            })
            .collect()
    }

    fn make_registration(intent: ZeroBenchIntentV1) -> ZeroBenchRegistrationV1 {
        let cpu = counter("parent.cpu_ns", CausalCounterUnitV1::CpuNanoseconds, 20);
        let tokens = counter(
            "parent.novel_causal_tokens",
            CausalCounterUnitV1::Tokens,
            30,
        );
        let wall = counter("parent.wall_ns", CausalCounterUnitV1::WallNanoseconds, 40);
        let bytes = counter("parent.bytes", CausalCounterUnitV1::Bytes, 50);
        let calls = counter("parent.calls", CausalCounterUnitV1::Calls, 60);
        let allocated = counter(
            "parent.allocated_bytes",
            CausalCounterUnitV1::AllocatedBytes,
            70,
        );
        let io = counter("parent.io_bytes", CausalCounterUnitV1::IoBytes, 80);
        let token_digest = resource_identity_digest(&tokens).unwrap();
        ZeroBenchRegistrationV1::new(
            intent,
            d(15),
            d(16),
            d(17),
            44,
            d(21),
            identity(),
            vec![cpu, tokens, wall, bytes, calls, allocated, io],
            token_digest,
            cases(),
            arms(),
            ReplicationClassV1::Independent,
            Some(d(22)),
        )
        .unwrap()
    }

    fn work_receipt(
        identity: ParentCounterIdentityV1,
        amount: u64,
        work_id: u8,
        guarded: bool,
    ) -> CausalWorkReceiptV1 {
        let CausalWorkOutcomeV1::Measured { receipt } = CausalWorkReceiptV1::build(
            d(14),
            ParentCounterObservationV1::Measured {
                window: ParentCounterWindowV1 {
                    identity,
                    start: 10,
                    end: 10 + amount,
                },
            },
            if guarded {
                vec![
                    CausalWorkChargeV1 {
                        work_unit_id: d(work_id),
                        class: CausalWorkClassV1::Candidate,
                        amount: amount - 1,
                    },
                    CausalWorkChargeV1 {
                        work_unit_id: d(work_id.wrapping_add(100)),
                        class: CausalWorkClassV1::Verification,
                        amount: 1,
                    },
                ]
            } else {
                vec![CausalWorkChargeV1 {
                    work_unit_id: d(work_id),
                    class: CausalWorkClassV1::Candidate,
                    amount,
                }]
            },
            ResiduePolicyV1::RejectUnclassified,
        )
        .unwrap() else {
            panic!("measured receipt expected")
        };
        receipt
    }

    fn work(
        registration: &ZeroBenchRegistrationV1,
        amount: u64,
        salt: u8,
        guarded: bool,
    ) -> Vec<CausalWorkReceiptV1> {
        registration
            .protected_resource_coordinates
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, identity)| {
                work_receipt(identity, amount, salt.wrapping_add(index as u8), guarded)
            })
            .collect()
    }

    fn metrics(
        time: u64,
    ) -> (
        ReasoningMetricsV1,
        CacheMetricsV1,
        AgentMetricsV1,
        SystemMetricsV1,
    ) {
        (
            ReasoningMetricsV1 {
                hidden_reasoning_tokens: MeasuredOrUnavailableU64V1::Unavailable,
                visible_output_tokens: 10,
                reasoning_turns_before_first_candidate: 1,
                semantic_reasoning_turns_before_first_candidate: 1,
                orchestration_turns_before_first_candidate: 0,
                incomplete_during_reasoning: false,
                reasoning_state_status: ReasoningStateStatusV1::Unavailable,
            },
            CacheMetricsV1 {
                logical_input_tokens: 100,
                provider_uncached_input_tokens: 100,
                provider_reported_cached_input_tokens: None,
                provider_cache_eligible: true,
                byte_identical_prefix_eligible: false,
                exact_local_artifact_reuse: 0,
                exact_reasoning_continuation_reuse: false,
                context_occupancy_tokens: 120,
            },
            AgentMetricsV1 {
                correct_locus_at_1: true,
                exact_read_at_1: true,
                verified_effect_at_1: true,
                model_calls: 1,
                tool_calls: 1,
                time_to_first_relevant_byte_micros: time / 4,
                time_to_first_candidate_micros: time / 2,
                time_to_verified_effect_micros: time,
            },
            SystemMetricsV1 {
                indexing_preparation_micros: 5,
                incremental_update_micros: Some(2),
                peak_rss_bytes: 1_000,
                disk_footprint_bytes: 500,
                stale_artifact_count: 0,
                crash_recovery_correct: Some(true),
                suite_processes_at_rest: 0,
            },
        )
    }

    fn quality_admission(
        registration: &ZeroBenchRegistrationV1,
        arm: ZeroBenchArmV1,
        case: &BenchmarkCaseV1,
        baseline_success: bool,
        candidate_success: bool,
    ) -> QualityAdmissionV1 {
        let raw_identity = registration
            .included_pin(ZeroBenchArmV1::Raw)
            .unwrap()
            .digest()
            .unwrap();
        let candidate_identity = registration.included_pin(arm).unwrap().digest().unwrap();
        let pair = QualityPairV1::new(
            case.case_digest,
            registration.comparison_identity.digest().unwrap(),
            raw_identity,
            candidate_identity,
            zero_bench_protected_outcome_digest_v1(baseline_success),
            zero_bench_protected_outcome_digest_v1(candidate_success),
            d(221),
            d(222),
            vec![ProtectedMetricV1 {
                metric_id: "protected_success".into(),
                order: MetricOrderV1::AtLeast,
                baseline_value: i64::from(baseline_success),
                candidate_value: i64::from(candidate_success),
            }],
        )
        .unwrap();
        let (certificate, resident) = evidence(&pair.canonical_bytes().unwrap());
        let verified = verify(&certificate, &resident).unwrap();
        let pointwise = PointwiseDominanceCertificateV1::verify(
            &pair,
            case.protected_predicate_digest,
            &verified,
        )
        .unwrap();
        QualityAdmissionV1::admit_strict(
            QualityEvidenceV1::PointwiseDominance(pointwise),
            FrozenBaselineV1::new(
                raw_identity,
                zero_bench_protected_outcome_digest_v1(baseline_success),
                d(223),
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn verified_run(
        registration: &ZeroBenchRegistrationV1,
        arm: ZeroBenchArmV1,
        case: &BenchmarkCaseV1,
        raw_success: bool,
        regression: bool,
        incomplete: bool,
    ) -> VerifiedZeroBenchRunV1 {
        let (published, candidate, candidate_published, evidence_class, fallback, amount, time) =
            match arm {
                ZeroBenchArmV1::Raw => (
                    Some(raw_success),
                    None,
                    false,
                    PublicationEvidenceClassV1::RawBaseline,
                    false,
                    100,
                    100,
                ),
                ZeroBenchArmV1::RaccRStrict => (
                    Some(raw_success),
                    Some(raw_success),
                    true,
                    PublicationEvidenceClassV1::PointwiseDominance,
                    false,
                    60,
                    80,
                ),
                ZeroBenchArmV1::RaccRAmplify => {
                    let result = if regression && raw_success {
                        false
                    } else {
                        true
                    };
                    (
                        Some(result),
                        Some(result),
                        true,
                        PublicationEvidenceClassV1::PointwiseDominance,
                        false,
                        40,
                        40,
                    )
                }
                ZeroBenchArmV1::RaccRCandidate => (
                    Some(raw_success),
                    Some(raw_success),
                    true,
                    PublicationEvidenceClassV1::UnguardedCandidate,
                    false,
                    70,
                    90,
                ),
                _ => (
                    Some(raw_success),
                    Some(raw_success),
                    true,
                    PublicationEvidenceClassV1::ExternalSystem,
                    false,
                    80,
                    90,
                ),
            };
        let (reasoning, cache, agent, system) = metrics(time);
        let receipts = work(
            registration,
            amount,
            (case.trial_index as u8).wrapping_add(1),
            arm.is_racc_guarded(),
        );
        let status = if incomplete && arm == ZeroBenchArmV1::RaccRAmplify {
            RunStatusV1::Incomplete
        } else {
            RunStatusV1::Complete
        };
        let evidence_class = if status != RunStatusV1::Complete && arm.is_racc_guarded() {
            PublicationEvidenceClassV1::Unidentified
        } else {
            evidence_class
        };
        let quality_admission = (status == RunStatusV1::Complete && arm.is_racc_guarded())
            .then(|| quality_admission(registration, arm, case, raw_success, candidate.unwrap()));
        let claim = zero_bench_run_claim_v1(
            registration,
            arm,
            case.case_digest,
            status,
            (status != RunStatusV1::Complete).then_some(d(230)),
            candidate,
            (status == RunStatusV1::Complete).then_some(published.unwrap()),
            candidate_published && status == RunStatusV1::Complete,
            evidence_class,
            fallback,
            reasoning,
            cache,
            agent,
            system,
            quality_admission.as_ref(),
            None,
            &receipts,
            result_verifier_identity(),
        )
        .unwrap();
        let (certificate, resident) = if status == RunStatusV1::Complete {
            evidence(&claim.canonical_bytes().unwrap())
        } else {
            evidence_with_exit(&claim.canonical_bytes().unwrap(), 1)
        };
        let verified = verify(&certificate, &resident).unwrap();
        verify_zero_bench_run_v1(
            registration,
            claim,
            quality_admission.as_ref(),
            None,
            &receipts,
            &verified,
        )
        .unwrap()
    }

    fn run_matrix(
        registration: &ZeroBenchRegistrationV1,
        regression: bool,
        incomplete: bool,
    ) -> Vec<VerifiedZeroBenchRunV1> {
        let mut runs = Vec::new();
        for arm in ZeroBenchArmV1::ALL {
            for (index, case) in registration.cases.iter().enumerate() {
                runs.push(verified_run(
                    registration,
                    arm,
                    case,
                    index % 5 == 0,
                    regression && index == 0,
                    incomplete && index == 0,
                ));
            }
        }
        runs
    }

    #[test]
    fn release_report_keeps_strata_and_passes_strict_and_amplify_gates() {
        let registration = make_registration(ZeroBenchIntentV1::Release);
        let report =
            compile_zero_bench_report_v1(&registration, run_matrix(&registration, false, false))
                .unwrap();
        assert_eq!(report.strict_verdict(), ReleaseVerdictV1::Pass);
        assert_eq!(report.amplify_verdict(), ReleaseVerdictV1::Pass);
        assert!(!report.permits_runtime_publication());
        assert!(!report.permits_best_in_class_claim());
        let amplify = report
            .record()
            .arms
            .iter()
            .find(|arm| arm.arm == ZeroBenchArmV1::RaccRAmplify)
            .unwrap();
        assert_eq!(amplify.paired_complete_runs, 30);
        assert_eq!(amplify.raw_successes, 6);
        assert_eq!(amplify.rescues_n01, 24);
        assert_eq!(amplify.regressions_n10, 0);
        assert_eq!(amplify.conservative_hoeffding_lcb_ppm, Some(576_393));
        assert_eq!(amplify.relative_gain_ppm, Some(4_000_000));
        assert_eq!(
            amplify.lower_median_novel_token_reduction_ppm,
            Some(600_000)
        );
        assert_eq!(amplify.lower_median_time_reduction_ppm, Some(600_000));
        assert_eq!(amplify.strata.len(), 15);
        assert_eq!(amplify.repositories.len(), 2);
        let telemetry = amplify.aggregate_telemetry.as_ref().unwrap();
        assert_eq!(telemetry.hidden_reasoning_unavailable_runs, 30);
        assert_eq!(telemetry.visible_output_tokens_total, 300);
        assert_eq!(telemetry.provider_cache_eligible_runs, 30);
        assert_eq!(telemetry.peak_rss_bytes_max, 1_000);
        assert_eq!(
            amplify.aggregate_work.as_ref().unwrap().coordinates[0]
                .class_totals
                .candidate,
            1_170
        );
        assert_eq!(
            amplify.aggregate_work.as_ref().unwrap().coordinates[0]
                .class_totals
                .verification,
            30
        );
        let bytes = report.record().canonical_bytes().unwrap();
        let value = serde_json::to_value(report.record()).unwrap();
        let keys = value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            [
                "schema_version",
                "contract_version",
                "registration_digest",
                "arms",
                "strict_verdict",
                "amplify_verdict",
                "all_required_arms_included",
                "best_in_class_claim_authorized",
                "report_digest",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
        );
        assert_eq!(
            ZeroBenchReportRecordV1::from_canonical_bytes(&bytes).unwrap(),
            *report.record()
        );
    }

    #[test]
    fn regression_and_incomplete_run_block_amplify_release() {
        let registration = make_registration(ZeroBenchIntentV1::Release);
        let case = &registration.cases[0];
        assert!(
            QualityPairV1::new(
                case.task_identity_digest,
                registration.comparison_identity.digest().unwrap(),
                registration
                    .included_pin(ZeroBenchArmV1::Raw)
                    .unwrap()
                    .digest()
                    .unwrap(),
                registration
                    .included_pin(ZeroBenchArmV1::RaccRAmplify)
                    .unwrap()
                    .digest()
                    .unwrap(),
                zero_bench_protected_outcome_digest_v1(true),
                zero_bench_protected_outcome_digest_v1(false),
                d(221),
                d(222),
                vec![ProtectedMetricV1 {
                    metric_id: "protected_success".into(),
                    order: MetricOrderV1::AtLeast,
                    baseline_value: 1,
                    candidate_value: 0,
                }],
            )
            .is_err()
        );

        let incomplete =
            compile_zero_bench_report_v1(&registration, run_matrix(&registration, false, true))
                .unwrap();
        assert_eq!(incomplete.amplify_verdict(), ReleaseVerdictV1::Incomplete);
    }

    #[test]
    fn missing_run_omission_and_secondary_model_fail_closed() {
        let registration = make_registration(ZeroBenchIntentV1::Release);
        let mut runs = run_matrix(&registration, false, false);
        runs.pop();
        assert_eq!(
            compile_zero_bench_report_v1(&registration, runs)
                .err()
                .unwrap()
                .failure_code(),
            ZeroBenchFailureCodeV1::IncompleteRunMatrix
        );

        let mut omitted_arms = arms();
        omitted_arms[1].registration = ArmRegistrationV1::Omitted {
            reason: OmissionReasonV1::Technical,
            evidence_digest: d(240),
        };
        let mut omitted = make_registration(ZeroBenchIntentV1::Smoke);
        omitted.arms = omitted_arms;
        omitted.run_slots =
            registered_run_slots(omitted.arm_order_seed, &omitted.cases, &omitted.arms).unwrap();
        omitted.stopping_rule_digest = digest_value(
            STOPPING_RULE_DOMAIN_V1,
            &json!({
                "mode": "fixed_registered_run_matrix",
                "run_count": omitted.run_slots.len(),
            }),
        );
        omitted.registration_digest = omitted.expected_digest().unwrap();
        omitted.validate().unwrap();

        let mut hidden = arms();
        let ArmRegistrationV1::Included { pin } = &mut hidden[0].registration else {
            panic!("raw is included")
        };
        pin.secondary_model_identity_digest = Some(d(241));
        let mut registration = make_registration(ZeroBenchIntentV1::Smoke);
        registration.arms = hidden;
        registration.registration_digest = registration.expected_digest().unwrap();
        assert_eq!(
            registration.validate().err().unwrap().failure_code(),
            ZeroBenchFailureCodeV1::InvalidRegistration
        );
    }

    #[test]
    fn cache_quality_and_evidence_laundering_are_rejected() {
        let registration = make_registration(ZeroBenchIntentV1::Smoke);
        let case = &registration.cases[0];
        let replay = verified_run(&registration, ZeroBenchArmV1::Raw, case, true, false, false);
        let bytes = replay.record().canonical_bytes().unwrap();
        let value = serde_json::to_value(replay.record()).unwrap();
        let keys = value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            [
                "contract_version",
                "claim",
                "claim_digest",
                "evidence_digest",
                "authority_digest"
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
        );
        assert_eq!(
            ZeroBenchVerifiedRunRecordV1::from_canonical_bytes(&bytes).unwrap(),
            *replay.record()
        );
        let mut whitespace = bytes;
        whitespace.push(b'\n');
        assert_eq!(
            ZeroBenchVerifiedRunRecordV1::from_canonical_bytes(&whitespace)
                .err()
                .unwrap()
                .failure_code(),
            ZeroBenchFailureCodeV1::NonCanonicalEncoding
        );
        let receipts = work(&registration, 10, 1, false);
        let (reasoning, cache, agent, system) = metrics(10);
        assert_eq!(
            zero_bench_run_claim_v1(
                &registration,
                ZeroBenchArmV1::RaccRCandidate,
                case.case_digest,
                RunStatusV1::Complete,
                None,
                Some(true),
                Some(true),
                true,
                PublicationEvidenceClassV1::UnguardedCandidate,
                false,
                reasoning,
                cache,
                agent,
                system,
                None,
                None,
                &receipts,
                d(250),
            )
            .err()
            .unwrap()
            .failure_code(),
            ZeroBenchFailureCodeV1::VerifierIdentityMismatch
        );
        let (reasoning, mut cache, agent, system) = metrics(10);
        cache.provider_cache_eligible = false;
        cache.provider_reported_cached_input_tokens = Some(1);
        assert_eq!(
            zero_bench_run_claim_v1(
                &registration,
                ZeroBenchArmV1::RaccRCandidate,
                case.case_digest,
                RunStatusV1::Complete,
                None,
                Some(true),
                Some(true),
                true,
                PublicationEvidenceClassV1::UnguardedCandidate,
                false,
                reasoning,
                cache,
                agent,
                system,
                None,
                None,
                &receipts,
                result_verifier_identity(),
            )
            .err()
            .unwrap()
            .failure_code(),
            ZeroBenchFailureCodeV1::InvalidRunMetrics
        );

        let (reasoning, cache, agent, system) = metrics(10);
        assert_eq!(
            zero_bench_run_claim_v1(
                &registration,
                ZeroBenchArmV1::RaccRStrict,
                case.case_digest,
                RunStatusV1::Complete,
                None,
                Some(true),
                Some(true),
                true,
                PublicationEvidenceClassV1::DistributionalOnly,
                false,
                reasoning,
                cache,
                agent,
                system,
                None,
                None,
                &receipts,
                result_verifier_identity(),
            )
            .err()
            .unwrap()
            .failure_code(),
            ZeroBenchFailureCodeV1::InvalidPublicationEvidence
        );

        let (reasoning, cache, agent, system) = metrics(10);
        let claim = zero_bench_run_claim_v1(
            &registration,
            ZeroBenchArmV1::RaccRCandidate,
            case.case_digest,
            RunStatusV1::Complete,
            None,
            Some(true),
            Some(true),
            true,
            PublicationEvidenceClassV1::UnguardedCandidate,
            false,
            reasoning,
            cache,
            agent,
            system,
            None,
            None,
            &receipts,
            result_verifier_identity(),
        )
        .unwrap();
        let (certificate, resident) = evidence(b"unrelated");
        let verified = verify(&certificate, &resident).unwrap();
        assert_eq!(
            verify_zero_bench_run_v1(&registration, claim, None, None, &receipts, &verified,)
                .err()
                .unwrap()
                .failure_code(),
            ZeroBenchFailureCodeV1::EvidencePayloadMismatch
        );
    }

    #[test]
    fn registration_requires_strata_repeats_and_frozen_thresholds() {
        let mut registration = make_registration(ZeroBenchIntentV1::Release);
        registration
            .cases
            .retain(|case| case.stratum != ZeroBenchTaskStratumV1::MultiRepositoryChange);
        registration.registration_digest = registration.expected_digest().unwrap();
        assert_eq!(
            registration.validate().err().unwrap().failure_code(),
            ZeroBenchFailureCodeV1::MissingTaskStratum
        );

        let mut registration = make_registration(ZeroBenchIntentV1::Release);
        let task = registration.cases[0].task_identity_digest;
        let keep = registration
            .cases
            .iter()
            .filter(|case| case.task_identity_digest == task)
            .map(|case| case.case_digest)
            .max()
            .unwrap();
        registration
            .cases
            .retain(|case| case.task_identity_digest != task || case.case_digest == keep);
        registration.registration_digest = registration.expected_digest().unwrap();
        assert_eq!(
            registration.validate().err().unwrap().failure_code(),
            ZeroBenchFailureCodeV1::InvalidRegistration
        );

        let mut registration = make_registration(ZeroBenchIntentV1::Release);
        registration.run_slots.swap(0, 1);
        registration.registration_digest = registration.expected_digest().unwrap();
        assert_eq!(
            registration.validate().err().unwrap().failure_code(),
            ZeroBenchFailureCodeV1::InvalidRegistration
        );

        let mut registration = make_registration(ZeroBenchIntentV1::Release);
        registration.alpha_ppm = 100_000;
        registration.registration_digest = registration.expected_digest().unwrap();
        assert_eq!(
            registration.validate().err().unwrap().failure_code(),
            ZeroBenchFailureCodeV1::ThresholdMismatch
        );
    }

    #[test]
    fn integer_statistics_are_conservative_and_overflow_checked() {
        assert_eq!(conservative_hoeffding_lcb_ppm(24, 30).unwrap(), 576_393);
        assert_eq!(checked_ratio_ppm_u64(24, 6).unwrap(), 4_000_000);
        assert_eq!(paired_reduction_ppm(100, 40).unwrap(), Some(600_000));
        assert_eq!(paired_reduction_ppm(0, 0).unwrap(), None);
        assert_eq!(ceil_sqrt_u128(50_000_000_000).unwrap(), 223_607);
    }

    #[test]
    fn contract_and_external_schema_digests_are_stable() {
        // 37d827f93b60ac253a4678d839b12023b2bcad3f24cbcf05e419c7f739085cc4
        assert_eq!(
            zero_bench_r_contract_digest_v1(),
            DigestV1::from_bytes([
                0x37, 0xd8, 0x27, 0xf9, 0x3b, 0x60, 0xac, 0x25, 0x3a, 0x46, 0x78, 0xd8, 0x39, 0xb1,
                0x20, 0x23, 0xb2, 0xbc, 0xad, 0x3f, 0x24, 0xcb, 0xcf, 0x05, 0xe4, 0x19, 0xc7, 0xf7,
                0x39, 0x08, 0x5c, 0xc4,
            ])
        );
        assert_eq!(
            DigestV1::from_bytes(zero_abi::sha256(include_bytes!(
                "../../../conformance/schemas/zerobench-r-run-v1.schema.json"
            )))
            .to_hex(),
            ZERO_BENCH_R_RUN_SCHEMA_SHA256_V1
        );
        assert_eq!(
            DigestV1::from_bytes(zero_abi::sha256(include_bytes!(
                "../../../conformance/schemas/zerobench-r-report-v1.schema.json"
            )))
            .to_hex(),
            ZERO_BENCH_R_REPORT_SCHEMA_SHA256_V1
        );
    }
}
