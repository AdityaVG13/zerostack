//! Authoritative Program assembly over planner, worker, MCP, lifecycle, and GC evidence.
//!
//! A `Program` is an executed unit of work that spans five independent evidence
//! sources: a **planner** (plan + step count), a **worker** (execution, closure
//! kind, MCP evidence binding, usage), an **MCP** surface (tool and call
//! commitments), a **lifecycle** state machine (open -> prepared -> executing ->
//! closed), and a **garbage collector** (collection after lifecycle closure).
//!
//! Each source emits its own *report*. Reports are kept separate: none of them
//! is a proof, and no report may claim another source's outcome. Truthful
//! aggregation happens only in [`assemble`], which:
//!
//! - requires every one of the five reports exactly once ([`ProgramReports`]
//!   carries `Option`s so a missing source is a real, detectable state);
//! - recomputes every report's self-binding digest from its fields and rejects
//!   mismatches ([`ProgramAssemblyError::MalformedReport`]);
//! - rejects zero-binding ("synthetic") evidence
//!   ([`ProgramAssemblyError::SyntheticReceipt`]);
//! - refuses fallback closure: a worker that fell back cannot yield a proof
//!   ([`ProgramAssemblyError::FallbackReceipt`]);
//! - cross-checks planner / worker / lifecycle step counts and the MCP evidence
//!   commitment carried by the worker report;
//! - requires lifecycle closure before GC evidence counts.
//!
//! The resulting [`ProgramProof`] is an opaque, linear object: fields are
//! private, it is neither `Clone` nor `Deserialize`, and it can only be created
//! by a successful [`assemble`]. `ProgramProof::verify` re-checks the stored
//! commitment without needing the source reports again.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{collections::BTreeSet, fmt};
use zero_abi::EngineIdentity;
use zero_store::{GcRunReceipt, GcRunState, gc_report_digest_hex};

pub const PROGRAM_ASSEMBLY_SCHEMA_VERSION: u16 = 1;
pub const PROGRAM_SOURCE_COUNT: usize = 5;
pub const MAX_PROGRAM_STEPS: u64 = 1_000_000;
pub const MAX_MCP_TOOLS: u64 = 4_096;
pub const MAX_MCP_CALLS: u64 = 65_536;
pub const MAX_GC_OBJECTS: u64 = 1_000_000;

/// Domain-separated digest used by every Program assembly commitment.
pub type ProgramDigest = [u8; 32];

const PLANNER_DOMAIN: &[u8] = b"zerostack.program.planner.v1\0";
const WORKER_DOMAIN: &[u8] = b"zerostack.program.worker.v1\0";
const MCP_DOMAIN: &[u8] = b"zerostack.program.mcp.v1\0";
const LIFECYCLE_DOMAIN: &[u8] = b"zerostack.program.lifecycle.v1\0";
const GC_DOMAIN: &[u8] = b"zerostack.program.gc.v1\0";
const MCP_EVIDENCE_DOMAIN: &[u8] = b"zerostack.program.worker.mcp-evidence.v1\0";
const PROGRAM_DOMAIN: &[u8] = b"zerostack.program.proof.v1\0";

/// The five independent evidence sources a Program proof aggregates.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum EvidenceSource {
    Planner,
    Worker,
    Mcp,
    Lifecycle,
    Gc,
}

impl EvidenceSource {
    pub const ALL: [EvidenceSource; PROGRAM_SOURCE_COUNT] = [
        EvidenceSource::Planner,
        EvidenceSource::Worker,
        EvidenceSource::Mcp,
        EvidenceSource::Lifecycle,
        EvidenceSource::Gc,
    ];

    pub fn kind(self) -> &'static str {
        match self {
            EvidenceSource::Planner => "planner",
            EvidenceSource::Worker => "worker",
            EvidenceSource::Mcp => "mcp",
            EvidenceSource::Lifecycle => "lifecycle",
            EvidenceSource::Gc => "gc",
        }
    }
}

impl fmt::Display for EvidenceSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.kind())
    }
}

/// Lifecycle states of a Program. Evidence counts only once `Closed` is reached.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum LifecycleState {
    Open,
    Prepared,
    Executing,
    Closed,
}

/// How a worker ended the program: committed (provable) or fell back (not).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum WorkerClosureKind {
    Commit,
    Fallback,
}

/// Bounded worker resource usage, bound into the worker report digest.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramUsage {
    pub cpu_ns: u64,
    pub memory_bytes: u64,
    pub io_bytes: u64,
}

fn hash_bytes(bytes: &[u8]) -> ProgramDigest {
    Sha256::digest(bytes).into()
}

fn append_bounded(target: &mut Vec<u8>, value: &[u8]) {
    target.extend_from_slice(&(value.len() as u64).to_le_bytes());
    target.extend_from_slice(value);
}

