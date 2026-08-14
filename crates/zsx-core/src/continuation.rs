//! V6-R2: session-level continuation persist/resume runtime
//! (ZS-ADAPTER-004, ZS-SESSION-001/005).
//!
//! When an execution aborts at an uncovered semantic decision point, the
//! harness can persist the typed decision payload as a continuation record
//! through [`ContinuationRegistryV1`] and later resume the plan with the
//! model's choice supplied. The record binds:
//!
//! - the opaque [`zero_abi::ContinuationHandleV1`] (a self-verifying handle:
//!   its id is recomputed from its bound roots on every use, so a forged or
//!   tampered handle is rejected before any mutation),
//! - the full `DecisionRequiredV1` payload (question, choices, observation
//!   class, observed value),
//! - the bound execution identity (generation + request id, also the handle
//!   epoch / session generation),
//! - the plan source that a resume re-executes,
//! - the session project root (the only authority root the session boundary
//!   can prove; the other seven roots are left unbound as `DigestV1::ZERO`
//!   rather than fabricated -- the same honesty law as the V6 envelope),
//! - an expiry deadline.
//!
//! The registry is durable through the zero-store session WAL journal
//! surface ([`zero_store::SessionWal`]) under the session state root, so a
//! restarted process replays the registry from disk and can resume a stored
//! handle without retransmitting evidence or replaying model history.
//!
//! Fail-closed resume law ([`ContinuationRegistryV1::consume`]): a resume is
//! refused loudly when the handle is unknown, not a scoped continuation
//! handle, bound to a revoked epoch (a different session generation) or a
//! different project scope, the record or handle was tampered (record digest
//! or self-verifying id mismatch), the record expired, the supplied choice
//! is not among the recorded choices, or the handle was already consumed.
//! A handle is single-use: it is durably consumed before the resumed
//! execution runs, so a replayed resume of the same handle is refused even
//! when the resumed plan settles with a failure.
//!
//! The registry journal is append-only with caller-owned compaction: frames
//! are `Persist` (one typed record) and `Consumed` (one tombstone). Replay
//! applies frames oldest-to-newest; a frame that cannot be parsed fails the
//! registry open loudly (corrupt journal), while a parseable but tampered
//! record is loaded and refused at consume time.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use zero_abi::{
    ContingentPolicyRuleV1, ContingentPolicyV1, ContinuationHandleV1, ContinuationRootsV1,
    DecisionRequiredV1, DigestV1, ObservedMatchV1, ROOTED_ABI_VERSION_V6, sha256,
};
use zero_store::{AppendOutcome, SessionWal, SessionWalConfig};

/// Schema version of the persisted continuation record and registry frames.
pub const CONTINUATION_REGISTRY_SCHEMA_VERSION_V1: u16 = 1;
/// Snapshot file name of the continuation registry WAL journal (the active
/// WAL sibling is `<snapshot>.wal`).
pub const CONTINUATION_REGISTRY_WAL_SNAPSHOT: &str = "continuations.snapshot";
/// Segment budget of the registry journal. Large enough that a plan source
/// of ordinary size always fits one frame; the journal refuses loudly (never
/// silently drops) when a frame or the journal exceeds it.
const CONTINUATION_REGISTRY_SEGMENT_LIMIT: u64 = 16 * 1024 * 1024;

/// Stable key of one continuation record: the bound execution identity and
/// the decision point id, exactly the components of the scoped handle
/// `zsx://g<generation>-r<request_id>/<decision_id>`.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ContinuationKeyV1 {
    pub generation: u64,
    pub request_id: u64,
    pub decision_id: String,
}

impl ContinuationKeyV1 {
    pub fn new(generation: u64, request_id: u64, decision_id: impl Into<String>) -> Self {
        Self {
            generation,
            request_id,
            decision_id: decision_id.into(),
        }
    }
}

/// Fail-closed refusal of the continuation registry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContinuationRegistryErrorV1 {
    /// The handle string is not a scoped continuation handle.
    InvalidHandle(String),
    /// No pending record exists for the handle.
    UnknownHandle,
    /// The persisted record does not match its recorded digest.
    TamperedRecord,
    /// The handle id does not recompute from its own fields.
    TamperedHandle,
    /// The handle is bound to a session generation that is no longer active.
    RevokedEpoch { expected: u64, actual: u64 },
    /// The handle roots belong to a different project scope.
    CrossProjectScope,
    /// The record was persisted for an earlier request of the same
    /// execution identity and decision point.
    DuplicatePersist,
    /// The record expired before the resume.
    Expired { expires_at_unix_ms: u64, now_unix_ms: u64 },
    /// The same handle was already consumed by an earlier resume.
    AlreadyConsumed,
    /// The supplied decision is not among the recorded choices.
    DecisionNotOffered { decision: String },
    /// The record or journal frame does not fit the journal budget.
    JournalFull { detail: String },
    /// A journal frame could not be interpreted.
    CorruptRegistry(String),
    /// The underlying journal surface failed.
    Io(String),
}

