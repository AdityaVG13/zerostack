//! Phase 7 InvariantCatalog + verification contract.
//!
//! Weak evidence cannot look like a pass. `Pending` is not `Satisfied`.
//! `hash = "TODO"` is invalid. Missing files are `fail-missing-evidence`.
//! Only `(ContractStatus::Pass, BaseGate::Allowed)` may close.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::parity_taxonomy::FeatureId;

pub const CATALOG_SCHEMA_VERSION: &str = "tokenzero.invariant-catalog.v1";
pub const VERIFICATION_CONTRACT_SCHEMA: &str = "tokenzero.verification-contract.v1";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct InvariantId(pub String);

impl InvariantId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InvariantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofKind {
    OracleDifferential,
    MetamorphicProperty,
    ProptestInvariant,
    CrashBoundary,
    EProcess,
    FuzzNonPanic,
    InstaSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofStatus {
    Pending,
    Satisfied,
    Failing,
    Stale,
    Missing,
}

impl ProofStatus {
    /// Only Satisfied is Met. Pending must never round up to a pass.
    pub fn is_met(self) -> bool {
        matches!(self, Self::Satisfied)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRef {
    pub path: PathBuf,
    pub hash: String,
    pub schema_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofObligation {
    pub kind: ProofKind,
    pub evidence_ref: ArtifactRef,
    pub status: ProofStatus,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParityInvariant {
    pub invariant_id: InvariantId,
    pub statement: String,
    pub assumptions: Vec<String>,
    pub linked_feature_ids: Vec<FeatureId>,
    pub proof_obligations: Vec<ProofObligation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogViolation {
    EmptyHash(String),
    TodoHash(String),
    HashNotHex(String),
    AbsolutePath(String),
    PathEscape(String),
    TargetDirPath(String),
    MissingFile {
        invariant_id: String,
        path: String,
    },
    HashMismatch {
        invariant_id: String,
        expected: String,
        actual: String,
    },
    SchemaVersionEmpty(String),
    NoObligations(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractStatus {
    Pass,
    FailMissingEvidence,
    FailInvalidReferences,
    FailMixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseGate {
    Allowed,
    BlockedByBaseGate,
    BlockedByContract,
    BlockedByBoth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseDecision {
    Close,
    Block {
        reason: &'static str,
        contract: ContractStatus,
        gate: BaseGate,
    },
}

#[derive(Debug, Clone)]
pub struct InvariantCatalog {
    pub schema_version: String,
    invariants: Vec<ParityInvariant>,
}

impl InvariantCatalog {
    pub fn new(invariants: Vec<ParityInvariant>) -> Self {
        Self {
            schema_version: CATALOG_SCHEMA_VERSION.to_string(),
            invariants,
        }
    }

    pub fn invariants(&self) -> &[ParityInvariant] {
        &self.invariants
    }

    /// TokenZero Phase 7 catalog. Pattern 65 subprocess abort is Satisfied
    /// by env-armed `crash_inject` tests in crash_windows.rs.
    pub fn tokenzero_phase7() -> Self {
        Self::new(vec![
            invariant(
                "INV-TZ-NW-001",
                "z.compress never emits a wrapper that costs more than raw under the kernel tokenizer; clamp must not turn a worse wrapper into omitted_tokens savings.",
                &["F-TZ-021"],
                ProofKind::MetamorphicProperty,
                "tests/unit/tokenzero-engine/phase4_output_contract_goldens.rs",
                "tokenzero.phase4-goldens.v1",
                ProofStatus::Satisfied,
            ),
            invariant(
                "INV-TZ-PULSE-001",
                "Pulse spent = visible + recovery. visible<raw with recovery=0 is not task_lossless. spent>raw is a negative savings ratio, never a clamped 0% save.",
                &["F-TZ-001-EST"],
                ProofKind::ProptestInvariant,
                "tests/unit/tokenzero-pulse/tokenizer_id_grammar.rs",
                "tokenzero.pulse-grammar.v1",
                ProofStatus::Satisfied,
            ),
            invariant(
                "INV-TZ-TOK-002",
                "Unlabeled estimate: tokenizer ids and Q99-as-exact fail closed in the kernel/Pulse preflight. estimator:<slug> remains the labeled approximate class.",
                &["F-TZ-001-EST"],
                ProofKind::ProptestInvariant,
                "tests/unit/tokenzero-core/model_artifact_limits.rs",
                "tokenzero.tokenizer-id-preflight.v1",
                ProofStatus::Satisfied,
            ),
            invariant(
                "INV-TZ-EXP-001",
                "Expand of a persisted blob ref returns the original stored bytes (WAL replay included).",
                &["F-TZ-002-RT"],
                ProofKind::CrashBoundary,
                "tests/unit/tokenzero-recovery/crash_windows.rs",
                "tokenzero.crash-windows.v1",
                ProofStatus::Satisfied,
            ),
            invariant(
                "INV-TZ-DUR-001",
                "Recovery persist publishes via SessionWal, not the retired in-crate atomic_write_json snapshot helper. WAL replay IO errors fail closed so persist cannot clear_wal after a silent snapshot-only load.",
                &["F-TZ-013"],
                ProofKind::CrashBoundary,
                "crates/tokenzero/tokenzero-recovery/src/lib.rs",
                "tokenzero.recovery-wal.v1",
                ProofStatus::Satisfied,
            ),
            invariant(
                "INV-TZ-CLI-001",
                "Every clap-visible subcommand is listed in capabilities.commands or experimental_commands.",
                &["F-TZ-016"],
                ProofKind::OracleDifferential,
                "tests/cli/cli_help_contract.rs",
                "tokenzero.cli-help-contract.v1",
                ProofStatus::Satisfied,
            ),
            invariant(
                "INV-TZ-CRASH-065",
                "Subprocess abort injection at persist/WAL windows (Pattern 65). TOKENZERO_ARM_CRASH_BOUNDARY aborts the child; recovery is committed-or-not, never a torn middle.",
                &["F-TZ-013"],
                ProofKind::CrashBoundary,
                "tests/unit/tokenzero-recovery/crash_windows.rs",
                "tokenzero.crash-subprocess.v1",
                ProofStatus::Satisfied,
            ),
        ])
    }

    pub fn validate(&self, repo_root: &Path) -> Vec<CatalogViolation> {
        let mut out = Vec::new();
        for inv in &self.invariants {
            if inv.proof_obligations.is_empty() {
                out.push(CatalogViolation::NoObligations(inv.invariant_id.0.clone()));
                continue;
            }
            for obl in &inv.proof_obligations {
                out.extend(validate_obligation(repo_root, &inv.invariant_id, obl));
            }
        }
        out
    }

    pub fn contract_status(&self, repo_root: &Path) -> ContractStatus {
        let mut missing = false;
        let mut invalid = false;
        for inv in &self.invariants {
            if inv.proof_obligations.is_empty() {
                missing = true;
                continue;
            }
            for obl in &inv.proof_obligations {
                if !obl.status.is_met() {
                    missing = true;
                    continue;
                }
                for v in validate_obligation(repo_root, &inv.invariant_id, obl) {
                    match v {
                        CatalogViolation::MissingFile { .. } => missing = true,
                        _ => invalid = true,
                    }
                }
            }
        }
        match (missing, invalid) {
            (false, false) => ContractStatus::Pass,
            (true, false) => ContractStatus::FailMissingEvidence,
            (false, true) => ContractStatus::FailInvalidReferences,
            (true, true) => ContractStatus::FailMixed,
        }
    }
}

/// Only the top-left cell closes. Pending evidence is not a pass.
pub fn close_decision(contract: ContractStatus, gate: BaseGate) -> CloseDecision {
    match (contract, gate) {
        (ContractStatus::Pass, BaseGate::Allowed) => CloseDecision::Close,
        (ContractStatus::Pass, BaseGate::BlockedByBaseGate)
        | (ContractStatus::Pass, BaseGate::BlockedByBoth) => CloseDecision::Block {
            reason: "base-gate",
            contract,
            gate: BaseGate::BlockedByBaseGate,
        },
        (ContractStatus::Pass, BaseGate::BlockedByContract) => CloseDecision::Block {
            reason: "inconsistent-gate-column",
            contract,
            gate,
        },
        (ContractStatus::FailMissingEvidence, BaseGate::Allowed)
        | (ContractStatus::FailMissingEvidence, BaseGate::BlockedByContract) => {
            CloseDecision::Block {
                reason: "contract-missing-evidence",
                contract,
                gate: BaseGate::BlockedByContract,
            }
        }
        (ContractStatus::FailInvalidReferences, BaseGate::Allowed)
        | (ContractStatus::FailInvalidReferences, BaseGate::BlockedByContract) => {
            CloseDecision::Block {
                reason: "contract-invalid-references",
                contract,
                gate: BaseGate::BlockedByContract,
            }
        }
        (ContractStatus::FailMixed, BaseGate::Allowed)
        | (ContractStatus::FailMixed, BaseGate::BlockedByContract) => CloseDecision::Block {
            reason: "contract-mixed",
            contract,
            gate: BaseGate::BlockedByContract,
        },
        (_, BaseGate::BlockedByBaseGate) => CloseDecision::Block {
            reason: "both",
            contract,
            gate: BaseGate::BlockedByBoth,
        },
        (_, BaseGate::BlockedByBoth) => CloseDecision::Block {
            reason: "both",
            contract,
            gate: BaseGate::BlockedByBoth,
        },
    }
}

fn invariant(
    id: &str,
    statement: &str,
    features: &[&str],
    kind: ProofKind,
    path: &str,
    schema: &str,
    status: ProofStatus,
) -> ParityInvariant {
    ParityInvariant {
        invariant_id: InvariantId(id.to_string()),
        statement: statement.to_string(),
        assumptions: Vec::new(),
        linked_feature_ids: features
            .iter()
            .map(|f| FeatureId((*f).to_string()))
            .collect(),
        proof_obligations: vec![ProofObligation {
            kind,
            evidence_ref: ArtifactRef {
                path: PathBuf::from(path),
                hash: String::new(),
                schema_version: schema.to_string(),
            },
            status,
            notes: None,
        }],
    }
}

fn validate_obligation(
    repo_root: &Path,
    invariant_id: &InvariantId,
    obl: &ProofObligation,
) -> Vec<CatalogViolation> {
    let mut out = Vec::new();
    let id = invariant_id.0.as_str();
    let refer = &obl.evidence_ref;
    if refer.schema_version.trim().is_empty() {
        out.push(CatalogViolation::SchemaVersionEmpty(id.to_string()));
    }
    if refer.path.is_absolute() {
        out.push(CatalogViolation::AbsolutePath(id.to_string()));
        return out;
    }
    if path_escapes(&refer.path) {
        out.push(CatalogViolation::PathEscape(id.to_string()));
        return out;
    }
    if refer.path.components().any(|c| c.as_os_str() == "target") {
        out.push(CatalogViolation::TargetDirPath(id.to_string()));
    }
    if obl.status.is_met() {
        if refer.hash.is_empty() {
            out.push(CatalogViolation::EmptyHash(id.to_string()));
        } else if refer.hash.eq_ignore_ascii_case("TODO")
            || refer.hash.eq_ignore_ascii_case("pending")
        {
            out.push(CatalogViolation::TodoHash(id.to_string()));
        } else if !is_sha256_hex(&refer.hash) {
            out.push(CatalogViolation::HashNotHex(id.to_string()));
        }
        let abs = repo_root.join(&refer.path);
        match std::fs::read(&abs) {
            Err(_) => out.push(CatalogViolation::MissingFile {
                invariant_id: id.to_string(),
                path: refer.path.display().to_string(),
            }),
            Ok(bytes) => {
                let actual = sha256_hex(&bytes);
                if !refer.hash.is_empty() && is_sha256_hex(&refer.hash) && refer.hash != actual {
                    out.push(CatalogViolation::HashMismatch {
                        invariant_id: id.to_string(),
                        expected: refer.hash.clone(),
                        actual,
                    });
                }
            }
        }
    }
    out
}

fn path_escapes(path: &Path) -> bool {
    let mut depth = 0i32;
    for c in path.components() {
        match c {
            Component::ParentDir => depth -= 1,
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => return true,
        }
        if depth < 0 {
            return true;
        }
    }
    false
}

fn is_sha256_hex(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Bind live catalog hashes from disk. Empty hashes on Satisfied become the
/// current file digest; TODO stays TODO so validate still fails.
pub fn seal_satisfied_hashes(catalog: &mut InvariantCatalog, repo_root: &Path) {
    for inv in &mut catalog.invariants {
        for obl in &mut inv.proof_obligations {
            if !obl.status.is_met() {
                continue;
            }
            if !obl.evidence_ref.hash.is_empty() {
                continue;
            }
            let abs = repo_root.join(&obl.evidence_ref.path);
            if let Ok(bytes) = std::fs::read(abs) {
                obl.evidence_ref.hash = sha256_hex(&bytes);
            }
        }
    }
}

pub fn unique_invariant_ids(catalog: &InvariantCatalog) -> bool {
    let mut seen = BTreeSet::new();
    catalog
        .invariants
        .iter()
        .all(|inv| seen.insert(inv.invariant_id.0.as_str()))
}

#[cfg(test)]
#[path = "../../../../tests/tokenzero/unit/tokenzero-test-support/invariant_catalog_tests.rs"]
mod tests;