fn append_u64(target: &mut Vec<u8>, value: u64) {
    target.extend_from_slice(&value.to_le_bytes());
}

fn append_digest(target: &mut Vec<u8>, value: &ProgramDigest) {
    target.extend_from_slice(value);
}

fn is_zero(digest: &ProgramDigest) -> bool {
    digest.iter().all(|byte| *byte == 0)
}

/// Commitment over the planner's evidence.
pub fn planner_report_digest(
    schema_version: u16,
    program_id: ProgramDigest,
    plan_digest: ProgramDigest,
    step_count: u64,
) -> ProgramDigest {
    let mut bytes = Vec::with_capacity(PLANNER_DOMAIN.len() + 8 + 32 + 32 + 8);
    append_bounded(&mut bytes, PLANNER_DOMAIN);
    append_u64(&mut bytes, u64::from(schema_version));
    append_digest(&mut bytes, &program_id);
    append_digest(&mut bytes, &plan_digest);
    append_u64(&mut bytes, step_count);
    hash_bytes(&bytes)
}

/// Commitment over the worker's evidence, including the MCP evidence binding.
#[allow(clippy::too_many_arguments)]
pub fn worker_report_digest(
    schema_version: u16,
    program_id: ProgramDigest,
    worker_id: ProgramDigest,
    executed_steps: u64,
    closure_kind: WorkerClosureKind,
    mcp_evidence_digest: ProgramDigest,
    effects_digest: ProgramDigest,
    output_digest: ProgramDigest,
    usage: ProgramUsage,
) -> ProgramDigest {
    let mut bytes = Vec::with_capacity(WORKER_DOMAIN.len() + 8 + 32 * 5 + 8 + 1 + 8 * 3);
    append_bounded(&mut bytes, WORKER_DOMAIN);
    append_u64(&mut bytes, u64::from(schema_version));
    append_digest(&mut bytes, &program_id);
    append_digest(&mut bytes, &worker_id);
    append_u64(&mut bytes, executed_steps);
    append_u64(&mut bytes, u64::from(closure_kind as u8));
    append_digest(&mut bytes, &mcp_evidence_digest);
    append_digest(&mut bytes, &effects_digest);
    append_digest(&mut bytes, &output_digest);
    append_u64(&mut bytes, usage.cpu_ns);
    append_u64(&mut bytes, usage.memory_bytes);
    append_u64(&mut bytes, usage.io_bytes);
    hash_bytes(&bytes)
}

/// Commitment over the MCP surface's evidence.
pub fn mcp_report_digest(
    schema_version: u16,
    program_id: ProgramDigest,
    tool_count: u64,
    call_count: u64,
    tools_digest: ProgramDigest,
) -> ProgramDigest {
    let mut bytes = Vec::with_capacity(MCP_DOMAIN.len() + 8 + 32 + 8 + 8 + 32);
    append_bounded(&mut bytes, MCP_DOMAIN);
    append_u64(&mut bytes, u64::from(schema_version));
    append_digest(&mut bytes, &program_id);
    append_u64(&mut bytes, tool_count);
    append_u64(&mut bytes, call_count);
    append_digest(&mut bytes, &tools_digest);
    hash_bytes(&bytes)
}

/// Commitment over the lifecycle state machine's evidence.
pub fn lifecycle_report_digest(
    schema_version: u16,
    program_id: ProgramDigest,
    transition_count: u64,
    executed_step_count: u64,
    final_state: LifecycleState,
) -> ProgramDigest {
    let mut bytes = Vec::with_capacity(LIFECYCLE_DOMAIN.len() + 8 + 32 + 8 + 8 + 1);
    append_bounded(&mut bytes, LIFECYCLE_DOMAIN);
    append_u64(&mut bytes, u64::from(schema_version));
    append_digest(&mut bytes, &program_id);
    append_u64(&mut bytes, transition_count);
    append_u64(&mut bytes, executed_step_count);
    append_u64(&mut bytes, u64::from(final_state as u8));
    hash_bytes(&bytes)
}

/// Commitment over the garbage collector's evidence.
pub fn gc_report_digest(
    schema_version: u16,
    program_id: ProgramDigest,
    collected_objects: u64,
    freed_bytes: u64,
    after_lifecycle_close: bool,
) -> ProgramDigest {
    let mut bytes = Vec::with_capacity(GC_DOMAIN.len() + 8 + 32 + 8 + 8 + 1);
    append_bounded(&mut bytes, GC_DOMAIN);
    append_u64(&mut bytes, u64::from(schema_version));
    append_digest(&mut bytes, &program_id);
    append_u64(&mut bytes, collected_objects);
    append_u64(&mut bytes, freed_bytes);
    append_u64(&mut bytes, u64::from(after_lifecycle_close));
    hash_bytes(&bytes)
}

