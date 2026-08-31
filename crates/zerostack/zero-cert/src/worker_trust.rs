//! Out-of-process worker trust boundary. Workers are untrusted producers.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use zero_abi::{Sha256Digest, canonical_json};

/// Schema version of worker-trust artifacts.
pub const WORKER_TRUST_SCHEMA_VERSION: u16 = 1;
/// Domain tag bound into worker envelope digests.
pub const WORKER_TRUST_ENVELOPE_DOMAIN: &[u8] = b"zerostack.worker-trust.envelope\0";
/// Domain tag bound into refusal record digests.
pub const WORKER_TRUST_REFUSAL_DOMAIN: &[u8] = b"zerostack.worker-trust.refusal\0";
/// Domain tag bound into admission receipt digests.
pub const WORKER_TRUST_ADMISSION_DOMAIN: &[u8] = b"zerostack.worker-trust.admission\0";
/// ABI tag carried by worker-trust artifacts.
pub const WORKER_TRUST_ABI_VERSION: &str = "zerostack.worker-trust/1";

fn now_unix_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn domain_digest(domain: &[u8], bytes: &[u8]) -> Sha256Digest {
    let mut tagged = Vec::with_capacity(domain.len() + bytes.len());
    tagged.extend_from_slice(domain);
    tagged.extend_from_slice(bytes);
    Sha256Digest::from_bytes(zero_abi::sha256(&tagged))
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, WorkerTrustError> {
    let json = serde_json::to_value(value)
        .map_err(|error| WorkerTrustError::InvalidEnvelope(format!("not serializable: {error}")))?;
    Ok(canonical_json(&json).into_bytes())
}

/// The identity a worker claims. Trust is digest-pinned: the boundary only
/// accepts claims that exactly match the pinned context.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerIdentityClaim {
    pub engine: String,
    pub artifact_digest: Sha256Digest,
    pub protocol_digest: Sha256Digest,
}

/// One worker frame. Frames are content-addressed: `payload_digest` must
/// equal sha256 of `payload`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerFrame {
    pub frame_index: u64,
    pub opcode: String,
    pub payload: Vec<u8>,
    pub payload_digest: Sha256Digest,
}

/// One worker trace line. Traces are bounded (stage registry + token
/// budget).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerTrustTrace {
    pub trace_index: u64,
    pub stage: String,
    pub tokens: u64,
    pub root: Sha256Digest,
}

/// The serialized worker output crossing the process boundary. This is the
/// wire form: JSON-canonical, digestable, and the only thing a remote
/// producer controls.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerTrustEnvelope {
    pub schema_version: u16,
    pub seq: u64,
    pub identity: WorkerIdentityClaim,
    pub frames: Vec<WorkerFrame>,
    pub traces: Vec<WorkerTrustTrace>,
    pub abi_version: String,
}

impl WorkerTrustEnvelope {
    pub fn new(
        seq: u64,
        identity: WorkerIdentityClaim,
        frames: Vec<WorkerFrame>,
        traces: Vec<WorkerTrustTrace>,
    ) -> Result<Self, WorkerTrustError> {
        let envelope = Self {
            schema_version: WORKER_TRUST_SCHEMA_VERSION,
            seq,
            identity,
            frames,
            traces,
            abi_version: WORKER_TRUST_ABI_VERSION.to_owned(),
        };
        envelope.validate()?;
        Ok(envelope)
    }

