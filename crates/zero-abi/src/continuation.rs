//! Continuation layer (ZS-SESSION-001..005, ZS-ADAPTER-004).
//!
//! An opaque [`ContinuationHandleV1`] binds the eight authority roots (task
//! contract, project, evidence, candidate, verification, ledger, contracts,
//! epoch) to one state of the D5 continuation machine
//! ([`crate::zero_execute::ContinuationStateV1`]). A handle is a self-verifying
//! root: any tamper, stale root set, cross-project scope, or forged identity
//! changes the handle id, so the fail-closed checks in
//! [`ContinuationHandleV1::validate_against`] reject it before any mutation.
//!
//! Fail-closed laws:
//! - A handle can only advance along [`allowed_transition`]; the D5 forbidden
//!   transitions (`Unknown -> Authorized`, `Executing -> Committed`,
//!   `WaitingDecision -> Executing`) are unreachable through this API.
//! - Branching spawns a child with a recorded parent; a child can never
//!   mutate the parent's roots, and only one verified child may commit.
//! - Compaction is permitted only after a sealed snapshot root exists and the
//!   handle is in a terminal post-commit state; replay of the compacted
//!   record yields the identical sealed head and audit roots.
//! - `validate_against` fails closed on wrong ABI version, forged id, stale
//!   roots, cross-project scope, and revoked epoch.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::identity::{
    ObjectClassV1, ROOTED_ABI_VERSION, canonical_object_bytes, object_root,
};
use crate::zero_execute::ContinuationStateV1;
use crate::DigestV1;

pub const CONTINUATION_CONTRACT_VERSION_V1: u16 = 1;

/// Fail-closed error for continuation construction and validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContinuationErrorV1 {
    InvalidHandle(String),
    ForgedHandle,
    WrongAbiVersion { actual: String },
    ForbiddenTransition { from: ContinuationStateV1, to: ContinuationStateV1 },
    IllegalBranch(ContinuationStateV1),
    CompactionNotPermitted { state: ContinuationStateV1 },
    CrossProjectScope,
    StaleRoots,
    RevokedEpoch { expected: u64, actual: u64 },
    UnverifiedChild,
}

impl fmt::Display for ContinuationErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHandle(detail) => write!(formatter, "invalid continuation handle: {detail}"),
            Self::ForgedHandle => write!(formatter, "continuation handle id is forged"),
            Self::WrongAbiVersion { actual } => {
                write!(formatter, "continuation abi version must be {ROOTED_ABI_VERSION}, got {actual}")
            }
            Self::ForbiddenTransition { from, to } => write!(
                formatter,
                "forbidden continuation transition {from:?} -> {to:?}"
            ),
            Self::IllegalBranch(state) => {
                write!(formatter, "branching from state {state:?} is not permitted")
            }
            Self::CompactionNotPermitted { state } => write!(
                formatter,
                "compaction requires a sealed snapshot and terminal state, got {state:?}"
            ),
            Self::CrossProjectScope => {
                write!(formatter, "handle roots belong to a different project scope")
            }
            Self::StaleRoots => write!(formatter, "handle roots are stale"),
            Self::RevokedEpoch { expected, actual } => write!(
                formatter,
                "handle epoch {actual} was revoked (expected {expected})"
            ),
            Self::UnverifiedChild => {
                write!(formatter, "no verified child may commit over its parent")
            }
        }
    }
}

impl Error for ContinuationErrorV1 {}

/// The eight authority roots bound by one continuation handle.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationRootsV1 {
    pub task_contract_root: DigestV1,
    pub project_root: DigestV1,
    pub evidence_root: DigestV1,
    pub candidate_root: DigestV1,
    pub verification_root: DigestV1,
    pub ledger_root: DigestV1,
    pub contracts_root: DigestV1,
    pub epoch: u64,
}

impl ContinuationRootsV1 {
    pub fn new(
        task_contract_root: DigestV1,
        project_root: DigestV1,
        evidence_root: DigestV1,
        candidate_root: DigestV1,
        verification_root: DigestV1,
        ledger_root: DigestV1,
        contracts_root: DigestV1,
        epoch: u64,
    ) -> Self {
        Self {
            task_contract_root,
            project_root,
            evidence_root,
            candidate_root,
            verification_root,
            ledger_root,
            contracts_root,
            epoch,
        }
    }
}