fn gc_report_digest_with_binding(
    schema_version: u16,
    program_id: ProgramDigest,
    collected_objects: u64,
    freed_bytes: u64,
    after_lifecycle_close: bool,
    binding: Option<&AppliedGcEvidenceV1>,
) -> ProgramDigest {
    let base = gc_report_digest(
        schema_version,
        program_id,
        collected_objects,
        freed_bytes,
        after_lifecycle_close,
    );
    let mut bytes = Vec::new();
    append_bounded(&mut bytes, b"zerostack.program.gc.applied.v1\0");
    append_digest(&mut bytes, &base);
    if let Some(binding) = binding {
        append_bounded(&mut bytes, binding.run_receipt_digest.as_bytes());
        for row in &binding.producer_epochs {
            append_bounded(&mut bytes, row.engine.as_str().as_bytes());
            append_u64(&mut bytes, row.epoch);
        }
        append_u64(&mut bytes, binding.verified_freed_bytes);
    }
    hash_bytes(&bytes)
}

/// Worker-side commitment over the MCP report's fields. This is the binding
/// that makes MCP calls backed by worker evidence: a Program proof requires
/// `worker.mcp_evidence_digest == mcp_evidence_digest(mcp)`.
pub fn mcp_evidence_digest(
    tool_count: u64,
    call_count: u64,
    tools_digest: ProgramDigest,
) -> ProgramDigest {
    let mut bytes = Vec::with_capacity(MCP_EVIDENCE_DOMAIN.len() + 8 + 8 + 32);
    append_bounded(&mut bytes, MCP_EVIDENCE_DOMAIN);
    append_u64(&mut bytes, tool_count);
    append_u64(&mut bytes, call_count);
    append_digest(&mut bytes, &tools_digest);
    hash_bytes(&bytes)
}

/// Commitment over all five source digests, in canonical source order.
pub fn program_digest(
    planner_digest: ProgramDigest,
    worker_digest: ProgramDigest,
    mcp_digest: ProgramDigest,
    lifecycle_digest: ProgramDigest,
    gc_digest: ProgramDigest,
) -> ProgramDigest {
    let mut bytes = Vec::with_capacity(PROGRAM_DOMAIN.len() + 32 * PROGRAM_SOURCE_COUNT);
    append_bounded(&mut bytes, PROGRAM_DOMAIN);
    append_digest(&mut bytes, &planner_digest);
    append_digest(&mut bytes, &worker_digest);
    append_digest(&mut bytes, &mcp_digest);
    append_digest(&mut bytes, &lifecycle_digest);
    append_digest(&mut bytes, &gc_digest);
    hash_bytes(&bytes)
}

/// Planner evidence: the plan commitment and the number of planned steps.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlannerReport {
    schema_version: u16,
    program_id: ProgramDigest,
    plan_digest: ProgramDigest,
    step_count: u64,
    digest: ProgramDigest,
}

impl PlannerReport {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        schema_version: u16,
        program_id: ProgramDigest,
        plan_digest: ProgramDigest,
        step_count: u64,
    ) -> Self {
        let digest = planner_report_digest(schema_version, program_id, plan_digest, step_count);
        Self {
            schema_version,
            program_id,
            plan_digest,
            step_count,
            digest,
        }
    }

    pub fn schema_version(&self) -> u16 {
        self.schema_version
    }
    pub fn program_id(&self) -> ProgramDigest {
        self.program_id
    }
    pub fn plan_digest(&self) -> ProgramDigest {
        self.plan_digest
    }
    pub fn step_count(&self) -> u64 {
        self.step_count
    }
    /// Self-binding commitment over this report's fields, recomputed by assembly.
    pub fn digest(&self) -> ProgramDigest {
        self.digest
    }
}

/// Worker evidence: execution, closure kind, MCP binding, and resource usage.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerReport {
    schema_version: u16,
    program_id: ProgramDigest,
    worker_id: ProgramDigest,
    executed_steps: u64,
    closure_kind: WorkerClosureKind,
    mcp_evidence_digest: ProgramDigest,
    effects_digest: ProgramDigest,
    output_digest: ProgramDigest,
    usage: ProgramUsage,
    digest: ProgramDigest,
}