    pub fn validate(&self) -> Result<(), WorkerTrustError> {
        if self.schema_version != WORKER_TRUST_SCHEMA_VERSION {
            return Err(WorkerTrustError::InvalidEnvelope(
                "envelope schema version is not supported".to_owned(),
            ));
        }
        if self.abi_version != WORKER_TRUST_ABI_VERSION {
            return Err(WorkerTrustError::InvalidEnvelope(
                "envelope ABI version is not supported".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, WorkerTrustError> {
        self.validate()?;
        canonical_bytes(self)
    }

    /// Content-derived envelope digest.
    pub fn digest(&self) -> Result<Sha256Digest, WorkerTrustError> {
        Ok(domain_digest(
            WORKER_TRUST_ENVELOPE_DOMAIN,
            &self.canonical_bytes()?,
        ))
    }
}

/// The digest-pinned trust context the boundary checks against. Mirrors the
/// `AssemblyManifest` identity binding from zero-abi: engine + artifact +
/// protocol digests, plus bounded frame/trace budgets.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrustContext {
    pub expected_engine: String,
    pub expected_artifact_digest: Sha256Digest,
    pub expected_protocol_digest: Sha256Digest,
    pub max_frames: u64,
    pub max_traces: u64,
    pub max_trace_tokens: u64,
}

impl TrustContext {
    pub fn new(
        expected_engine: impl Into<String>,
        expected_artifact_digest: Sha256Digest,
        expected_protocol_digest: Sha256Digest,
        max_frames: u64,
        max_traces: u64,
        max_trace_tokens: u64,
    ) -> Result<Self, WorkerTrustError> {
        let context = Self {
            expected_engine: expected_engine.into(),
            expected_artifact_digest,
            expected_protocol_digest,
            max_frames,
            max_traces,
            max_trace_tokens,
        };
        if context.expected_engine.is_empty()
            || context.expected_artifact_digest == Sha256Digest::ZERO
            || context.expected_protocol_digest == Sha256Digest::ZERO
        {
            return Err(WorkerTrustError::InvalidEnvelope(
                "trust context engine and digests must be nonzero".to_owned(),
            ));
        }
        if context.max_frames == 0 || context.max_traces == 0 || context.max_trace_tokens == 0 {
            return Err(WorkerTrustError::InvalidEnvelope(
                "trust context budgets must be nonzero".to_owned(),
            ));
        }
        Ok(context)
    }
}

/// Why the boundary refused a worker envelope.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRefusalReason {
    IdentityMismatch,
    ForgedFrame { frame_index: u64 },
    ReplayedFrame { frame_index: u64 },
    FrameBudgetExceeded,
    ForgedTrace { trace_index: u64 },
    TraceTokenBudgetExceeded,
    ReplayedEnvelope,
}

/// Sealed refusal record: the receipt of a fail-loud refusal. Refusals are
/// never silent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRefusalRecord {
    pub schema_version: u16,
    pub envelope_digest: Sha256Digest,
    pub reason: WorkerRefusalReason,
    pub detail: String,
    pub seq: u64,
    pub refused_at_unix_ns: u64,
    pub abi_version: String,
}

impl WorkerRefusalRecord {
    fn new(
        envelope_digest: Sha256Digest,
        reason: WorkerRefusalReason,
        detail: String,
        seq: u64,
    ) -> Self {
        Self {
            schema_version: WORKER_TRUST_SCHEMA_VERSION,
            envelope_digest,
            reason,
            detail,
            seq,
            refused_at_unix_ns: now_unix_ns(),
            abi_version: WORKER_TRUST_ABI_VERSION.to_owned(),
        }
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, WorkerTrustError> {
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<Sha256Digest, WorkerTrustError> {
        Ok(domain_digest(
            WORKER_TRUST_REFUSAL_DOMAIN,
            &self.canonical_bytes()?,
        ))
    }
}

/// Sealed admission receipt: the envelope passed the boundary. Admission is
/// NOT authority -- cache/commit authority still requires the gates.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerAdmissionReceipt {
    pub schema_version: u16,
    pub envelope_digest: Sha256Digest,
    pub seq: u64,
    pub frames: u64,
    pub traces: u64,
    pub trace_tokens: u64,
    pub admitted_at_unix_ns: u64,
    pub abi_version: String,
}

impl WorkerAdmissionReceipt {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, WorkerTrustError> {
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<Sha256Digest, WorkerTrustError> {
        Ok(domain_digest(
            WORKER_TRUST_ADMISSION_DOMAIN,
            &self.canonical_bytes()?,
        ))
    }
}

/// Loud, fail-closed worker-trust errors. Every refusal carries its sealed
/// record; structural invalidity is a loud error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerTrustError {
    Refused { record: WorkerRefusalRecord },
    InvalidEnvelope(String),
}

impl std::fmt::Display for WorkerTrustError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerTrustError::Refused { record } => {
                write!(
                    formatter,
                    "worker trust refusal {:?}: {}",
                    record.reason, record.detail
                )
            }
            WorkerTrustError::InvalidEnvelope(detail) => {
                write!(formatter, "invalid worker envelope: {detail}")
            }
        }
    }
}

impl std::error::Error for WorkerTrustError {}

/// The out-of-process trust boundary. Stateful: tracks the last accepted
/// envelope sequence so whole-envelope replays are refused.
#[derive(Clone, Debug)]
pub struct WorkerTrustBoundary {
    pub context: TrustContext,
    pub last_accepted_seq: u64,
}

impl WorkerTrustBoundary {
    pub fn new(context: TrustContext) -> Self {
        Self {
            context,
            last_accepted_seq: 0,
        }
    }

    fn refuse(
        &self,
        envelope_digest: Sha256Digest,
        reason: WorkerRefusalReason,
        detail: String,
        seq: u64,
    ) -> WorkerTrustError {
        WorkerTrustError::Refused {
            record: WorkerRefusalRecord::new(envelope_digest, reason, detail, seq),
        }
    }

