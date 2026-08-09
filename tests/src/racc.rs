//! Deterministic, engine-free RACC release-gate conformance checks.

use crate::checks::CheckStatus;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const RACC_CERTIFICATE_SCHEMA: &str = include_str!("../contracts/racc-certificate.schema.json");
pub const RACC_RECEIPT_SCHEMA: &str = include_str!("../contracts/racc-receipt.schema.json");
pub const RACC_TASK_ACCEPTANCE_RECEIPT_SCHEMA: &str =
    include_str!("../contracts/racc-task-acceptance-receipt.schema.json");
pub const RACC_INVALIDATION_FRESHNESS_SCHEMA: &str =
    include_str!("../contracts/invalidation-freshness-v1.schema.json");
pub const RACC_TWO_PHASE_GATE_SCHEMA: &str =
    include_str!("../contracts/two-phase-gate-v1.schema.json");

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RaccGateId {
    #[serde(rename = "RACC-CERT")]
    Cert,
    #[serde(rename = "RACC-RECEIPT")]
    Receipt,
    #[serde(rename = "RACC-GATE-IRREV")]
    GateIrrev,
    #[serde(rename = "RACC-BUDGET")]
    Budget,
    #[serde(rename = "RACC-INLINE")]
    Inline,
    #[serde(rename = "RACC-RESIDENCY")]
    Residency,
    #[serde(rename = "RACC-TASK-TRANSACTION")]
    TaskTransaction,
}