impl WorkerReport {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        schema_version: u16,
        program_id: ProgramDigest,
        worker_id: ProgramDigest,
        executed_steps: u64,
        closure_kind: WorkerClosureKind,
        mcp_evidence_digest: ProgramDigest,
        effects_digest: ProgramDigest,
        output_digest: ProgramDigest,
        usage: ProgramUsage,
    ) -> Self {
        let digest = worker_report_digest(
            schema_version,
            program_id,
            worker_id,
            executed_steps,
            closure_kind,
            mcp_evidence_digest,
            effects_digest,
            output_digest,
            usage,
        );
        Self {
            schema_version,
            program_id,
            worker_id,
            executed_steps,
            closure_kind,
            mcp_evidence_digest,
            effects_digest,
            output_digest,
            usage,
            digest,
        }
    }

    pub fn schema_version(&self) -> u16 {
        self.schema_version
    }
    pub fn program_id(&self) -> ProgramDigest {
        self.program_id
    }
    pub fn worker_id(&self) -> ProgramDigest {
        self.worker_id
    }
    pub fn executed_steps(&self) -> u64 {
        self.executed_steps
    }
    pub fn closure_kind(&self) -> WorkerClosureKind {
        self.closure_kind
    }
    pub fn mcp_evidence_digest(&self) -> ProgramDigest {
        self.mcp_evidence_digest
    }
    pub fn effects_digest(&self) -> ProgramDigest {
        self.effects_digest
    }
    pub fn output_digest(&self) -> ProgramDigest {
        self.output_digest
    }
    pub fn usage(&self) -> ProgramUsage {
        self.usage
    }
    /// Self-binding commitment over this report's fields, recomputed by assembly.
    pub fn digest(&self) -> ProgramDigest {
        self.digest
    }
}

/// MCP surface evidence: tool and call commitments.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct McpReport {
    schema_version: u16,
    program_id: ProgramDigest,
    tool_count: u64,
    call_count: u64,
    tools_digest: ProgramDigest,
    digest: ProgramDigest,
}

impl McpReport {
    pub fn new(
        schema_version: u16,
        program_id: ProgramDigest,
        tool_count: u64,
        call_count: u64,
        tools_digest: ProgramDigest,
    ) -> Self {
        let digest = mcp_report_digest(
            schema_version,
            program_id,
            tool_count,
            call_count,
            tools_digest,
        );
        Self {
            schema_version,
            program_id,
            tool_count,
            call_count,
            tools_digest,
            digest,
        }
    }

    pub fn schema_version(&self) -> u16 {
        self.schema_version
    }
    pub fn program_id(&self) -> ProgramDigest {
        self.program_id
    }
    pub fn tool_count(&self) -> u64 {
        self.tool_count
    }
    pub fn call_count(&self) -> u64 {
        self.call_count
    }
    pub fn tools_digest(&self) -> ProgramDigest {
        self.tools_digest
    }
    /// Self-binding commitment over this report's fields, recomputed by assembly.
    pub fn digest(&self) -> ProgramDigest {
        self.digest
    }
}

/// Lifecycle evidence: the open -> prepared -> executing -> closed machine.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleReport {
    schema_version: u16,
    program_id: ProgramDigest,
    transition_count: u64,
    executed_step_count: u64,
    final_state: LifecycleState,
    digest: ProgramDigest,
}

impl LifecycleReport {
    pub fn new(
        schema_version: u16,
        program_id: ProgramDigest,
        transition_count: u64,
        executed_step_count: u64,
        final_state: LifecycleState,
    ) -> Self {
        let digest = lifecycle_report_digest(
            schema_version,
            program_id,
            transition_count,
            executed_step_count,
            final_state,
        );
        Self {
            schema_version,
            program_id,
            transition_count,
            executed_step_count,
            final_state,
            digest,
        }
    }