impl fmt::Display for ContinuationRegistryErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHandle(detail) => write!(formatter, "invalid continuation handle: {detail}"),
            Self::UnknownHandle => write!(formatter, "unknown continuation handle"),
            Self::TamperedRecord => write!(formatter, "continuation record was tampered"),
            Self::TamperedHandle => write!(formatter, "continuation handle id is forged or tampered"),
            Self::RevokedEpoch { expected, actual } => write!(
                formatter,
                "continuation handle epoch {actual} was revoked (active epoch {expected})"
            ),
            Self::CrossProjectScope => {
                write!(formatter, "continuation handle belongs to a different project scope")
            }
            Self::DuplicatePersist => write!(
                formatter,
                "a continuation for this execution identity and decision point already exists"
            ),
            Self::Expired { expires_at_unix_ms, now_unix_ms } => write!(
                formatter,
                "continuation expired at {expires_at_unix_ms}ms (now {now_unix_ms}ms)"
            ),
            Self::AlreadyConsumed => write!(formatter, "continuation handle was already consumed"),
            Self::DecisionNotOffered { decision } => write!(
                formatter,
                "decision {decision:?} is not among the recorded choices"
            ),
            Self::JournalFull { detail } => write!(formatter, "continuation journal is full: {detail}"),
            Self::CorruptRegistry(detail) => write!(formatter, "corrupt continuation journal: {detail}"),
            Self::Io(detail) => write!(formatter, "continuation journal I/O failure: {detail}"),
        }
    }
}

impl std::error::Error for ContinuationRegistryErrorV1 {}

/// One persisted continuation record (the typed record of ZS-ADAPTER-004).
///
/// `record_digest` is the sha256 of the canonical JSON of every other field,
/// so any tamper with the record (source, decision, expiry, identity, ...)
/// is detected at consume time even when the handle itself is untouched.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationRecordV1 {
    pub schema_version: u16,
    pub handle: ContinuationHandleV1,
    pub decision: DecisionRequiredV1,
    pub generation: u64,
    pub request_id: u64,
    /// The plan source a resume re-executes with the decision supplied.
    pub source: String,
    /// The authorized session root captured at persist time.
    pub project_root: String,
    pub expires_at_unix_ms: u64,
    pub record_digest: [u8; 32],
}

impl ContinuationRecordV1 {
    fn compute_digest(&self) -> Result<[u8; 32], String> {
        let mut blank = self.clone();
        blank.record_digest = [0u8; 32];
        let bytes = serde_json::to_vec(&blank)
            .map_err(|error| format!("cannot canonicalize continuation record: {error}"))?;
        Ok(sha256(&bytes))
    }

    /// Self-verifying check: schema, recorded digest, and handle id. A
    /// tampered record or forged handle fails here before any resume.
    pub fn verify(&self) -> Result<(), ContinuationRegistryErrorV1> {
        if self.schema_version != CONTINUATION_REGISTRY_SCHEMA_VERSION_V1 {
            return Err(ContinuationRegistryErrorV1::CorruptRegistry(format!(
                "unsupported continuation record schema {}",
                self.schema_version
            )));
        }
        let computed = self.compute_digest().map_err(ContinuationRegistryErrorV1::CorruptRegistry)?;
        if computed != self.record_digest {
            return Err(ContinuationRegistryErrorV1::TamperedRecord);
        }
        self.handle
            .verify_id()
            .map_err(|_| ContinuationRegistryErrorV1::TamperedHandle)
    }
}

/// One append-only journal frame of the registry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContinuationFrameV1 {
    Persist { record: ContinuationRecordV1 },
    Consumed { generation: u64, request_id: u64, decision_id: String },
}

/// Input of [`ContinuationRegistryV1::persist`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContinuationPersistRequestV1 {
    pub generation: u64,
    pub request_id: u64,
    pub decision: DecisionRequiredV1,
    pub source: String,
    pub project_root: String,
    pub expires_at_unix_ms: u64,
}