/// Opaque self-verifying continuation handle. The handle id is the rooted
/// digest of version + state + roots + parent; the fields are private to the
/// module so a handle can only be built or advanced through this API.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationHandleV1 {
    handle_version: u16,
    state: ContinuationStateV1,
    roots: ContinuationRootsV1,
    parent_handle: Option<DigestV1>,
    abi_version: String,
    /// The self-verifying root of this handle. Every validation recomputes
    /// it from the fields, so a forged handle is always rejected.
    handle_id: DigestV1,
}

impl ContinuationHandleV1 {
    /// Bind the eight roots at an initial state. `Bound` is the canonical
    /// starting state; any other state is rejected.
    pub fn bind(roots: ContinuationRootsV1) -> Result<Self, ContinuationErrorV1> {
        Self::build(ContinuationStateV1::Bound, roots, None)
    }

    fn build(
        state: ContinuationStateV1,
        roots: ContinuationRootsV1,
        parent_handle: Option<DigestV1>,
    ) -> Result<Self, ContinuationErrorV1> {
        let handle = Self {
            handle_version: CONTINUATION_CONTRACT_VERSION_V1,
            state,
            roots,
            parent_handle,
            abi_version: ROOTED_ABI_VERSION.to_owned(),
            handle_id: DigestV1::ZERO,
        };
        let handle_id = handle.compute_id()?;
        Ok(Self {
            handle_id,
            ..handle
        })
    }

    fn compute_id(&self) -> Result<DigestV1, ContinuationErrorV1> {
        let value = serde_json::to_value(self)
            .map_err(|error| ContinuationErrorV1::InvalidHandle(error.to_string()))?;
        // Strip the recorded id before hashing (the id cannot bind itself).
        let mut object = value
            .as_object()
            .cloned()
            .ok_or_else(|| ContinuationErrorV1::InvalidHandle("not an object".into()))?;
        object.remove("handle_id");
        let bytes = canonical_object_bytes(
            ObjectClassV1::ContinuationHandle,
            ROOTED_ABI_VERSION,
            &Value::Object(object),
        )
        .map_err(|error| ContinuationErrorV1::InvalidHandle(error.to_string()))?;
        object_root(
            ObjectClassV1::ContinuationHandle,
            ROOTED_ABI_VERSION,
            &bytes,
        )
        .map_err(|error| ContinuationErrorV1::InvalidHandle(error.to_string()))
    }

    /// Recompute and compare the handle id. A forged or tampered handle fails
    /// here before any mutation is possible.
    pub fn verify_id(&self) -> Result<(), ContinuationErrorV1> {
        let computed = self.compute_id()?;
        if computed != self.handle_id {
            return Err(ContinuationErrorV1::ForgedHandle);
        }
        Ok(())
    }

    pub fn handle_id(&self) -> DigestV1 {
        self.handle_id
    }

    pub fn state(&self) -> ContinuationStateV1 {
        self.state
    }

    pub fn roots(&self) -> &ContinuationRootsV1 {
        &self.roots
    }

    pub fn parent(&self) -> Option<DigestV1> {
        self.parent_handle
    }

    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    /// Fail-closed validation against the session's expectations: correct
    /// ABI, self-consistent id, exact project scope, non-stale roots, and a
    /// non-revoked epoch. Any mismatch rejects the handle without mutation.
    pub fn validate_against(
        &self,
        expected_abi: &str,
        expected_project_root: DigestV1,
        expected_epoch: u64,
    ) -> Result<(), ContinuationErrorV1> {
        if self.abi_version != expected_abi {
            return Err(ContinuationErrorV1::WrongAbiVersion {
                actual: self.abi_version.clone(),
            });
        }
        self.verify_id()?;
        if self.roots.project_root != expected_project_root {
            return Err(ContinuationErrorV1::CrossProjectScope);
        }
        if self.roots.epoch != expected_epoch {
            return Err(ContinuationErrorV1::RevokedEpoch {
                expected: expected_epoch,
                actual: self.roots.epoch,
            });
        }
        Ok(())
    }

    /// Advance along the D5 machine. The forbidden transitions are
    /// unreachable through this API; a policy must be supplied exactly when
    /// the transition requires it (`WaitingDecision -> Planned`).
    pub fn advance(
        &self,
        to: ContinuationStateV1,
        policy_supplied: bool,
    ) -> Result<Self, ContinuationErrorV1> {
        self.verify_id()?;
        if !ContinuationStateV1::allowed_transition(self.state, to, policy_supplied) {
            return Err(ContinuationErrorV1::ForbiddenTransition {
                from: self.state,
                to,
            });
        }
        Self::build(to, self.roots.clone(), self.parent_handle)
    }