    pub fn schema_version(&self) -> u16 {
        self.schema_version
    }
    pub fn program_id(&self) -> ProgramDigest {
        self.program_id
    }
    pub fn transition_count(&self) -> u64 {
        self.transition_count
    }
    pub fn executed_step_count(&self) -> u64 {
        self.executed_step_count
    }
    pub fn final_state(&self) -> LifecycleState {
        self.final_state
    }
    /// Self-binding commitment over this report's fields, recomputed by assembly.
    pub fn digest(&self) -> ProgramDigest {
        self.digest
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GcProducerEpochV1 {
    pub engine: EngineIdentity,
    pub epoch: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedGcEvidenceV1 {
    pub run_receipt: GcRunReceipt,
    pub run_receipt_digest: String,
    pub producer_epochs: Vec<GcProducerEpochV1>,
    pub verified_freed_bytes: u64,
}

impl AppliedGcEvidenceV1 {
    pub fn new(
        run_receipt: GcRunReceipt,
        mut producer_epochs: Vec<GcProducerEpochV1>,
        verified_freed_bytes: u64,
    ) -> Result<Self, String> {
        producer_epochs.sort_by_key(|row| row.engine);
        let run_receipt_digest =
            gc_report_digest_hex(&run_receipt).map_err(|error| error.to_string())?;
        let evidence = Self {
            run_receipt,
            run_receipt_digest,
            producer_epochs,
            verified_freed_bytes,
        };
        evidence
            .validate()
            .then_some(evidence)
            .ok_or_else(|| "invalid applied GC evidence binding".to_string())
    }

    pub fn validate(&self) -> bool {
        if !self.run_receipt.apply || self.run_receipt.state != GcRunState::Complete {
            return false;
        }
        if gc_report_digest_hex(&self.run_receipt).ok().as_deref()
            != Some(self.run_receipt_digest.as_str())
        {
            return false;
        }
        let engines = self
            .producer_epochs
            .iter()
            .map(|row| row.engine)
            .collect::<BTreeSet<_>>();
        engines
            == BTreeSet::from([
                EngineIdentity::FsZero,
                EngineIdentity::GraphZero,
                EngineIdentity::TokenZero,
            ])
            && self.producer_epochs.len() == 3
            && self.producer_epochs.iter().all(|row| row.epoch > 0)
            && self
                .producer_epochs
                .windows(2)
                .all(|pair| pair[0].engine < pair[1].engine)
            && self
                .run_receipt
                .deleted
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                == self.run_receipt.deleted.len()
    }
}

/// Garbage collector evidence: collection that happened after lifecycle closure.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GcReport {
    schema_version: u16,
    program_id: ProgramDigest,
    collected_objects: u64,
    freed_bytes: u64,
    after_lifecycle_close: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    applied: Option<AppliedGcEvidenceV1>,
    digest: ProgramDigest,
}

impl GcReport {
    pub fn new(
        schema_version: u16,
        program_id: ProgramDigest,
        collected_objects: u64,
        freed_bytes: u64,
        after_lifecycle_close: bool,
    ) -> Self {
        let digest = gc_report_digest_with_binding(
            schema_version,
            program_id,
            collected_objects,
            freed_bytes,
            after_lifecycle_close,
            None,
        );
        Self {
            schema_version,
            program_id,
            collected_objects,
            freed_bytes,
            after_lifecycle_close,
            applied: None,
            digest,
        }
    }

    pub fn new_applied(
        schema_version: u16,
        program_id: ProgramDigest,
        applied: AppliedGcEvidenceV1,
    ) -> Self {
        let collected_objects = applied.run_receipt.deleted.len() as u64;
        let freed_bytes = applied.verified_freed_bytes;
        let after_lifecycle_close = true;
        let digest = gc_report_digest_with_binding(
            schema_version,
            program_id,
            collected_objects,
            freed_bytes,
            after_lifecycle_close,
            Some(&applied),
        );
        Self {
            schema_version,
            program_id,
            collected_objects,
            freed_bytes,
            after_lifecycle_close,
            applied: Some(applied),
            digest,
        }
    }

    pub fn schema_version(&self) -> u16 {
        self.schema_version
    }
    pub fn program_id(&self) -> ProgramDigest {
        self.program_id
    }
    pub fn collected_objects(&self) -> u64 {
        self.collected_objects
    }
    pub fn freed_bytes(&self) -> u64 {
        self.freed_bytes
    }
    pub fn after_lifecycle_close(&self) -> bool {
        self.after_lifecycle_close
    }
    pub fn applied(&self) -> Option<&AppliedGcEvidenceV1> {
        self.applied.as_ref()
    }
    /// Self-binding commitment over this report's fields, recomputed by assembly.
    pub fn digest(&self) -> ProgramDigest {
        self.digest
    }
}

/// Five separately collected reports. Every slot is optional: evidence
/// gathering may fail to produce a source, and assembly must then fail.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProgramReports {
    planner: Option<PlannerReport>,
    worker: Option<WorkerReport>,
    mcp: Option<McpReport>,
    lifecycle: Option<LifecycleReport>,
    gc: Option<GcReport>,
}

impl ProgramReports {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn planner(mut self, report: PlannerReport) -> Self {
        self.planner = Some(report);
        self
    }

    pub fn worker(mut self, report: WorkerReport) -> Self {
        self.worker = Some(report);
        self
    }

    pub fn mcp(mut self, report: McpReport) -> Self {
        self.mcp = Some(report);
        self
    }

    pub fn lifecycle(mut self, report: LifecycleReport) -> Self {
        self.lifecycle = Some(report);
        self
    }

    pub fn gc(mut self, report: GcReport) -> Self {
        self.gc = Some(report);
        self
    }

    pub fn planner_report(&self) -> Option<&PlannerReport> {
        self.planner.as_ref()
    }
    pub fn worker_report(&self) -> Option<&WorkerReport> {
        self.worker.as_ref()
    }
    pub fn mcp_report(&self) -> Option<&McpReport> {
        self.mcp.as_ref()
    }
    pub fn lifecycle_report(&self) -> Option<&LifecycleReport> {
        self.lifecycle.as_ref()
    }
    pub fn gc_report(&self) -> Option<&GcReport> {
        self.gc.as_ref()
    }

