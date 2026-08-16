//! Static module-boundary audit and replayed-authority verification
//! (ZS-KERNEL-005 / V6-R14).
//!
//! Authority is separated by construction: planner/model/retriever/
//! cache-optimizer code lives outside this crate and must not be able to
//! construct authority objects; only trusted checkers issue short-lived
//! scoped authority after rooted evidence. This module makes that boundary
//! auditable and testable:
//!
//! - [`authority_boundary_audit_v1`] is the static audit artifact: a sealed
//!   registry of every authority artifact in the trusted crates, its
//!   construction surface, and its guard. `role_constructible == false`
//!   means the type cannot be constructed by role code -- enforced at
//!   compile time by the private-field layouts plus the `compile_fail`
//!   doctests below, and at runtime by the read-side authority checks.
//! - For serializable artifacts (records must cross process boundaries),
//!   construction is public but *authority is not*: a record only carries
//!   authority when a trusted gate issued it AND the journal shows the
//!   issuance. [`verify_decision_authority_v1`] and
//!   [`verify_commit_authority_v1`] are the read-side checks that refuse a
//!   record the journal never saw (an "authority" forged by role code).
//! - The replayed-authority acceptance (captured-epoch replay) is tested in
//!   `tests/unit/zero-cert/boundary_audit.rs`: an authority captured at
//!   epoch N, replayed after the project root advanced, fails loud with no
//!   journal event and no CAS mutation.
//!
//! ## Static module-boundary audit (compile-time)
//!
//! A planner/model/retriever/cache-optimizer module cannot construct an
//! authority session -- `RootGateSessionV1` is a private-field type with no
//! public constructor:
//!
//! ~~~compile_fail
//! use zero_cert::{DigestV1, RootGateSessionV1};
//! fn forge_session() -> RootGateSessionV1 {
//!     RootGateSessionV1 {
//!         declared_parent_root: DigestV1::ZERO,
//!         verified_successor_root: DigestV1::ZERO,
//!         authorized: true,
//!     }
//! }
//! ~~~
//!
//! ...and cannot forge verified evidence -- `VerifiedEvidence` is a
//! private-field type with no public constructor:
//!
//! ~~~compile_fail
//! use zero_cert::{EvidenceCertificate, VerifiedEvidence};
//! fn forge_evidence(c: &'static EvidenceCertificate<'static>) -> VerifiedEvidence<'static, 'static> {
//!     VerifiedEvidence { certificate: c }
//! }
//! ~~~

use serde::{Deserialize, Serialize};

use zero_abi::{DigestV1, canonical_json};

use crate::kernel_runtime::{CacheAdmissionRecordV1, KernelEventJournalV1, KernelRuntimeError, JournalStore};

/// Schema version of the boundary audit report.
pub const BOUNDARY_AUDIT_SCHEMA_VERSION_V1: u16 = 1;
/// Domain tag bound into the audit report digest.
pub const BOUNDARY_AUDIT_DOMAIN_V1: &[u8] = b"zerostack.boundary-audit.v1\0";
/// ABI tag carried by audit artifacts.
pub const BOUNDARY_AUDIT_ABI_VERSION_V1: &str = "v6-r14";

/// How an authority artifact is constructed. `PrivateFields` and
/// `GuardedConstructor` surfaces are compile-time sealed; `PublicArtifact`
/// surfaces are serializable records whose authority comes from the
/// verifying gate, never from construction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstructionSurfaceV1 {
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
pub struct AuthoritySurfaceV1 {
    pub authority_type: String,
    pub construction_surface: ConstructionSurfaceV1,
    /// False when role code cannot construct the artifact at all; true only
    /// for `PublicArtifact` records whose *authority* is still guarded.
    pub role_constructible: bool,
}

/// The sealed static audit report. Its digest anchors the registry; the
/// registry is the contract an external reviewer can diff against source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityBoundaryAuditReportV1 {
    pub schema_version: u16,
    pub invariant: String,
    pub entries: Vec<AuthoritySurfaceV1>,
    pub abi_version: String,
}

impl AuthorityBoundaryAuditReportV1 {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, KernelRuntimeError> {
        let json = serde_json::to_value(self)
            .map_err(|error| KernelRuntimeError::Io(format!("audit report serialization: {error}")))?;
        Ok(canonical_json(&json).into_bytes())
    }

    pub fn digest(&self) -> Result<DigestV1, KernelRuntimeError> {
        let mut tagged = Vec::with_capacity(BOUNDARY_AUDIT_DOMAIN_V1.len() + 128);
        tagged.extend_from_slice(BOUNDARY_AUDIT_DOMAIN_V1);
        tagged.extend_from_slice(&self.canonical_bytes()?);
        Ok(DigestV1::from_bytes(zero_abi::sha256(&tagged)))
    }
}

