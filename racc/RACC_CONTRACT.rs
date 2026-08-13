//! Minimal semantic contract for a proof-carrying RACC backend.
//! This is an interface skeleton, not a production implementation.

#![allow(dead_code)]

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ObjectId(pub [u8; 32]);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Digest(pub [u8; 32]);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct SymbolId(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct NodeId(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct TestId(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct CommandId(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct HistoryId(pub u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpanRef {
    pub object_id: ObjectId,
    pub byte_start: u64,
    pub byte_len: u64,
    pub object_digest: Digest,
    pub span_digest: Digest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RelationMask(pub u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Query {
    ReadSpan(SpanRef),
    ExactSearch { scope: ObjectId, pattern: Vec<u8> },
    Definition { symbol: SymbolId },
    References { symbol: SymbolId },
    AstClosure {
        seeds: Vec<NodeId>,
        relations: RelationMask,
        radius: u32,
    },
    CallPath { source: SymbolId, target: SymbolId },
    DataflowSlice { sink: NodeId },
    Diff { old: ObjectId, new: ObjectId },
    BuildReceipt { command: CommandId },
    TestTrace { test: TestId },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Provenance {
    pub engine_version: String,
    pub source_objects: Vec<ObjectId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompletenessWitness {
    ExactRange,
    ExhaustiveByteSearch { scope_len: u64, matches: u64 },
    ParserIndexClosure {
        parser_version: String,
        relation_count: u64,
    },
    BuildExit { exit_code: i32, stdout_digest: Digest, stderr_digest: Digest },
    TestExit { exit_code: i32, trace_digest: Digest },
    RawFallback,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceCertificate {
    pub query: Query,
    pub spans: Vec<SpanRef>,
    pub payload: Vec<u8>,
    pub provenance: Provenance,
    pub completeness: CompletenessWitness,
    pub input_token_cost: u64,
    pub backend_work_units: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TokenBudget(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NextBudget(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetainedFractionPpm(pub u32);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicySufficiencyWitness {
    pub checker_id: String,
    pub proof_digest: Digest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskAcceptanceReceipt {
    pub verifier_id: String,
    pub result_digest: Digest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecisionGate {
    Certified(PolicySufficiencyWitness),
    TaskVerified(TaskAcceptanceReceipt),
    Expand(NextBudget),
    RawFallback,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationMetadata {
    pub media_type: String,
    pub logical_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewRequest {
    pub task_id: String,
    pub query: Query,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CertifiedView {
    pub rendered: Vec<u8>,
    pub certificates: Vec<EvidenceCertificate>,
    pub gate: DecisionGate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedEvidence(pub EvidenceCertificate);
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawView(pub Vec<u8>);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenLedger {
    pub raw_input_tokens: u64,
    pub racc_input_tokens: u64,
    pub model_output_tokens: u64,
    pub model_calls: u64,
    pub fallback_tokens: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DominanceReceipt {
    pub ledger: TokenLedger,
    pub target_retained_ppm: RetainedFractionPpm,
    pub archive_root: Digest,
    pub certificate_root: Digest,
    pub byte_exact: bool,
    pub policy_exact_or_fallback: bool,
    pub task_verified: bool,
}

impl DominanceReceipt {
    /// Pure arithmetic part of the ex-post phase certificate.
    pub fn meets_token_target(&self) -> bool {
        let lhs = u128::from(self.ledger.racc_input_tokens) * 1_000_000u128;
        let rhs = u128::from(self.ledger.raw_input_tokens)
            * u128::from(self.target_retained_ppm.0);
        lhs <= rhs
    }

    pub fn exact_phase_valid(&self) -> bool {
        self.byte_exact
            && self.policy_exact_or_fallback
            && self.task_verified
            && self.meets_token_target()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RaccError {
    MissingObject,
    InvalidRange,
    QueryUnsupported,
    BudgetExceeded,
    InternalInvariant,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerificationError {
    DigestMismatch,
    Incomplete,
    UnsupportedWitness,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReceiptError {
    IncompleteLedger,
    ExactnessUnproven,
}

pub trait RaccBackend {
    fn ingest(
        &mut self,
        bytes: &[u8],
        metadata: ObservationMetadata,
    ) -> Result<ObjectId, RaccError>;

    fn propose_view(
        &mut self,
        request: ViewRequest,
        budget: TokenBudget,
    ) -> Result<CertifiedView, RaccError>;

    fn expand(&self, query: Query) -> Result<EvidenceCertificate, RaccError>;

    fn verify(
        &self,
        certificate: &EvidenceCertificate,
    ) -> Result<VerifiedEvidence, VerificationError>;

    fn raw_fallback(&self, history: HistoryId) -> Result<RawView, RaccError>;

    fn finalize_receipt(
        &self,
        target: RetainedFractionPpm,
    ) -> Result<DominanceReceipt, ReceiptError>;
}

#[cfg(test)]
#[path = "../tests/racc/RACC_CONTRACT_tests.rs"]
mod tests;