    /// Truthful aggregation. See the module documentation for the checks.
    pub fn assemble(self) -> Result<ProgramProof, ProgramAssemblyError> {
        assemble(self)
    }
}

/// Why a Program cannot be proven. Assembly fails closed: the first violated
/// invariant determines the error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgramAssemblyError {
    MissingReport(EvidenceSource),
    SchemaVersionMismatch,
    ProgramIdMismatch,
    MalformedReport(EvidenceSource),
    SyntheticReceipt(EvidenceSource),
    StepCountMismatch,
    LifecycleTransitionMismatch,
    FallbackReceipt,
    McpEvidenceMismatch,
    LifecycleNotClosed,
    GcBeforeLifecycleClose,
    BoundsExceeded(EvidenceSource),
    ProgramDigestMismatch,
}

impl ProgramAssemblyError {
    pub fn kind(&self) -> &'static str {
        match self {
            ProgramAssemblyError::MissingReport(source) => match source {
                EvidenceSource::Planner => "missing_report_planner",
                EvidenceSource::Worker => "missing_report_worker",
                EvidenceSource::Mcp => "missing_report_mcp",
                EvidenceSource::Lifecycle => "missing_report_lifecycle",
                EvidenceSource::Gc => "missing_report_gc",
            },
            ProgramAssemblyError::SchemaVersionMismatch => "schema_version_mismatch",
            ProgramAssemblyError::ProgramIdMismatch => "program_id_mismatch",
            ProgramAssemblyError::MalformedReport(source) => match source {
                EvidenceSource::Planner => "malformed_report_planner",
                EvidenceSource::Worker => "malformed_report_worker",
                EvidenceSource::Mcp => "malformed_report_mcp",
                EvidenceSource::Lifecycle => "malformed_report_lifecycle",
                EvidenceSource::Gc => "malformed_report_gc",
            },
            ProgramAssemblyError::SyntheticReceipt(source) => match source {
                EvidenceSource::Planner => "synthetic_receipt_planner",
                EvidenceSource::Worker => "synthetic_receipt_worker",
                EvidenceSource::Mcp => "synthetic_receipt_mcp",
                EvidenceSource::Lifecycle => "synthetic_receipt_lifecycle",
                EvidenceSource::Gc => "synthetic_receipt_gc",
            },
            ProgramAssemblyError::StepCountMismatch => "step_count_mismatch",
            ProgramAssemblyError::LifecycleTransitionMismatch => "lifecycle_transition_mismatch",
            ProgramAssemblyError::FallbackReceipt => "fallback_receipt",
            ProgramAssemblyError::McpEvidenceMismatch => "mcp_evidence_mismatch",
            ProgramAssemblyError::LifecycleNotClosed => "lifecycle_not_closed",
            ProgramAssemblyError::GcBeforeLifecycleClose => "gc_before_lifecycle_close",
            ProgramAssemblyError::BoundsExceeded(source) => match source {
                EvidenceSource::Planner => "bounds_exceeded_planner",
                EvidenceSource::Worker => "bounds_exceeded_worker",
                EvidenceSource::Mcp => "bounds_exceeded_mcp",
                EvidenceSource::Lifecycle => "bounds_exceeded_lifecycle",
                EvidenceSource::Gc => "bounds_exceeded_gc",
            },
            ProgramAssemblyError::ProgramDigestMismatch => "program_digest_mismatch",
        }
    }
}

impl fmt::Display for ProgramAssemblyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.kind())
    }
}

impl std::error::Error for ProgramAssemblyError {}

/// Authoritative Program proof. Opaque and linear: no `Clone`, no
/// `Deserialize`, private fields, and construction only via [`assemble`].
#[derive(Debug)]
pub struct ProgramProof {
    program_id: ProgramDigest,
    program_digest: ProgramDigest,
    step_count: u64,
    tool_count: u64,
    call_count: u64,
    collected_objects: u64,
    freed_bytes: u64,
    planner_digest: ProgramDigest,
    worker_digest: ProgramDigest,
    mcp_digest: ProgramDigest,
    lifecycle_digest: ProgramDigest,
    gc_digest: ProgramDigest,
}