impl RaccGateId {
    pub const ALL: [Self; 7] = [
        Self::Cert,
        Self::Receipt,
        Self::GateIrrev,
        Self::Budget,
        Self::Inline,
        Self::Residency,
        Self::TaskTransaction,
    ];
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RaccCheckResult {
    pub id: RaccGateId,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RaccHarnessReport {
    pub checks: Vec<RaccCheckResult>,
    pub passed: usize,
    pub failed: usize,
}

impl RaccHarnessReport {
    pub fn all_pass(&self) -> bool {
        self.failed == 0 && self.checks.len() == RaccGateId::ALL.len()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypedQuery {
    ReadSpan {
        object: String,
        start: u64,
        end: u64,
    },
    ExactSearch {
        scope: String,
        pattern: Vec<u8>,
    },
    Definition {
        symbol: u64,
    },
    References {
        symbol: u64,
    },
    AstClosure {
        seeds: Vec<u64>,
        relations: u64,
        radius: u32,
    },
    CallPath {
        source: u64,
        target: u64,
    },
    DataflowSlice {
        sink: u64,
    },
    Diff {
        old: String,
        new: String,
    },
    BuildReceipt {
        command: u64,
    },
    TestTrace {
        test: u64,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Provenance {
    pub parser_id: String,
    pub parser_version: String,
    pub index_id: String,
    pub index_version: String,
    pub operator_id: String,
    pub operator_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CompletenessWitness {
    ReadSpan,
    ExactSearch {
        scope: String,
        pattern: Vec<u8>,
        scope_len: u64,
        match_count: u64,
    },
    Definition {
        symbol: u64,
        index_id: String,
        index_version: String,
    },
    References {
        symbol: u64,
        index_id: String,
        index_version: String,
        match_count: u64,
    },
    AstClosure {
        seeds: Vec<u64>,
        relations: u64,
        radius: u32,
        parser_id: String,
        parser_version: String,
        visited_nodes: u64,
    },
    CallPath {
        source: u64,
        target: u64,
        edge_count: u64,
    },
    DataflowSlice {
        sink: u64,
        visited_nodes: u64,
    },
    Diff {
        old: String,
        new: String,
    },
    BuildReceipt {
        command: u64,
        exit_code: i32,
        stdout_digest: String,
        stderr_digest: String,
    },
    TestTrace {
        test: u64,
        exit_code: i32,
        trace_digest: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RaccCertificate {
    pub schema_version: u32,
    pub domain: String,
    pub query: TypedQuery,
    pub payload: Vec<Vec<u8>>,
    pub provenance: Provenance,
    pub completeness: CompletenessWitness,
}

#[derive(Clone, Debug)]
pub struct QueryFixture {
    pub query: TypedQuery,
    pub source: &'static [u8],
}

pub fn immutable_query_fixtures() -> Vec<QueryFixture> {
    const SOURCE: &[u8] = b"alpha beta alpha\nfn target() { beta(); }\n";
    vec![
        QueryFixture {
            query: TypedQuery::ReadSpan {
                object: "fixture-v1".into(),
                start: 0,
                end: 5,
            },
            source: SOURCE,
        },
        QueryFixture {
            query: TypedQuery::ExactSearch {
                scope: "fixture-v1".into(),
                pattern: b"alpha".to_vec(),
            },
            source: SOURCE,
        },
        QueryFixture {
            query: TypedQuery::Definition { symbol: 7 },
            source: SOURCE,
        },
        QueryFixture {
            query: TypedQuery::References { symbol: 7 },
            source: SOURCE,
        },
        QueryFixture {
            query: TypedQuery::AstClosure {
                seeds: vec![1, 2],
                relations: 3,
                radius: 2,
            },
            source: SOURCE,
        },
        QueryFixture {
            query: TypedQuery::CallPath {
                source: 7,
                target: 9,
            },
            source: SOURCE,
        },
        QueryFixture {
            query: TypedQuery::DataflowSlice { sink: 4 },
            source: SOURCE,
        },
        QueryFixture {
            query: TypedQuery::Diff {
                old: "fixture-v0".into(),
                new: "fixture-v1".into(),
            },
            source: SOURCE,
        },
        QueryFixture {
            query: TypedQuery::BuildReceipt { command: 11 },
            source: SOURCE,
        },
        QueryFixture {
            query: TypedQuery::TestTrace { test: 13 },
            source: SOURCE,
        },
    ]
}

fn locked_provenance() -> Provenance {
    Provenance {
        parser_id: "fixture-parser".into(),
        parser_version: "1".into(),
        index_id: "fixture-index".into(),
        index_version: "4".into(),
        operator_id: "fixture-operator".into(),
        operator_version: "2".into(),
    }
}

fn count_matches(source: &[u8], pattern: &[u8]) -> u64 {
    if pattern.is_empty() {
        return 0;
    }
    source
        .windows(pattern.len())
        .filter(|window| *window == pattern)
        .count() as u64
}

fn independently_derive_certificate(fixture: &QueryFixture) -> RaccCertificate {
    let query = fixture.query.clone();
    let (payload, completeness) = match &query {
        TypedQuery::ReadSpan { start, end, .. } => (
            vec![fixture.source[*start as usize..*end as usize].to_vec()],
            CompletenessWitness::ReadSpan,
        ),
        TypedQuery::ExactSearch { scope, pattern } => {
            let matches = count_matches(fixture.source, pattern);
            (
                vec![pattern.clone(); matches as usize],
                CompletenessWitness::ExactSearch {
                    scope: scope.clone(),
                    pattern: pattern.clone(),
                    scope_len: fixture.source.len() as u64,
                    match_count: matches,
                },
            )
        }
        TypedQuery::Definition { symbol } => (
            vec![b"fn target".to_vec()],
            CompletenessWitness::Definition {
                symbol: *symbol,
                index_id: "fixture-index".into(),
                index_version: "4".into(),
            },
        ),
        TypedQuery::References { symbol } => (
            vec![b"target()".to_vec(), b"target".to_vec()],
            CompletenessWitness::References {
                symbol: *symbol,
                index_id: "fixture-index".into(),
                index_version: "4".into(),
                match_count: 2,
            },
        ),
        TypedQuery::AstClosure {
            seeds,
            relations,
            radius,
        } => (
            vec![b"1,2,3,4".to_vec()],
            CompletenessWitness::AstClosure {
                seeds: seeds.clone(),
                relations: *relations,
                radius: *radius,
                parser_id: "fixture-parser".into(),
                parser_version: "1".into(),
                visited_nodes: 4,
            },
        ),
        TypedQuery::CallPath { source, target } => (
            vec![b"7>8>9".to_vec()],
            CompletenessWitness::CallPath {
                source: *source,
                target: *target,
                edge_count: 2,
            },
        ),
        TypedQuery::DataflowSlice { sink } => (
            vec![b"1>3>4".to_vec()],
            CompletenessWitness::DataflowSlice {
                sink: *sink,
                visited_nodes: 3,
            },
        ),
        TypedQuery::Diff { old, new } => (
            vec![b"+ target".to_vec()],
            CompletenessWitness::Diff {
                old: old.clone(),
                new: new.clone(),
            },
        ),
        TypedQuery::BuildReceipt { command } => (
            vec![b"exit=0".to_vec()],
            CompletenessWitness::BuildReceipt {
                command: *command,
                exit_code: 0,
                stdout_digest: digest_hex(b"built"),
                stderr_digest: digest_hex(b""),
            },
        ),
        TypedQuery::TestTrace { test } => (
            vec![b"pass".to_vec()],
            CompletenessWitness::TestTrace {
                test: *test,
                exit_code: 0,
                trace_digest: digest_hex(b"trace-13"),
            },
        ),
    };
    RaccCertificate {
        schema_version: 1,
        domain: "zerostack.racc.typed-query.v1".into(),
        query,
        payload,
        provenance: locked_provenance(),
        completeness,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CertificateMutation {
    None,
    OmitPayload,
    ExtraPayload,
    StaleIndex,
    StaleParser,
    StaleOperator,
    WrongDomain,
    WrongQueryParameters,
    WrongWitnessKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiptMutation {
    None,
    ReplayIdentity,
    PhaseArithmetic,
    OmitFailedTrials,
    OmitRetries,
    OmitVerificationCalls,
    OmitRecoveryCalls,
    OmitExpansions,
    OmitFailedExpansions,
    OmitFallbackCharges,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetMutation {
    None,
    Nonnested,
    UnderreportedCost,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResidencyMutation {
    None,
    Corruption,
    SilentMiss,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Charges {
    pub successful_trials: u64,
    pub failed_trials: u64,
    pub retries: u64,
    pub verification_calls: u64,
    pub recovery_calls: u64,
    pub expansions: u64,
    pub failed_expansions: u64,
    pub fallback_charges: u64,
}

impl Charges {
    pub fn total(&self) -> u64 {
        self.successful_trials
            + self.failed_trials
            + self.retries
            + self.verification_calls
            + self.recovery_calls
            + self.expansions
            + self.failed_expansions
            + self.fallback_charges
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PhaseArithmetic {
    pub phase: String,
    pub charges: Charges,
    pub reported_total: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DominanceReceipt {
    pub schema_version: u32,
    pub target_identity: String,
    pub target_digest: String,
    pub phases: Vec<PhaseArithmetic>,
    pub replay_identity: String,
}

pub fn canonical_replay_identity(
    target_identity: &str,
    target_digest: &str,
    phases: &[PhaseArithmetic],
) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(target_identity.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(target_digest.as_bytes());
    for phase in phases {
        bytes.push(0xff);
        bytes.extend_from_slice(phase.phase.as_bytes());
        for value in [
            phase.charges.successful_trials,
            phase.charges.failed_trials,
            phase.charges.retries,
            phase.charges.verification_calls,
            phase.charges.recovery_calls,
            phase.charges.expansions,
            phase.charges.failed_expansions,
            phase.charges.fallback_charges,
            phase.reported_total,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    digest_hex(&bytes)
}

fn expected_phases() -> Vec<PhaseArithmetic> {
    let mut phases = vec![
        PhaseArithmetic {
            phase: "explore".into(),
            charges: Charges {
                successful_trials: 2,
                failed_trials: 1,
                retries: 1,
                verification_calls: 2,
                recovery_calls: 1,
                expansions: 2,
                failed_expansions: 1,
                fallback_charges: 1,
            },
            reported_total: 0,
        },
        PhaseArithmetic {
            phase: "verify".into(),
            charges: Charges {
                successful_trials: 1,
                failed_trials: 1,
                retries: 1,
                verification_calls: 3,
                recovery_calls: 1,
                expansions: 1,
                failed_expansions: 1,
                fallback_charges: 2,
            },
            reported_total: 0,
        },
    ];
    for phase in &mut phases {
        phase.reported_total = phase.charges.total();
    }
    phases
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IrreversibleDecision {
    RawFallback,
    CommittedCompressed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BudgetObservation {
    pub requested: Vec<u64>,
    pub measured_costs: Vec<u64>,
    pub reported_costs: Vec<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InlineObservation {
    pub payload: Vec<u8>,
    pub certificate: RaccCertificate,
    pub round_trips: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredFixture {
    pub id: String,
    pub bytes: Vec<u8>,
    pub metadata: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreLookup {
    Hit {
        bytes: Vec<u8>,
        metadata: BTreeMap<String, String>,
    },
    Miss {
        id: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEffectClass {
    Reversible,
    ApprovalRequired,
    Irreversible,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskAttemptCase {
    PassingVerifier,
    FailingVerifier,
    Irreversible,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskTransactionMutation {
    None,
    MissingCharge,
    MissingReceiptCommit,
    AllowIrreversible,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAttemptDisposition {
    Committed,
    RawRollback,
    RejectedIrreversible,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskAcceptanceReceiptDocument {
    pub schema_version: u32,
    pub task_id: String,
    pub verifier_command_id: u64,
    pub verifier_environment_digest: String,
    pub outcome: String,
    pub exit_code: i32,
    pub expected_artifact_digests: Vec<String>,
    pub observed_artifact_digests: Vec<String>,
    pub journal_id: String,
    pub attempt_cost: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskAttemptObservation {
    pub effect_class: TaskEffectClass,
    pub exit_code: Option<i32>,
    pub expected_artifact_digests: Vec<String>,
    pub observed_artifact_digests: Vec<String>,
    pub journal_id: String,
    pub attempt_cost: u64,
    pub charged_attempt_cost: u64,
    pub receipt: Option<TaskAcceptanceReceiptDocument>,
    pub disposition: TaskAttemptDisposition,
}

pub trait RaccSubstrate {
    fn certified_query(&mut self, fixture: &QueryFixture) -> RaccCertificate;
    fn dominance_receipt(&mut self) -> DominanceReceipt;
    fn irreversible_without_evidence(&mut self) -> IrreversibleDecision;
    fn expansion_budget(&mut self) -> BudgetObservation;
    fn inline_fetch(&mut self, fixture: &QueryFixture) -> InlineObservation;
    fn residency_round_trip(
        &mut self,
        objects: &[StoredFixture],
    ) -> Vec<(StoreLookup, StoreLookup)>;
    fn task_attempt(&mut self, case: TaskAttemptCase) -> TaskAttemptObservation;
}

fn pass(id: RaccGateId, detail: impl Into<String>) -> RaccCheckResult {
    RaccCheckResult {
        id,
        status: CheckStatus::Pass,
        detail: detail.into(),
    }
}
fn fail(id: RaccGateId, detail: impl Into<String>) -> RaccCheckResult {
    RaccCheckResult {
        id,
        status: CheckStatus::Fail,
        detail: detail.into(),
    }
}

pub fn check_cert(substrate: &mut impl RaccSubstrate) -> RaccCheckResult {
    for fixture in immutable_query_fixtures() {
        let actual = substrate.certified_query(&fixture);
        let expected = independently_derive_certificate(&fixture);
        if actual != expected {
            return fail(
                RaccGateId::Cert,
                format!(
                    "independent payload/provenance/completeness derivation rejected {:?}",
                    fixture.query
                ),
            );
        }
    }
    pass(
        RaccGateId::Cert,
        "all ten supported typed query kinds independently derived",
    )
}

pub fn check_receipt(substrate: &mut impl RaccSubstrate) -> RaccCheckResult {
    let receipt = substrate.dominance_receipt();
    let expected = expected_phases();
    if receipt.schema_version != 1
        || receipt.target_identity != "fixture-target"
        || receipt.target_digest != digest_hex(b"fixture-target-v1")
    {
        return fail(RaccGateId::Receipt, "target identity or digest changed");
    }
    if receipt.phases != expected {
        return fail(
            RaccGateId::Receipt,
            "exact phase charge arithmetic or complete accounting mismatch",
        );
    }
    if receipt
        .phases
        .iter()
        .any(|phase| phase.reported_total != phase.charges.total())
    {
        return fail(
            RaccGateId::Receipt,
            "reported phase total does not equal independently summed charges",
        );
    }
    let replay = canonical_replay_identity(
        &receipt.target_identity,
        &receipt.target_digest,
        &receipt.phases,
    );
    if receipt.replay_identity != replay {
        return fail(RaccGateId::Receipt, "replay identity mismatch");
    }
    pass(
        RaccGateId::Receipt,
        "replay identity and every charge dimension recomputed",
    )
}

pub fn check_irreversible_gate(substrate: &mut impl RaccSubstrate) -> RaccCheckResult {
    match substrate.irreversible_without_evidence() {
        IrreversibleDecision::RawFallback => pass(
            RaccGateId::GateIrrev,
            "unproven irreversible effect routed to RawFallback",
        ),
        IrreversibleDecision::CommittedCompressed => {
            fail(RaccGateId::GateIrrev, "irreversible gate was skipped")
        }
    }
}

pub fn check_budget(substrate: &mut impl RaccSubstrate) -> RaccCheckResult {
    let observation = substrate.expansion_budget();
    if observation.requested.is_empty()
        || observation.requested.len() != observation.measured_costs.len()
        || observation.measured_costs != observation.reported_costs
    {
        return fail(
            RaccGateId::Budget,
            "measured and reported expansion cost mismatch",
        );
    }
    if observation
        .requested
        .windows(2)
        .any(|pair| pair[1] != pair[0].saturating_mul(2))
    {
        return fail(
            RaccGateId::Budget,
            "budgets are not nested monotone doublings",
        );
    }
    if observation
        .measured_costs
        .iter()
        .zip(&observation.requested)
        .any(|(cost, budget)| cost > budget)
    {
        return fail(
            RaccGateId::Budget,
            "an expansion exceeded its enclosing budget",
        );
    }
    let cumulative = observation
        .measured_costs
        .iter()
        .try_fold(0_u64, |sum, cost| sum.checked_add(*cost));
    let factor_four = observation
        .requested
        .last()
        .and_then(|last| last.checked_mul(4));
    if cumulative.is_none() || factor_four.is_none() || cumulative.unwrap() > factor_four.unwrap() {
        return fail(RaccGateId::Budget, "independent factor-4 bound failed");
    }
    pass(
        RaccGateId::Budget,
        "nested doubling and factor-4 arithmetic verified",
    )
}

pub fn check_inline(substrate: &mut impl RaccSubstrate) -> RaccCheckResult {
    let fixture = immutable_query_fixtures().remove(0);
    let observation = substrate.inline_fetch(&fixture);
    let expected = independently_derive_certificate(&fixture);
    if observation.round_trips != 1 {
        return fail(RaccGateId::Inline, "certificate required a second fetch");
    }
    if observation.payload != expected.payload.concat() || observation.certificate != expected {
        return fail(
            RaccGateId::Inline,
            "inline payload/certificate pair is not independently valid",
        );
    }
    pass(
        RaccGateId::Inline,
        "payload and certificate arrived in one round trip",
    )
}

pub fn residency_fixtures(n: usize) -> Vec<StoredFixture> {
    (0..n)
        .map(|index| StoredFixture {
            id: format!("resident-{index}"),
            bytes: format!("immutable-object-{index}\0binary").into_bytes(),
            metadata: BTreeMap::from([
                ("content_type".into(), "application/octet-stream".into()),
                ("ordinal".into(), index.to_string()),
            ]),
        })
        .collect()
}

pub fn check_residency(substrate: &mut impl RaccSubstrate) -> RaccCheckResult {
    let objects = residency_fixtures(8);
    let observations = substrate.residency_round_trip(&objects);
    if observations.len() != objects.len() {
        return fail(
            RaccGateId::Residency,
            "substrate omitted a residency observation",
        );
    }
    for (object, (resident, removed)) in objects.iter().zip(observations) {
        match resident {
            StoreLookup::Hit { bytes, metadata }
                if bytes == object.bytes && metadata == object.metadata => {}
            _ => {
                return fail(
                    RaccGateId::Residency,
                    format!(
                        "resident object {} was not byte-identical with metadata",
                        object.id
                    ),
                )
            }
        }
        match removed {
            StoreLookup::Miss { id } if id == object.id => {}
            _ => {
                return fail(
                    RaccGateId::Residency,
                    format!(
                        "guarded removal of {} did not yield a typed miss",
                        object.id
                    ),
                )
            }
        }
    }
    pass(
        RaccGateId::Residency,
        "eight resident objects recovered byte-identically then returned typed misses",
    )
}

fn passing_attempt_valid(attempt: &TaskAttemptObservation) -> bool {
    attempt.effect_class == TaskEffectClass::Reversible
        && attempt.exit_code == Some(0)
        && attempt.disposition == TaskAttemptDisposition::Committed
        && attempt.attempt_cost > 0
        && attempt.charged_attempt_cost == attempt.attempt_cost
}

fn passing_receipt_metadata_valid(receipt: &TaskAcceptanceReceiptDocument) -> bool {
    receipt.schema_version == 1
        && receipt.task_id == "fixture-task"
        && receipt.verifier_command_id == 41
        && receipt.verifier_environment_digest == digest_hex(b"fixture-verifier-env")
        && receipt.outcome == "passed"
        && receipt.exit_code == 0
}

fn passing_receipt_artifacts_valid(
    attempt: &TaskAttemptObservation,
    receipt: &TaskAcceptanceReceiptDocument,
) -> bool {
    receipt.expected_artifact_digests == attempt.expected_artifact_digests
        && receipt.observed_artifact_digests == attempt.observed_artifact_digests
        && receipt.expected_artifact_digests == receipt.observed_artifact_digests
        && receipt.journal_id == attempt.journal_id
        && receipt.attempt_cost == attempt.attempt_cost
}

fn passing_receipt_valid(
    attempt: &TaskAttemptObservation,
    receipt: &TaskAcceptanceReceiptDocument,
) -> bool {
    passing_attempt_valid(attempt)
        && passing_receipt_metadata_valid(receipt)
        && passing_receipt_artifacts_valid(attempt, receipt)
}

fn failing_rollback_valid(attempt: &TaskAttemptObservation) -> bool {
    attempt.exit_code == Some(17)
        && attempt.receipt.is_none()
        && attempt.disposition == TaskAttemptDisposition::RawRollback
        && attempt.attempt_cost > 0
        && attempt.charged_attempt_cost == attempt.attempt_cost
}

fn irreversible_refusal_valid(attempt: &TaskAttemptObservation) -> bool {
    attempt.effect_class == TaskEffectClass::Irreversible
        && attempt.disposition == TaskAttemptDisposition::RejectedIrreversible
        && attempt.receipt.is_none()
        && attempt.exit_code.is_none()
}

pub fn check_task_transaction(substrate: &mut impl RaccSubstrate) -> RaccCheckResult {
    let passing = substrate.task_attempt(TaskAttemptCase::PassingVerifier);
    let Some(receipt) = &passing.receipt else {
        return fail(
            RaccGateId::TaskTransaction,
            "passing commit omitted objective receipt",
        );
    };
    if !passing_receipt_valid(&passing, receipt) {
        return fail(
            RaccGateId::TaskTransaction,
            "passing objective verifier/receipt/charge contract mismatch",
        );
    }

    let failing = substrate.task_attempt(TaskAttemptCase::FailingVerifier);
    if !failing_rollback_valid(&failing) {
        return fail(
            RaccGateId::TaskTransaction,
            "failing verifier did not rollback to raw with charged attempt",
        );
    }

    let irreversible = substrate.task_attempt(TaskAttemptCase::Irreversible);
    if !irreversible_refusal_valid(&irreversible) {
        return fail(
            RaccGateId::TaskTransaction,
            "irreversible speculation was not typed pre-attempt rejection",
        );
    }
    pass(RaccGateId::TaskTransaction, "passing verifier committed with receipt; failure rolled back charged; irreversible rejected")
}

pub fn run_racc_suite(substrate: &mut impl RaccSubstrate) -> RaccHarnessReport {
    let checks = vec![
        check_cert(substrate),
        check_receipt(substrate),
        check_irreversible_gate(substrate),
        check_budget(substrate),
        check_inline(substrate),
        check_residency(substrate),
        check_task_transaction(substrate),
    ];
    let passed = checks
        .iter()
        .filter(|result| result.status == CheckStatus::Pass)
        .count();
    RaccHarnessReport {
        failed: checks.len() - passed,
        passed,
        checks,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegressionEvidence {
    PoweredPaired {
        powered: bool,
        no_regression: bool,
    },
    T13NoRegret {
        receipt: TaskAcceptanceReceiptDocument,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskReleaseEvidence {
    pub task_id: String,
    pub raw_cost: u64,
    pub compressed_cost: u64,
    pub evidence: RegressionEvidence,
}
impl TaskReleaseEvidence {
    pub fn ratio(&self) -> Option<(u64, u64)> {
        (self.raw_cost != 0).then_some((self.compressed_cost, self.raw_cost))
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseEvidence {
    pub target_identity: String,
    pub target_digest: String,
    pub preregistered_before_evaluation: bool,
    pub tasks: Vec<TaskReleaseEvidence>,
    pub expected_accounting: Charges,
    pub reported_accounting: Charges,
    pub hub_fake_substrate: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseAggregateResult {
    pub status: CheckStatus,
    pub production_release_pass: bool,
    pub task_ratios: Vec<(String, u64, u64)>,
    pub detail: String,
}

fn release_targets_fixed(evidence: &ReleaseEvidence) -> bool {
    evidence.preregistered_before_evaluation
        && !evidence.target_identity.is_empty()
        && evidence.target_digest == digest_hex(evidence.target_identity.as_bytes())
}

fn regression_evidence_valid(evidence: &RegressionEvidence) -> bool {
    match evidence {
        RegressionEvidence::PoweredPaired {
            powered,
            no_regression,
        } => *powered && *no_regression,
        RegressionEvidence::T13NoRegret { receipt } => {
            receipt.schema_version == 1
                && receipt.outcome == "passed"
                && receipt.exit_code == 0
                && receipt.attempt_cost > 0
                && receipt.expected_artifact_digests == receipt.observed_artifact_digests
        }
    }
}

pub fn check_release_aggregate(evidence: &ReleaseEvidence) -> ReleaseAggregateResult {
    let ratios: Vec<_> = evidence
        .tasks
        .iter()
        .filter_map(|task| task.ratio().map(|(c, r)| (task.task_id.clone(), c, r)))
        .collect();
    let targets_fixed = release_targets_fixed(evidence);
    let no_regression = evidence
        .tasks
        .iter()
        .all(|task| regression_evidence_valid(&task.evidence));
    let complete_accounting = evidence.expected_accounting == evidence.reported_accounting;
    let valid = targets_fixed
        && no_regression
        && complete_accounting
        && ratios.len() == evidence.tasks.len()
        && !ratios.is_empty();
    let production_release_pass = valid && !evidence.hub_fake_substrate;
    let detail = if valid && evidence.hub_fake_substrate {
        "hub validation passed; fake green is explicitly not a production release pass"
    } else if valid {
        "paper 12.2 release aggregate passed"
    } else {
        "paper 12.2 release aggregate failed"
    };
    ReleaseAggregateResult {
        status: if valid {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        production_release_pass,
        task_ratios: ratios,
        detail: detail.into(),
    }
}

pub fn digest_hex(bytes: &[u8]) -> String {
    let mut lanes = [
        0xcbf29ce484222325_u64,
        0x84222325cbf29ce4,
        0x9e3779b185ebca87,
        0x517cc1b727220a95,
    ];
    for (index, byte) in bytes.iter().enumerate() {
        let lane_index = index % lanes.len();
        let lane = &mut lanes[lane_index];
        *lane ^= u64::from(*byte);
        *lane = lane.wrapping_mul(0x100000001b3);
    }
    lanes.iter().map(|lane| format!("{lane:016x}")).collect()
}

pub fn validate_racc_schema(schema: &str, document: &Value) -> Result<(), String> {
    let schema: Value = serde_json::from_str(schema).map_err(|error| error.to_string())?;
    let validator = jsonschema::validator_for(&schema).map_err(|error| error.to_string())?;
    validator
        .validate(document)
        .map_err(|error| error.to_string())
}
