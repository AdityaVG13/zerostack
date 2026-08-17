//! Continuation layer (ZS-SESSION-001..005, ZS-ADAPTER-004).
//!
//! An opaque [`ContinuationHandle`] binds the eight authority roots (task
//! contract, project, evidence, candidate, verification, ledger, contracts,
//! epoch) to one state of the D5 continuation machine
//! ([`crate::zero_execute::ContinuationState`]). A handle is a self-verifying
//! root: any tamper, stale root set, cross-project scope, or forged identity
//! changes the handle id, so the fail-closed checks in
//! [`ContinuationHandle::validate_against`] reject it before any mutation.
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
    ObjectClass, ROOTED_ABI_VERSION, canonical_object_bytes, object_root,
};
use crate::zero_execute::ContinuationState;
use crate::Sha256Digest;

pub const CONTINUATION_CONTRACT_VERSION: u16 = 1;

/// Fail-closed error for continuation construction and validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContinuationError {
    InvalidHandle(String),
    ForgedHandle,
    WrongAbiVersion { actual: String },
    ForbiddenTransition { from: ContinuationState, to: ContinuationState },
    IllegalBranch(ContinuationState),
    CompactionNotPermitted { state: ContinuationState },
    CrossProjectScope,
    StaleRoots,
    RevokedEpoch { expected: u64, actual: u64 },
    UnverifiedChild,
}

impl fmt::Display for ContinuationError {
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

impl Error for ContinuationError {}

/// The eight authority roots bound by one continuation handle.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationRoots {
    pub task_contract_root: Sha256Digest,
    pub project_root: Sha256Digest,
    pub evidence_root: Sha256Digest,
    pub candidate_root: Sha256Digest,
    pub verification_root: Sha256Digest,
    pub ledger_root: Sha256Digest,
    pub contracts_root: Sha256Digest,
    pub epoch: u64,
}