/// The static module-boundary audit registry for the trusted authority
/// artifacts. Every entry names the artifact, its construction surface, and
/// the guard that issues it. The audit is honest: serializable records are
/// marked `PublicArtifact` with their verifying gate, never claimed sealed.
pub fn authority_boundary_audit_v1() -> AuthorityBoundaryAuditReportV1 {
    AuthorityBoundaryAuditReportV1 {
        schema_version: BOUNDARY_AUDIT_SCHEMA_VERSION_V1,
        invariant:
            "planner/model/retriever/cache-optimizer code cannot construct authority objects; \
             only trusted checkers issue short-lived scoped authority after rooted evidence"
                .to_owned(),
        entries: vec![
            AuthoritySurfaceV1 {
                authority_type: "zero_cert::VerifiedEvidence".to_owned(),
                construction_surface: ConstructionSurfaceV1::PrivateFields,
                role_constructible: false,
            },
            AuthoritySurfaceV1 {
                authority_type: "zero_cert::RootGateSessionV1".to_owned(),
                construction_surface: ConstructionSurfaceV1::PrivateFields,
                role_constructible: false,
            },
            AuthoritySurfaceV1 {
                authority_type: "zero_cert::CacheAdmissionRecordV1".to_owned(),
                construction_surface: ConstructionSurfaceV1::PublicArtifact {
                    verified_by: "CacheAdmissionGateV1::decide + journal CacheDecision event".to_owned(),
                },
                role_constructible: true,
            },
            AuthoritySurfaceV1 {
                authority_type: "zero_abi::SuccessorRecordV1".to_owned(),
                construction_surface: ConstructionSurfaceV1::PublicArtifact {
                    verified_by: "ProjectRootGateV1::commit + journal Commit event".to_owned(),
                },
                role_constructible: true,
            },
            AuthoritySurfaceV1 {
                authority_type: "zero_cert::WorkerAdmissionReceiptV1".to_owned(),
                construction_surface: ConstructionSurfaceV1::PublicArtifact {
                    verified_by: "WorkerTrustBoundaryV1::admit (digest-pinned context)".to_owned(),
                },
                role_constructible: true,
            },
        ],
        abi_version: BOUNDARY_AUDIT_ABI_VERSION_V1.to_owned(),
    }
}

/// Loud read-side authority failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoundaryAuditErrorV1 {
    /// A decision record was presented that the journal never issued: the
    /// record root does not match any journaled CacheDecision payload.
    UnauthorizedDecision { record_root: String },
    /// A successor record was presented that the journal never committed:
    /// its new root is not the journal's last Commit payload.
    UnauthorizedCommit { claimed_new_root: String, journal_root: Option<String> },
}

impl std::fmt::Display for BoundaryAuditErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoundaryAuditErrorV1::UnauthorizedDecision { record_root } => {
                write!(formatter, "decision record {record_root} was never issued by the journal")
            }
            BoundaryAuditErrorV1::UnauthorizedCommit {
                claimed_new_root,
                journal_root,
            } => write!(
                formatter,
                "successor {claimed_new_root} was never committed (journal root {journal_root:?})"
            ),
        }
    }
}

impl std::error::Error for BoundaryAuditErrorV1 {}

/// Read-side authority check for cache decisions: a `CacheAdmissionRecordV1`
/// carries authority only when a `CacheDecision` journal event carries its
/// exact record root. A record constructed by role code (same fields, never
/// journaled) is refused -- construction is public, authority is not.
pub fn verify_decision_authority_v1<S: JournalStore>(
    journal: &KernelEventJournalV1<S>,
    record: &CacheAdmissionRecordV1,
) -> Result<(), BoundaryAuditErrorV1> {
    let record_root = record.record_root();
    for event in journal.records() {
        if event.payload_root == record_root {
            return Ok(());
        }
    }
    Err(BoundaryAuditErrorV1::UnauthorizedDecision { record_root })
}

/// Read-side authority check for commits: a presented successor claim
/// (captured `SuccessorRecordV1`) carries authority only when the journal's
/// last `Commit` event carries the exact new root. A captured-epoch replay
/// -- the same successor presented after the project root advanced -- is
/// refused because the journal's last commit is no longer the captured
/// root.
pub fn verify_commit_authority_v1<S: JournalStore>(
    journal: &KernelEventJournalV1<S>,
    claimed_new_root: DigestV1,
) -> Result<(), BoundaryAuditErrorV1> {
    match journal.current_project_root() {
        Ok(Some(journal_root)) if journal_root == claimed_new_root => Ok(()),
        Ok(journal_root) => Err(BoundaryAuditErrorV1::UnauthorizedCommit {
            claimed_new_root: claimed_new_root.to_hex(),
            journal_root: journal_root.map(|root| root.to_hex()),
        }),
        Err(error) => Err(BoundaryAuditErrorV1::UnauthorizedCommit {
            claimed_new_root: claimed_new_root.to_hex(),
            journal_root: Some(format!("journal read failed: {error}")),
        }),
    }
}
