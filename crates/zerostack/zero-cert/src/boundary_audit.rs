//! Static module-boundary audit and replayed-authority verification.

use serde::{Deserialize, Serialize};

use zero_abi::{Sha256Digest, canonical_json};

use crate::kernel_runtime::{
    CacheAdmissionRecord, JournalStore, KernelEventJournal, KernelRuntimeError,
};

/// Schema version of the boundary audit report.
pub const BOUNDARY_AUDIT_SCHEMA_VERSION: u16 = 1;
/// Domain tag bound into the audit report digest.
pub const BOUNDARY_AUDIT_DOMAIN: &[u8] = b"zerostack.boundary-audit\0";
/// ABI tag carried by audit artifacts.
pub const BOUNDARY_AUDIT_ABI_VERSION: &str = "zerostack.boundary-audit/1";

/// How an authority artifact is constructed. `PrivateFields` and
/// `GuardedConstructor` surfaces are compile-time sealed; `PublicArtifact` surfaces
/// are serializable records whose authority comes from the verifying gate, never from construction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstructionSurface {
    /// All fields private; no public constructor (compile-time sealed).
    PrivateFields,
    /// Constructed only through a named guarded function after rooted
    /// evidence.
    GuardedConstructor { guard: String },
    /// A serializable record; construction is public, authority is not --
    /// the named verifier refuses records the journal never issued.
    PublicArtifact { verified_by: String },
}

/// One audited authority artifact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoritySurface {
    pub authority_type: String,
    pub construction_surface: ConstructionSurface,
    /// False when role code cannot construct the artifact at all; true only
    /// for `PublicArtifact` records whose *authority* is still guarded.
    pub role_constructible: bool,
}

/// The sealed static audit report. Its digest anchors the registry; the
/// registry is the contract an external reviewer can diff against source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityBoundaryAuditReport {
    pub schema_version: u16,
    pub invariant: String,
    pub entries: Vec<AuthoritySurface>,
    pub abi_version: String,
}

impl AuthorityBoundaryAuditReport {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, KernelRuntimeError> {
        let json = serde_json::to_value(self).map_err(|error| {
            KernelRuntimeError::Io(format!("audit report serialization: {error}"))
        })?;
        Ok(canonical_json(&json).into_bytes())
    }

    pub fn digest(&self) -> Result<Sha256Digest, KernelRuntimeError> {
        let mut tagged = Vec::with_capacity(BOUNDARY_AUDIT_DOMAIN.len() + 128);
        tagged.extend_from_slice(BOUNDARY_AUDIT_DOMAIN);
        tagged.extend_from_slice(&self.canonical_bytes()?);
        Ok(Sha256Digest::from_bytes(zero_abi::sha256(&tagged)))
    }
}

/// The static module-boundary audit registry for the trusted authority artifacts. Every entry names
/// the artifact, its construction surface, and the guard that issues it.
pub fn authority_boundary_audit() -> AuthorityBoundaryAuditReport {
    AuthorityBoundaryAuditReport {
        schema_version: BOUNDARY_AUDIT_SCHEMA_VERSION,
        invariant:
            "planner/model/retriever/cache-optimizer code cannot construct authority objects; \
             only trusted checkers issue short-lived scoped authority after rooted evidence"
                .to_owned(),
        entries: vec![
            AuthoritySurface {
                authority_type: "zero_cert::VerifiedEvidence".to_owned(),
                construction_surface: ConstructionSurface::PrivateFields,
                role_constructible: false,
            },
            AuthoritySurface {
                authority_type: "zero_cert::RootGateSession".to_owned(),
                construction_surface: ConstructionSurface::PrivateFields,
                role_constructible: false,
            },
            AuthoritySurface {
                authority_type: "zero_cert::CacheAdmissionRecord".to_owned(),
                construction_surface: ConstructionSurface::PublicArtifact {
                    verified_by: "CacheAdmissionGate::decide + journal CacheDecision event"
                        .to_owned(),
                },
                role_constructible: true,
            },
            AuthoritySurface {
                authority_type: "zero_abi::SuccessorRecord".to_owned(),
                construction_surface: ConstructionSurface::PublicArtifact {
                    verified_by: "ProjectRootGate::commit + journal Commit event".to_owned(),
                },
                role_constructible: true,
            },
            AuthoritySurface {
                authority_type: "zero_cert::WorkerAdmissionReceipt".to_owned(),
                construction_surface: ConstructionSurface::PublicArtifact {
                    verified_by: "WorkerTrustBoundary::admit (digest-pinned context)".to_owned(),
                },
                role_constructible: true,
            },
        ],
        abi_version: BOUNDARY_AUDIT_ABI_VERSION.to_owned(),
    }
}

/// Loud read-side authority failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoundaryAuditError {
    /// A decision record was presented that the journal never issued: the
    /// record root does not match any journaled CacheDecision payload.
    UnauthorizedDecision { record_root: String },
    /// A successor record was presented that the journal never committed:
    /// its new root is not the journal's last Commit payload.
    UnauthorizedCommit {
        claimed_new_root: String,
        journal_root: Option<String>,
    },
}

impl std::fmt::Display for BoundaryAuditError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoundaryAuditError::UnauthorizedDecision { record_root } => {
                write!(
                    formatter,
                    "decision record {record_root} was never issued by the journal"
                )
            }
            BoundaryAuditError::UnauthorizedCommit {
                claimed_new_root,
                journal_root,
            } => write!(
                formatter,
                "successor {claimed_new_root} was never committed (journal root {journal_root:?})"
            ),
        }
    }
}

impl std::error::Error for BoundaryAuditError {}

/// Read-side authority check for cache decisions: a `CacheAdmissionRecord` carries authority only
/// when a `CacheDecision` journal event carries its exact record root. A record constructed by
/// role code (same fields, never journaled) is refused -- construction is public, authority is not.
pub fn verify_decision_authority<S: JournalStore>(
    journal: &KernelEventJournal<S>,
    record: &CacheAdmissionRecord,
) -> Result<(), BoundaryAuditError> {
    let record_root = record.record_root();
    for event in journal.records() {
        if event.payload_root == record_root {
            return Ok(());
        }
    }
    Err(BoundaryAuditError::UnauthorizedDecision { record_root })
}

/// Read-side authority check for commits: a presented successor claim (captured `SuccessorRecord`)
/// carries authority only when the journal's last `Commit` event carries the exact new root.
pub fn verify_commit_authority<S: JournalStore>(
    journal: &KernelEventJournal<S>,
    claimed_new_root: Sha256Digest,
) -> Result<(), BoundaryAuditError> {
    match journal.current_project_root() {
        Ok(Some(journal_root)) if journal_root == claimed_new_root => Ok(()),
        Ok(journal_root) => Err(BoundaryAuditError::UnauthorizedCommit {
            claimed_new_root: claimed_new_root.to_hex(),
            journal_root: journal_root.map(|root| root.to_hex()),
        }),
        Err(error) => Err(BoundaryAuditError::UnauthorizedCommit {
            claimed_new_root: claimed_new_root.to_hex(),
            journal_root: Some(format!("journal read failed: {error}")),
        }),
    }
}