/// Receipt of one persisted continuation: the scoped handle the model holds
/// (identical to the envelope's `continuation_handle`) plus the opaque
/// self-verifying handle id.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContinuationReceiptV1 {
    pub generation: u64,
    pub request_id: u64,
    pub decision_id: String,
    /// Hex of the opaque [`ContinuationHandleV1`] id bound to the record.
    pub handle_id_hex: String,
    /// The scoped handle `zsx://g<generation>-r<request_id>/<decision_id>`.
    pub continuation_handle: String,
    pub expires_at_unix_ms: u64,
}

/// What a successful [`ContinuationRegistryV1::consume`] returns: the
/// verified record and the one-shot contingent policy that supplies the
/// model's decision to the resumed plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContinuationResumeBindingV1 {
    pub record: ContinuationRecordV1,
    pub policy: ContingentPolicyV1,
    pub continuation_handle: String,
}

/// Session-level continuation registry: pending records and consumed
/// tombstones, durably journaled under the session state root through the
/// zero-store session WAL surface.
#[derive(Debug)]
pub struct ContinuationRegistryV1 {
    wal: SessionWal,
    pending: BTreeMap<ContinuationKeyV1, ContinuationRecordV1>,
    consumed: BTreeSet<ContinuationKeyV1>,
}

impl ContinuationRegistryV1 {
    /// Open the registry under `state_root`, replaying the journal. A frame
    /// that cannot be parsed fails the open loudly; a parseable but tampered
    /// record is loaded and refused at consume time.
    pub fn open(state_root: &Path) -> Result<Self, ContinuationRegistryErrorV1> {
        let snapshot = state_root.join(CONTINUATION_REGISTRY_WAL_SNAPSHOT);
        let wal = SessionWal::new(
            snapshot,
            SessionWalConfig {
                segment_limit: CONTINUATION_REGISTRY_SEGMENT_LIMIT,
                ..SessionWalConfig::default()
            },
        )
        .map_err(|error| ContinuationRegistryErrorV1::Io(error.to_string()))?;
        let replay = wal
            .replay()
            .map_err(|error| ContinuationRegistryErrorV1::Io(error.to_string()))?;
        let mut pending = BTreeMap::new();
        let mut consumed = BTreeSet::new();
        for frame in replay.records {
            let frame: ContinuationFrameV1 = serde_json::from_slice(&frame)
                .map_err(|error| ContinuationRegistryErrorV1::CorruptRegistry(error.to_string()))?;
            match frame {
                ContinuationFrameV1::Persist { record } => {
                    let key = ContinuationKeyV1::new(
                        record.generation,
                        record.request_id,
                        record.decision.decision_id.clone(),
                    );
                    if pending.contains_key(&key) {
                        return Err(ContinuationRegistryErrorV1::CorruptRegistry(format!(
                            "duplicate persist for {:?}",
                            key
                        )));
                    }
                    // Verification is deferred to consume: a tampered record
                    // still occupies its key so the resume refusal is loud
                    // and specific (TamperedRecord), and one bad record never
                    // bricks resumes of unrelated handles.
                    pending.insert(key, record);
                }
                ContinuationFrameV1::Consumed {
                    generation,
                    request_id,
                    decision_id,
                } => {
                    let key = ContinuationKeyV1::new(generation, request_id, decision_id);
                    pending.remove(&key);
                    consumed.insert(key);
                }
            }
        }
        Ok(Self {
            wal,
            pending,
            consumed,
        })
    }