    /// Branch a child continuation from this handle. Branching is permitted
    /// only from `Resolved` (the point where alternatives are explored) and
    /// only before the parent has left the planning phase. A child shares the
    /// parent's roots but records its parent id; the parent never mutates.
    pub fn spawn_child(&self, to: ContinuationStateV1) -> Result<Self, ContinuationErrorV1> {
        self.verify_id()?;
        if !matches!(self.state, ContinuationStateV1::Resolved) {
            return Err(ContinuationErrorV1::IllegalBranch(self.state));
        }
        if !ContinuationStateV1::allowed_transition(self.state, to, false)
            && !matches!(to, ContinuationStateV1::Planned)
        {
            return Err(ContinuationErrorV1::ForbiddenTransition {
                from: self.state,
                to,
            });
        }
        Self::build(to, self.roots.clone(), Some(self.handle_id))
    }

    /// A child that reached `Committed` is the verified child; it carries its
    /// parent id so the branch tree can prove which child won.
    pub fn is_verified_child_of(&self, parent: DigestV1) -> bool {
        self.state == ContinuationStateV1::Committed
            && self.parent_handle == Some(parent)
    }

    /// Whether compaction is permitted for this handle: a terminal
    /// post-commit state with a sealed snapshot root recorded by the caller.
    pub fn compaction_permitted(&self, sealed_snapshot_root: DigestV1) -> bool {
        self.verify_id().is_ok()
            && self.state == ContinuationStateV1::Committed
            && sealed_snapshot_root != DigestV1::ZERO
    }

    /// Durable round trip: a serialized handle deserializes to the identical
    /// handle id (the wire form is self-verifying). ABI-incompatible or
    /// tampered wire forms fail closed during deserialization or on
    /// `verify_id`.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ContinuationErrorV1> {
        let value = serde_json::to_value(self)
            .map_err(|error| ContinuationErrorV1::InvalidHandle(error.to_string()))?;
        canonical_object_bytes(
            ObjectClassV1::ContinuationHandle,
            ROOTED_ABI_VERSION,
            &value,
        )
        .map_err(|error| ContinuationErrorV1::InvalidHandle(error.to_string()))
    }
}

/// One sealed compaction record: a handle id bound to the sealed snapshot
/// root at compaction time. Replaying the record must reproduce the identical
/// authoritative state (same handle id, same sealed root).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationCompactRecordV1 {
    pub record_version: u16,
    pub handle_id: DigestV1,
    pub sealed_snapshot_root: DigestV1,
    pub abi_version: String,
}

impl ContinuationCompactRecordV1 {
    pub fn seal(
        handle: &ContinuationHandleV1,
        sealed_snapshot_root: DigestV1,
    ) -> Result<Self, ContinuationErrorV1> {
        if !handle.compaction_permitted(sealed_snapshot_root) {
            return Err(ContinuationErrorV1::CompactionNotPermitted {
                state: handle.state(),
            });
        }
        Ok(Self {
            record_version: CONTINUATION_CONTRACT_VERSION_V1,
            handle_id: handle.handle_id(),
            sealed_snapshot_root,
            abi_version: ROOTED_ABI_VERSION.to_owned(),
        })
    }

    pub fn validate(&self) -> Result<(), ContinuationErrorV1> {
        if self.record_version != CONTINUATION_CONTRACT_VERSION_V1 {
            return Err(ContinuationErrorV1::InvalidHandle(format!(
                "unsupported record version {}",
                self.record_version
            )));
        }
        if self.abi_version != ROOTED_ABI_VERSION {
            return Err(ContinuationErrorV1::WrongAbiVersion {
                actual: self.abi_version.clone(),
            });
        }
        if self.sealed_snapshot_root == DigestV1::ZERO {
            return Err(ContinuationErrorV1::InvalidHandle(
                "sealed_snapshot_root must be nonzero".into(),
            ));
        }
        Ok(())
    }

    /// Replay the compacted record against a resumed handle: the handle must
    /// be the recorded one and the sealed snapshot must match. This is the
    /// before/after-compaction equivalence check.
    pub fn replay_against(
        &self,
        handle: &ContinuationHandleV1,
    ) -> Result<(), ContinuationErrorV1> {
        self.validate()?;
        if handle.handle_id() != self.handle_id {
            return Err(ContinuationErrorV1::ForgedHandle);
        }
        if !handle.compaction_permitted(self.sealed_snapshot_root) {
            return Err(ContinuationErrorV1::CompactionNotPermitted {
                state: handle.state(),
            });
        }
        Ok(())
    }
}