impl ProgramProof {
    pub fn program_id(&self) -> ProgramDigest {
        self.program_id
    }
    pub fn program_digest(&self) -> ProgramDigest {
        self.program_digest
    }
    pub fn step_count(&self) -> u64 {
        self.step_count
    }
    pub fn tool_count(&self) -> u64 {
        self.tool_count
    }
    pub fn call_count(&self) -> u64 {
        self.call_count
    }
    pub fn collected_objects(&self) -> u64 {
        self.collected_objects
    }
    pub fn freed_bytes(&self) -> u64 {
        self.freed_bytes
    }
    pub fn planner_digest(&self) -> ProgramDigest {
        self.planner_digest
    }
    pub fn worker_digest(&self) -> ProgramDigest {
        self.worker_digest
    }
    pub fn mcp_digest(&self) -> ProgramDigest {
        self.mcp_digest
    }
    pub fn lifecycle_digest(&self) -> ProgramDigest {
        self.lifecycle_digest
    }
    pub fn gc_digest(&self) -> ProgramDigest {
        self.gc_digest
    }

    /// Re-checks the stored commitment without the source reports.
    pub fn verify(&self) -> Result<(), ProgramAssemblyError> {
        let recomputed = program_digest(
            self.planner_digest,
            self.worker_digest,
            self.mcp_digest,
            self.lifecycle_digest,
            self.gc_digest,
        );
        if recomputed == self.program_digest {
            Ok(())
        } else {
            Err(ProgramAssemblyError::ProgramDigestMismatch)
        }
    }
}