    /// Persist one typed continuation record and journal it durably. The
    /// record binds the self-verifying handle (project root + epoch only:
    /// the roots this boundary can prove), the decision payload, the
    /// execution identity, the plan source, and the expiry.
    pub fn persist(
        &mut self,
        request: &ContinuationPersistRequestV1,
    ) -> Result<ContinuationReceiptV1, ContinuationRegistryErrorV1> {
        if request.generation == 0 {
            return Err(ContinuationRegistryErrorV1::InvalidHandle(
                "generation must be nonzero".into(),
            ));
        }
        if request.project_root.is_empty() {
            return Err(ContinuationRegistryErrorV1::InvalidHandle(
                "project root must be nonempty".into(),
            ));
        }
        validate_payload(&request.decision)?;
        let key = ContinuationKeyV1::new(
            request.generation,
            request.request_id,
            request.decision.decision_id.clone(),
        );
        if self.pending.contains_key(&key) || self.consumed.contains(&key) {
            return Err(ContinuationRegistryErrorV1::DuplicatePersist);
        }
        // Bind the handle over the roots the session boundary can prove:
        // the authorized project root and the epoch (generation). The other
        // authority roots are unbound (zero) rather than fabricated.
        let roots = ContinuationRootsV1::new(
            DigestV1::ZERO,
            DigestV1::from_bytes(sha256(request.project_root.as_bytes())),
            DigestV1::ZERO,
            DigestV1::ZERO,
            DigestV1::ZERO,
            DigestV1::ZERO,
            DigestV1::ZERO,
            request.generation,
        );
        let handle = ContinuationHandleV1::bind(roots)
            .map_err(|error| ContinuationRegistryErrorV1::InvalidHandle(error.to_string()))?;
        let mut record = ContinuationRecordV1 {
            schema_version: CONTINUATION_REGISTRY_SCHEMA_VERSION_V1,
            handle,
            decision: request.decision.clone(),
            generation: request.generation,
            request_id: request.request_id,
            source: request.source.clone(),
            project_root: request.project_root.clone(),
            expires_at_unix_ms: request.expires_at_unix_ms,
            record_digest: [0u8; 32],
        };
        record.record_digest = record
            .compute_digest()
            .map_err(ContinuationRegistryErrorV1::CorruptRegistry)?;
        self.append_frame(&ContinuationFrameV1::Persist {
            record: record.clone(),
        })?;
        self.pending.insert(key.clone(), record);
        Ok(ContinuationReceiptV1 {
            generation: key.generation,
            request_id: key.request_id,
            decision_id: key.decision_id.clone(),
            handle_id_hex: hex_id(&self.pending[&key].handle.handle_id()),
            continuation_handle: scoped_handle(&key),
            expires_at_unix_ms: request.expires_at_unix_ms,
        })
    }

    /// Validate one handle against the session and, on success, durably
    /// consume it (single-use) and return the record plus the one-shot
    /// policy that supplies `decision` (the model's chosen alternative) to
    /// the resumed plan.
    ///
    /// `epoch` is the active session generation and `project_root` the
    /// active session root; `now_unix_ms` drives the expiry check so callers
    /// control the clock.
    pub fn consume(
        &mut self,
        handle: &str,
        decision: &str,
        project_root: &str,
        epoch: u64,
        now_unix_ms: u64,
    ) -> Result<ContinuationResumeBindingV1, ContinuationRegistryErrorV1> {
        let key = parse_scoped_handle(handle)?;
        if key.generation != epoch {
            return Err(ContinuationRegistryErrorV1::RevokedEpoch {
                expected: epoch,
                actual: key.generation,
            });
        }
        let record = match self.pending.get(&key) {
            Some(record) => record,
            None => {
                if self.consumed.contains(&key) {
                    return Err(ContinuationRegistryErrorV1::AlreadyConsumed);
                }
                return Err(ContinuationRegistryErrorV1::UnknownHandle);
            }
        };
        record.verify()?;
        if record.generation != key.generation || record.decision.decision_id != key.decision_id {
            return Err(ContinuationRegistryErrorV1::TamperedRecord);
        }
        record
            .handle
            .validate_against(
                ROOTED_ABI_VERSION_V6,
                DigestV1::from_bytes(sha256(project_root.as_bytes())),
                epoch,
            )
            .map_err(|error| match error {
                zero_abi::ContinuationErrorV1::WrongAbiVersion { .. } => {
                    ContinuationRegistryErrorV1::TamperedHandle
                }
                zero_abi::ContinuationErrorV1::ForgedHandle => {
                    ContinuationRegistryErrorV1::TamperedHandle
                }
                zero_abi::ContinuationErrorV1::CrossProjectScope => {
                    ContinuationRegistryErrorV1::CrossProjectScope
                }
                zero_abi::ContinuationErrorV1::RevokedEpoch { .. } => {
                    ContinuationRegistryErrorV1::RevokedEpoch {
                        expected: epoch,
                        actual: record.handle.roots().epoch,
                    }
                }
                other => ContinuationRegistryErrorV1::InvalidHandle(other.to_string()),
            })?;
        if record.project_root != project_root {
            return Err(ContinuationRegistryErrorV1::CrossProjectScope);
        }
        if now_unix_ms >= record.expires_at_unix_ms {
            return Err(ContinuationRegistryErrorV1::Expired {
                expires_at_unix_ms: record.expires_at_unix_ms,
                now_unix_ms,
            });
        }
        if !record.decision.choices.iter().any(|choice| choice == decision) {
            return Err(ContinuationRegistryErrorV1::DecisionNotOffered {
                decision: decision.to_owned(),
            });
        }
        let rule = ContingentPolicyRuleV1::new(
            record.decision.observation_class.clone(),
            ObservedMatchV1::Exact {
                value: record.decision.observed_value.clone(),
            },
            decision,
        )
        .map_err(|error| {
            ContinuationRegistryErrorV1::CorruptRegistry(format!(
                "recorded decision payload cannot form a resume rule: {error}"
            ))
        })?;
        let policy = ContingentPolicyV1::new(vec![rule]).map_err(|error| {
            ContinuationRegistryErrorV1::CorruptRegistry(format!(
                "recorded decision payload cannot form a resume policy: {error}"
            ))
        })?;
        // End the borrow of `pending` before the durable mutation: the
        // record is fully validated at this point.
        let record = record.clone();
        self.append_frame(&ContinuationFrameV1::Consumed {
            generation: key.generation,
            request_id: key.request_id,
            decision_id: key.decision_id.clone(),
        })?;
        self.pending.remove(&key);
        self.consumed.insert(key.clone());
        Ok(ContinuationResumeBindingV1 {
            record,
            policy,
            continuation_handle: scoped_handle(&key),
        })
    }