    /// Admit a worker envelope, or refuse it loudly with a sealed record.
    /// On acceptance the envelope's seq becomes the boundary's last
    /// accepted seq (replays refused thereafter).
    pub fn admit(
        &mut self,
        envelope: &WorkerTrustEnvelope,
    ) -> Result<WorkerAdmissionReceipt, WorkerTrustError> {
        envelope.validate()?;
        let envelope_digest = envelope.digest()?;

        let refusal = |reason: WorkerRefusalReason, detail: String| {
            self.refuse(envelope_digest, reason, detail, envelope.seq)
        };

        // 1. Digest-pinned identity: the claim must match the context
        // exactly. A forged or stolen identity cannot pass.
        if envelope.identity.engine != self.context.expected_engine
            || envelope.identity.artifact_digest != self.context.expected_artifact_digest
            || envelope.identity.protocol_digest != self.context.expected_protocol_digest
        {
            return Err(refusal(
                WorkerRefusalReason::IdentityMismatch,
                format!(
                    "identity claim (engine={}, artifact={}, protocol={}) does not match the pinned context",
                    envelope.identity.engine,
                    envelope.identity.artifact_digest,
                    envelope.identity.protocol_digest
                ),
            ));
        }

        // 2. Envelope ordering: replays of a whole envelope are refused.
        if envelope.seq <= self.last_accepted_seq {
            return Err(refusal(
                WorkerRefusalReason::ReplayedEnvelope,
                format!(
                    "envelope seq {} is not newer than the last accepted seq {}",
                    envelope.seq, self.last_accepted_seq
                ),
            ));
        }

        // 3. Frames: bounded count, content-addressed payloads, unique
        // indices (no forged or replayed frames).
        if envelope.frames.len() as u64 > self.context.max_frames {
            return Err(refusal(
                WorkerRefusalReason::FrameBudgetExceeded,
                format!(
                    "{} frames exceed the bound {}",
                    envelope.frames.len(),
                    self.context.max_frames
                ),
            ));
        }
        let mut seen_frames = std::collections::BTreeSet::new();
        for frame in &envelope.frames {
            if !seen_frames.insert(frame.frame_index) {
                return Err(refusal(
                    WorkerRefusalReason::ReplayedFrame {
                        frame_index: frame.frame_index,
                    },
                    format!("frame index {} appears more than once", frame.frame_index),
                ));
            }
            if frame.opcode.is_empty() {
                return Err(refusal(
                    WorkerRefusalReason::ForgedFrame {
                        frame_index: frame.frame_index,
                    },
                    "frame opcode must be nonempty".to_owned(),
                ));
            }
            let actual = zero_abi::sha256(&frame.payload);
            if Sha256Digest::from_bytes(actual) != frame.payload_digest {
                return Err(refusal(
                    WorkerRefusalReason::ForgedFrame {
                        frame_index: frame.frame_index,
                    },
                    format!(
                        "frame {} payload does not hash to its declared digest",
                        frame.frame_index
                    ),
                ));
            }
        }

        // 4. Traces: bounded count and token budget, well-typed lines.
        if envelope.traces.len() as u64 > self.context.max_traces {
            return Err(refusal(
                WorkerRefusalReason::TraceTokenBudgetExceeded,
                format!(
                    "{} traces exceed the bound {}",
                    envelope.traces.len(),
                    self.context.max_traces
                ),
            ));
        }
        let mut total_tokens: u64 = 0;
        let mut seen_traces = std::collections::BTreeSet::new();
        for trace in &envelope.traces {
            if !seen_traces.insert(trace.trace_index) {
                return Err(refusal(
                    WorkerRefusalReason::ForgedTrace {
                        trace_index: trace.trace_index,
                    },
                    format!("trace index {} appears more than once", trace.trace_index),
                ));
            }
            if trace.stage.is_empty() || trace.root == Sha256Digest::ZERO {
                return Err(refusal(
                    WorkerRefusalReason::ForgedTrace {
                        trace_index: trace.trace_index,
                    },
                    format!(
                        "trace {} must have a stage and a nonzero root",
                        trace.trace_index
                    ),
                ));
            }
            total_tokens = total_tokens.saturating_add(trace.tokens);
        }
        if total_tokens > self.context.max_trace_tokens {
            return Err(refusal(
                WorkerRefusalReason::TraceTokenBudgetExceeded,
                format!(
                    "trace token total {total_tokens} exceeds the bound {}",
                    self.context.max_trace_tokens
                ),
            ));
        }

        self.last_accepted_seq = envelope.seq;
        Ok(WorkerAdmissionReceipt {
            schema_version: WORKER_TRUST_SCHEMA_VERSION,
            envelope_digest,
            seq: envelope.seq,
            frames: envelope.frames.len() as u64,
            traces: envelope.traces.len() as u64,
            trace_tokens: total_tokens,
            admitted_at_unix_ns: now_unix_ns(),
            abi_version: WORKER_TRUST_ABI_VERSION.to_owned(),
        })
    }
}

/// The frozen contract manifest for the worker trust boundary.
pub fn worker_trust_contract() -> serde_json::Value {
    serde_json::json!({
        "schema_version": WORKER_TRUST_SCHEMA_VERSION,
        "boundary": {
            "identity": "digest-pinned engine/artifact/protocol; forged or stolen identity refused",
            "frames": "content-addressed payloads; forged or replayed frames refused; bounded",
            "traces": "well-typed and token-bounded; forged traces refused",
            "envelope_order": "seq monotonic; replayed envelopes refused",
        },
        "refusals": "fail-loud with sealed WorkerRefusalRecord; never silent",
        "authority": "admission is NOT authority; cache authority requires CacheAdmissionGate, commit authority requires ProjectRootGate",
        "abi_version": WORKER_TRUST_ABI_VERSION,
    })
}