impl ContinuationRoots {
    pub fn new(
        task_contract_root: Sha256Digest,
        project_root: Sha256Digest,
        evidence_root: Sha256Digest,
        candidate_root: Sha256Digest,
        verification_root: Sha256Digest,
        ledger_root: Sha256Digest,
        contracts_root: Sha256Digest,
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
pub struct ContinuationHandle {
    handle_version: u16,
    state: ContinuationState,
    roots: ContinuationRoots,
    parent_handle: Option<Sha256Digest>,
    abi_version: String,
    /// The self-verifying root of this handle. Every validation recomputes
    /// it from the fields, so a forged handle is always rejected.
    handle_id: Sha256Digest,
}

impl ContinuationHandle {
    /// Bind the eight roots at an initial state. `Bound` is the canonical
    /// starting state; any other state is rejected.
    pub fn bind(roots: ContinuationRoots) -> Result<Self, ContinuationError> {
        Self::build(ContinuationState::Bound, roots, None)
    }

    fn build(
        state: ContinuationState,
        roots: ContinuationRoots,
        parent_handle: Option<Sha256Digest>,
    ) -> Result<Self, ContinuationError> {
        let handle = Self {
            handle_version: CONTINUATION_CONTRACT_VERSION,
            state,
            roots,
            parent_handle,
            abi_version: ROOTED_ABI_VERSION.to_owned(),
            handle_id: Sha256Digest::ZERO,
        };
        let handle_id = handle.compute_id()?;
        Ok(Self {
            handle_id,
            ..handle
        })
    }

    fn compute_id(&self) -> Result<Sha256Digest, ContinuationError> {
        let value = serde_json::to_value(self)
            .map_err(|error| ContinuationError::InvalidHandle(error.to_string()))?;
        // Strip the recorded id before hashing (the id cannot bind itself).
        let mut object = value
            .as_object()
            .cloned()
            .ok_or_else(|| ContinuationError::InvalidHandle("not an object".into()))?;
        object.remove("handle_id");
        let bytes = canonical_object_bytes(
            ObjectClass::ContinuationHandle,
            ROOTED_ABI_VERSION,
            &Value::Object(object),
        )
        .map_err(|error| ContinuationError::InvalidHandle(error.to_string()))?;
        object_root(
            ObjectClass::ContinuationHandle,
            ROOTED_ABI_VERSION,
            &bytes,
        )
        .map_err(|error| ContinuationError::InvalidHandle(error.to_string()))
    }

    /// Recompute and compare the handle id. A forged or tampered handle fails
    /// here before any mutation is possible.
    pub fn verify_id(&self) -> Result<(), ContinuationError> {
        let computed = self.compute_id()?;
        if computed != self.handle_id {
            return Err(ContinuationError::ForgedHandle);
        }
        Ok(())
    }

    pub fn handle_id(&self) -> Sha256Digest {
        self.handle_id
    }

    pub fn state(&self) -> ContinuationState {
        self.state
    }

    pub fn roots(&self) -> &ContinuationRoots {
        &self.roots
    }

    pub fn parent(&self) -> Option<Sha256Digest> {
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
        expected_project_root: Sha256Digest,
        expected_epoch: u64,
    ) -> Result<(), ContinuationError> {
        if self.abi_version != expected_abi {
            return Err(ContinuationError::WrongAbiVersion {
                actual: self.abi_version.clone(),
            });
        }
        self.verify_id()?;
        if self.roots.project_root != expected_project_root {
            return Err(ContinuationError::CrossProjectScope);
        }
        if self.roots.epoch != expected_epoch {
            return Err(ContinuationError::RevokedEpoch {
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
        to: ContinuationState,
        policy_supplied: bool,
    ) -> Result<Self, ContinuationError> {
        self.verify_id()?;
        if !ContinuationState::allowed_transition(self.state, to, policy_supplied) {
            return Err(ContinuationError::ForbiddenTransition {
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
    pub fn spawn_child(&self, to: ContinuationState) -> Result<Self, ContinuationError> {
        self.verify_id()?;
        if !matches!(self.state, ContinuationState::Resolved) {
            return Err(ContinuationError::IllegalBranch(self.state));
        }
        if !ContinuationState::allowed_transition(self.state, to, false)
            && !matches!(to, ContinuationState::Planned)
        {
            return Err(ContinuationError::ForbiddenTransition {
                from: self.state,
                to,
            });
        }
        Self::build(to, self.roots.clone(), Some(self.handle_id))
    }

    /// A child that reached `Committed` is the verified child; it carries its
    /// parent id so the branch tree can prove which child won.
    pub fn is_verified_child_of(&self, parent: Sha256Digest) -> bool {
        self.state == ContinuationState::Committed
            && self.parent_handle == Some(parent)
    }

    /// Whether compaction is permitted for this handle: a terminal
    /// post-commit state with a sealed snapshot root recorded by the caller.
    pub fn compaction_permitted(&self, sealed_snapshot_root: Sha256Digest) -> bool {
        self.verify_id().is_ok()
            && self.state == ContinuationState::Committed
            && sealed_snapshot_root != Sha256Digest::ZERO
    }

    /// Durable round trip: a serialized handle deserializes to the identical
    /// handle id (the wire form is self-verifying). ABI-incompatible or
    /// tampered wire forms fail closed during deserialization or on
    /// `verify_id`.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ContinuationError> {
        let value = serde_json::to_value(self)
            .map_err(|error| ContinuationError::InvalidHandle(error.to_string()))?;
        canonical_object_bytes(
            ObjectClass::ContinuationHandle,
            ROOTED_ABI_VERSION,
            &value,
        )
        .map_err(|error| ContinuationError::InvalidHandle(error.to_string()))
    }
}

/// One sealed compaction record: a handle id bound to the sealed snapshot
/// root at compaction time. Replaying the record must reproduce the identical
/// authoritative state (same handle id, same sealed root).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationCompactRecord {
    pub record_version: u16,
    pub handle_id: Sha256Digest,
    pub sealed_snapshot_root: Sha256Digest,
    pub abi_version: String,
}

impl ContinuationCompactRecord {
    pub fn seal(
        handle: &ContinuationHandle,
        sealed_snapshot_root: Sha256Digest,
    ) -> Result<Self, ContinuationError> {
        if !handle.compaction_permitted(sealed_snapshot_root) {
            return Err(ContinuationError::CompactionNotPermitted {
                state: handle.state(),
            });
        }
        Ok(Self {
            record_version: CONTINUATION_CONTRACT_VERSION,
            handle_id: handle.handle_id(),
            sealed_snapshot_root,
            abi_version: ROOTED_ABI_VERSION.to_owned(),
        })
    }

    pub fn validate(&self) -> Result<(), ContinuationError> {
        if self.record_version != CONTINUATION_CONTRACT_VERSION {
            return Err(ContinuationError::InvalidHandle(format!(
                "unsupported record version {}",
                self.record_version
            )));
        }
        if self.abi_version != ROOTED_ABI_VERSION {
            return Err(ContinuationError::WrongAbiVersion {
                actual: self.abi_version.clone(),
            });
        }
        if self.sealed_snapshot_root == Sha256Digest::ZERO {
            return Err(ContinuationError::InvalidHandle(
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
        handle: &ContinuationHandle,
    ) -> Result<(), ContinuationError> {
        self.validate()?;
        if handle.handle_id() != self.handle_id {
            return Err(ContinuationError::ForgedHandle);
        }
        if !handle.compaction_permitted(self.sealed_snapshot_root) {
            return Err(ContinuationError::CompactionNotPermitted {
                state: handle.state(),
            });
        }
        Ok(())
    }
}