/// Truthful aggregation of the five separated reports into an authoritative
/// Program proof. Fails closed: no fallback path yields a proof.
pub fn assemble(reports: ProgramReports) -> Result<ProgramProof, ProgramAssemblyError> {
    let planner = reports
        .planner
        .ok_or(ProgramAssemblyError::MissingReport(EvidenceSource::Planner))?;
    let worker = reports
        .worker
        .ok_or(ProgramAssemblyError::MissingReport(EvidenceSource::Worker))?;
    let mcp = reports
        .mcp
        .ok_or(ProgramAssemblyError::MissingReport(EvidenceSource::Mcp))?;
    let lifecycle = reports
        .lifecycle
        .ok_or(ProgramAssemblyError::MissingReport(
            EvidenceSource::Lifecycle,
        ))?;
    let gc = reports
        .gc
        .ok_or(ProgramAssemblyError::MissingReport(EvidenceSource::Gc))?;

    let version = PROGRAM_ASSEMBLY_SCHEMA_VERSION;
    for source in EvidenceSource::ALL {
        let report_version = match source {
            EvidenceSource::Planner => planner.schema_version,
            EvidenceSource::Worker => worker.schema_version,
            EvidenceSource::Mcp => mcp.schema_version,
            EvidenceSource::Lifecycle => lifecycle.schema_version,
            EvidenceSource::Gc => gc.schema_version,
        };
        if report_version != version {
            return Err(ProgramAssemblyError::SchemaVersionMismatch);
        }
    }

    let program_id = planner.program_id;
    for source in EvidenceSource::ALL {
        let report_program_id = match source {
            EvidenceSource::Planner => planner.program_id,
            EvidenceSource::Worker => worker.program_id,
            EvidenceSource::Mcp => mcp.program_id,
            EvidenceSource::Lifecycle => lifecycle.program_id,
            EvidenceSource::Gc => gc.program_id,
        };
        if report_program_id != program_id {
            return Err(ProgramAssemblyError::ProgramIdMismatch);
        }
    }

    // Synthetic evidence: zero bindings or zero work cannot be proven.
    if is_zero(&program_id) {
        return Err(ProgramAssemblyError::SyntheticReceipt(
            EvidenceSource::Planner,
        ));
    }
    if is_zero(&planner.plan_digest) || planner.step_count == 0 {
        return Err(ProgramAssemblyError::SyntheticReceipt(
            EvidenceSource::Planner,
        ));
    }
    if is_zero(&worker.worker_id)
        || is_zero(&worker.mcp_evidence_digest)
        || is_zero(&worker.effects_digest)
        || is_zero(&worker.output_digest)
        || worker.executed_steps == 0
    {
        return Err(ProgramAssemblyError::SyntheticReceipt(
            EvidenceSource::Worker,
        ));
    }
    if is_zero(&mcp.tools_digest) {
        return Err(ProgramAssemblyError::SyntheticReceipt(EvidenceSource::Mcp));
    }
    if lifecycle.executed_step_count == 0 {
        return Err(ProgramAssemblyError::SyntheticReceipt(
            EvidenceSource::Lifecycle,
        ));
    }

    // Self-consistency: every claimed digest must be recomputable from fields.
    if planner.digest
        != planner_report_digest(
            planner.schema_version,
            planner.program_id,
            planner.plan_digest,
            planner.step_count,
        )
    {
        return Err(ProgramAssemblyError::MalformedReport(
            EvidenceSource::Planner,
        ));
    }
    if worker.digest
        != worker_report_digest(
            worker.schema_version,
            worker.program_id,
            worker.worker_id,
            worker.executed_steps,
            worker.closure_kind,
            worker.mcp_evidence_digest,
            worker.effects_digest,
            worker.output_digest,
            worker.usage,
        )
    {
        return Err(ProgramAssemblyError::MalformedReport(
            EvidenceSource::Worker,
        ));
    }
    if mcp.digest
        != mcp_report_digest(
            mcp.schema_version,
            mcp.program_id,
            mcp.tool_count,
            mcp.call_count,
            mcp.tools_digest,
        )
    {
        return Err(ProgramAssemblyError::MalformedReport(EvidenceSource::Mcp));
    }
    if lifecycle.digest
        != lifecycle_report_digest(
            lifecycle.schema_version,
            lifecycle.program_id,
            lifecycle.transition_count,
            lifecycle.executed_step_count,
            lifecycle.final_state,
        )
    {
        return Err(ProgramAssemblyError::MalformedReport(
            EvidenceSource::Lifecycle,
        ));
    }
    if gc.digest
        != gc_report_digest_with_binding(
            gc.schema_version,
            gc.program_id,
            gc.collected_objects,
            gc.freed_bytes,
            gc.after_lifecycle_close,
            gc.applied.as_ref(),
        )
    {
        return Err(ProgramAssemblyError::MalformedReport(EvidenceSource::Gc));
    }

    // Fallback closure never proves.
    if worker.closure_kind != WorkerClosureKind::Commit {
        return Err(ProgramAssemblyError::FallbackReceipt);
    }

    // Cross-source step counts must agree.
    if planner.step_count != worker.executed_steps
        || planner.step_count != lifecycle.executed_step_count
    {
        return Err(ProgramAssemblyError::StepCountMismatch);
    }

    // Lifecycle invariant: open->prepared, prepared->executing, one transition
    // per executed step, then executing->closed.
    if lifecycle.transition_count != lifecycle.executed_step_count + 2 {
        return Err(ProgramAssemblyError::LifecycleTransitionMismatch);
    }
    if lifecycle.final_state != LifecycleState::Closed {
        return Err(ProgramAssemblyError::LifecycleNotClosed);
    }

    // MCP calls must be backed by worker evidence: no synthetic tool receipts.
    if worker.mcp_evidence_digest
        != mcp_evidence_digest(mcp.tool_count, mcp.call_count, mcp.tools_digest)
    {
        return Err(ProgramAssemblyError::McpEvidenceMismatch);
    }

    // GC evidence counts only after lifecycle closure.
    if !gc.after_lifecycle_close {
        return Err(ProgramAssemblyError::GcBeforeLifecycleClose);
    }

    // Bounds.
    if planner.step_count > MAX_PROGRAM_STEPS {
        return Err(ProgramAssemblyError::BoundsExceeded(
            EvidenceSource::Planner,
        ));
    }
    if worker.executed_steps > MAX_PROGRAM_STEPS {
        return Err(ProgramAssemblyError::BoundsExceeded(EvidenceSource::Worker));
    }
    if mcp.tool_count > MAX_MCP_TOOLS {
        return Err(ProgramAssemblyError::BoundsExceeded(EvidenceSource::Mcp));
    }
    if mcp.call_count > MAX_MCP_CALLS {
        return Err(ProgramAssemblyError::BoundsExceeded(EvidenceSource::Mcp));
    }
    if gc.collected_objects > MAX_GC_OBJECTS {
        return Err(ProgramAssemblyError::BoundsExceeded(EvidenceSource::Gc));
    }
    let Some(applied_gc) = gc.applied.as_ref() else {
        return Err(ProgramAssemblyError::MalformedReport(EvidenceSource::Gc));
    };
    if !applied_gc.validate()
        || applied_gc.run_receipt.deleted.len() as u64 != gc.collected_objects
        || applied_gc.verified_freed_bytes != gc.freed_bytes
    {
        return Err(ProgramAssemblyError::MalformedReport(EvidenceSource::Gc));
    }

    let planner_digest = planner.digest;
    let worker_digest = worker.digest;
    let mcp_digest = mcp.digest;
    let lifecycle_digest = lifecycle.digest;
    let gc_digest = gc.digest;
    let proof_digest = program_digest(
        planner_digest,
        worker_digest,
        mcp_digest,
        lifecycle_digest,
        gc_digest,
    );

    Ok(ProgramProof {
        program_id,
        program_digest: proof_digest,
        step_count: planner.step_count,
        tool_count: mcp.tool_count,
        call_count: mcp.call_count,
        collected_objects: gc.collected_objects,
        freed_bytes: gc.freed_bytes,
        planner_digest,
        worker_digest,
        mcp_digest,
        lifecycle_digest,
        gc_digest,
    })
}

#[cfg(test)]
mod tests {
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
}