    /// Active (pending) record count, for observability and tests.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Consumed (single-use spent) record count, for observability and tests.
    pub fn consumed_count(&self) -> usize {
        self.consumed.len()
    }

    /// Active WAL file path of the registry journal.
    pub fn wal_path(&self) -> std::path::PathBuf {
        self.wal.wal_path()
    }

    fn append_frame(
        &mut self,
        frame: &ContinuationFrameV1,
    ) -> Result<(), ContinuationRegistryErrorV1> {
        let bytes = serde_json::to_vec(frame)
            .map_err(|error| ContinuationRegistryErrorV1::Io(error.to_string()))?;
        match self.wal.append(&bytes) {
            Ok(AppendOutcome::Appended) => Ok(()),
            Ok(AppendOutcome::NeedsCompaction) => {
                Err(ContinuationRegistryErrorV1::JournalFull {
                    detail: "the registry journal reached its segment budget; compaction is required"
                        .into(),
                })
            }
            Err(error) => Err(ContinuationRegistryErrorV1::Io(error.to_string())),
        }
    }
}

/// The scoped handle shape the V6 envelope emits and the resume API accepts:
/// `zsx://g<generation>-r<request_id>/<decision_id>`.
fn scoped_handle(key: &ContinuationKeyV1) -> String {
    format!("zsx://g{}-r{}/{}", key.generation, key.request_id, key.decision_id)
}

/// Parse a scoped handle back into its key. `generation` is the digits after
/// `zsx://g`, `request_id` the digits after `-r`, and the remainder (which
/// may itself contain `/`) is the decision id.
fn parse_scoped_handle(handle: &str) -> Result<ContinuationKeyV1, ContinuationRegistryErrorV1> {
    let invalid = || {
        ContinuationRegistryErrorV1::InvalidHandle(
            "expected zsx://g<generation>-r<request_id>/<decision_id>".into(),
        )
    };
    let rest = handle
        .strip_prefix("zsx://g")
        .ok_or_else(invalid)?;
    let generation_end = rest.find("-r").ok_or_else(invalid)?;
    let generation: u64 = rest[..generation_end]
        .parse()
        .map_err(|_| invalid())?;
    let after_request = &rest[generation_end + 2..];
    let request_end = after_request.find('/').ok_or_else(invalid)?;
    let request_id: u64 = after_request[..request_end]
        .parse()
        .map_err(|_| invalid())?;
    let decision_id = &after_request[request_end + 1..];
    if decision_id.is_empty() {
        return Err(invalid());
    }
    Ok(ContinuationKeyV1::new(generation, request_id, decision_id))
}

fn hex_id(digest: &DigestV1) -> String {
    zero_abi::sha256_hex(digest.as_bytes())
}

fn validate_payload(
    decision: &DecisionRequiredV1,
) -> Result<(), ContinuationRegistryErrorV1> {
    if decision.decision_id.is_empty() {
        return Err(ContinuationRegistryErrorV1::InvalidHandle(
            "decision_id must be nonempty".into(),
        ));
    }
    if decision.question.is_empty() {
        return Err(ContinuationRegistryErrorV1::InvalidHandle(
            "question must be nonempty".into(),
        ));
    }
    if decision.choices.is_empty() {
        return Err(ContinuationRegistryErrorV1::InvalidHandle(
            "choices must be nonempty".into(),
        ));
    }
    if decision.observed_value.is_empty() {
        return Err(ContinuationRegistryErrorV1::InvalidHandle(
            "observed_value must be nonempty".into(),
        ));
    }
    decision
        .observation_class
        .validate()
        .map_err(|error| ContinuationRegistryErrorV1::InvalidHandle(error.to_string()))
}

#[cfg(test)]
#[path = "../../../tests/rust/zsx-core/unit/continuation.rs"]
mod tests;
