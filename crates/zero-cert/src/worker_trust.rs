//! Out-of-process worker trust boundary (ZS-OPS-005 / V6-R14).
//!
//! Workers are untrusted producers. Output crosses the process boundary as
//! a serialized [`WorkerEnvelopeV1`] (identity claim + frames + traces) and
//! is admitted -- or refused -- by [`WorkerTrustBoundaryV1`] against a
//! digest-pinned [`TrustContextV1`]:
//!
//! - Identity must match the pinned engine/artifact/protocol digests
//!   exactly (forged or stolen identity is refused).
//! - Frames must be content-addressed: every frame's payload must hash to
//!   its declared digest (a forged frame is refused by digest, never by
//!   trust), frame indices must be unique (replayed frames refused), and
//!   frame counts are bounded.
//! - Traces must be well-typed and bounded (forged traces and token-budget
//!   overruns refused).
//! - Envelopes are ordered by `seq`; a replayed envelope is refused.
//!
//! Every refusal is fail-loud with a sealed [`WorkerRefusalRecordV1`] (the
//! receipt), and acceptance yields a sealed [`WorkerAdmissionReceiptV1`].
//! Admission is NOT authority: an admitted envelope still cannot acquire
//! cache or commit authority -- cache admission requires
//! [`CacheAdmissionGateV1`] over a rooted `PayloadFormationReceiptV1`, and
//! commit requires [`ProjectRootGateV1`]'s verify -> authorize -> commit
//! chain. The out-of-process fixture in `tests/rust/zero-cert/worker_trust.rs`
//! proves that forged frames/traces acquire neither.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use zero_abi::{DigestV1, canonical_json};

/// Schema version of worker-trust artifacts.
pub const WORKER_TRUST_SCHEMA_VERSION_V1: u16 = 1;
/// Domain tag bound into worker envelope digests.
pub const WORKER_TRUST_ENVELOPE_DOMAIN_V1: &[u8] = b"zerostack.worker-trust.envelope.v1\0";
/// Domain tag bound into refusal record digests.
pub const WORKER_TRUST_REFUSAL_DOMAIN_V1: &[u8] = b"zerostack.worker-trust.refusal.v1\0";
/// Domain tag bound into admission receipt digests.
pub const WORKER_TRUST_ADMISSION_DOMAIN_V1: &[u8] = b"zerostack.worker-trust.admission.v1\0";
/// ABI tag carried by worker-trust artifacts.
pub const WORKER_TRUST_ABI_VERSION_V1: &str = "v6-r14";

fn now_unix_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
}

fn domain_digest(domain: &[u8], bytes: &[u8]) -> DigestV1 {
    let mut tagged = Vec::with_capacity(domain.len() + bytes.len());
    tagged.extend_from_slice(domain);
    tagged.extend_from_slice(bytes);
    DigestV1::from_bytes(zero_abi::sha256(&tagged))
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, WorkerTrustErrorV1> {
    let json = serde_json::to_value(value)
        .map_err(|error| WorkerTrustErrorV1::InvalidEnvelope(format!("not serializable: {error}")))?;
    Ok(canonical_json(&json).into_bytes())
}

/// The identity a worker claims. Trust is digest-pinned: the boundary only
/// accepts claims that exactly match the pinned context.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerIdentityClaimV1 {
    pub engine: String,
    pub artifact_digest: DigestV1,
    pub protocol_digest: DigestV1,
}

/// One worker frame. Frames are content-addressed: `payload_digest` must
/// equal sha256 of `payload`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerFrameV1 {
    pub frame_index: u64,
    pub opcode: String,
    pub payload: Vec<u8>,
    pub payload_digest: DigestV1,
}

/// One worker trace line. Traces are bounded (stage registry + token
/// budget).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerTraceV1 {
    pub trace_index: u64,
    pub stage: String,
    pub tokens: u64,
    pub root: DigestV1,
}

/// The serialized worker output crossing the process boundary. This is the
/// wire form: JSON-canonical, digestable, and the only thing a remote
/// producer controls.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerEnvelopeV1 {
    pub schema_version: u16,
    pub seq: u64,
    pub identity: WorkerIdentityClaimV1,
    pub frames: Vec<WorkerFrameV1>,
    pub traces: Vec<WorkerTraceV1>,
    pub abi_version: String,
}

impl WorkerEnvelopeV1 {
    pub fn new(
        seq: u64,
        identity: WorkerIdentityClaimV1,
        frames: Vec<WorkerFrameV1>,
        traces: Vec<WorkerTraceV1>,
    ) -> Result<Self, WorkerTrustErrorV1> {
        let envelope = Self {
            schema_version: WORKER_TRUST_SCHEMA_VERSION_V1,
            seq,
            identity,
            frames,
            traces,
            abi_version: WORKER_TRUST_ABI_VERSION_V1.to_owned(),
        };
        envelope.validate()?;
        Ok(envelope)
    }

    pub fn validate(&self) -> Result<(), WorkerTrustErrorV1> {
        if self.schema_version != WORKER_TRUST_SCHEMA_VERSION_V1 {
            return Err(WorkerTrustErrorV1::InvalidEnvelope(
                "envelope schema version is not supported".to_owned(),
            ));
        }
        if self.abi_version != WORKER_TRUST_ABI_VERSION_V1 {
            return Err(WorkerTrustErrorV1::InvalidEnvelope(
                "envelope ABI version is not supported".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, WorkerTrustErrorV1> {
        self.validate()?;
        canonical_bytes(self)
    }

    /// Content-derived envelope digest.
    pub fn digest(&self) -> Result<DigestV1, WorkerTrustErrorV1> {
        Ok(domain_digest(
            WORKER_TRUST_ENVELOPE_DOMAIN_V1,
            &self.canonical_bytes()?,
        ))
    }
}

/// The digest-pinned trust context the boundary checks against. Mirrors the
/// `AssemblyManifestV1` identity binding from zero-abi: engine + artifact +
/// protocol digests, plus bounded frame/trace budgets.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrustContextV1 {
    pub expected_engine: String,
    pub expected_artifact_digest: DigestV1,
    pub expected_protocol_digest: DigestV1,
    pub max_frames: u64,
    pub max_traces: u64,
    pub max_trace_tokens: u64,
}

impl TrustContextV1 {
    pub fn new(
        expected_engine: impl Into<String>,
        expected_artifact_digest: DigestV1,
        expected_protocol_digest: DigestV1,
        max_frames: u64,
        max_traces: u64,
        max_trace_tokens: u64,
    ) -> Result<Self, WorkerTrustErrorV1> {
        let context = Self {
            expected_engine: expected_engine.into(),
            expected_artifact_digest,
            expected_protocol_digest,
            max_frames,
            max_traces,
            max_trace_tokens,
        };
        if context.expected_engine.is_empty()
            || context.expected_artifact_digest == DigestV1::ZERO
            || context.expected_protocol_digest == DigestV1::ZERO
        {
            return Err(WorkerTrustErrorV1::InvalidEnvelope(
                "trust context engine and digests must be nonzero".to_owned(),
            ));
        }
        if context.max_frames == 0 || context.max_traces == 0 || context.max_trace_tokens == 0 {
            return Err(WorkerTrustErrorV1::InvalidEnvelope(
                "trust context budgets must be nonzero".to_owned(),
            ));
        }
        Ok(context)
    }
}

/// Why the boundary refused a worker envelope.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRefusalReasonV1 {
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
pub struct WorkerRefusalRecordV1 {
    pub schema_version: u16,
    pub envelope_digest: DigestV1,
    pub reason: WorkerRefusalReasonV1,
    pub detail: String,
    pub seq: u64,
    pub refused_at_unix_ns: u64,
    pub abi_version: String,
}

impl WorkerRefusalRecordV1 {
    fn new(
        envelope_digest: DigestV1,
        reason: WorkerRefusalReasonV1,
        detail: String,
        seq: u64,
    ) -> Self {
        Self {
            schema_version: WORKER_TRUST_SCHEMA_VERSION_V1,
            envelope_digest,
            reason,
            detail,
            seq,
            refused_at_unix_ns: now_unix_ns(),
            abi_version: WORKER_TRUST_ABI_VERSION_V1.to_owned(),
        }
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, WorkerTrustErrorV1> {
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<DigestV1, WorkerTrustErrorV1> {
        Ok(domain_digest(
            WORKER_TRUST_REFUSAL_DOMAIN_V1,
            &self.canonical_bytes()?,
        ))
    }
}

/// Sealed admission receipt: the envelope passed the boundary. Admission is
/// NOT authority -- cache/commit authority still requires the gates.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerAdmissionReceiptV1 {
    pub schema_version: u16,
    pub envelope_digest: DigestV1,
    pub seq: u64,
    pub frames: u64,
    pub traces: u64,
    pub trace_tokens: u64,
    pub admitted_at_unix_ns: u64,
    pub abi_version: String,
}

impl WorkerAdmissionReceiptV1 {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, WorkerTrustErrorV1> {
        canonical_bytes(self)
    }

    pub fn digest(&self) -> Result<DigestV1, WorkerTrustErrorV1> {
        Ok(domain_digest(
            WORKER_TRUST_ADMISSION_DOMAIN_V1,
            &self.canonical_bytes()?,
        ))
    }
}

/// Loud, fail-closed worker-trust errors. Every refusal carries its sealed
/// record; structural invalidity is a loud error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerTrustErrorV1 {
    Refused { record: WorkerRefusalRecordV1 },
    InvalidEnvelope(String),
}

impl std::fmt::Display for WorkerTrustErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerTrustErrorV1::Refused { record } => {
                write!(formatter, "worker trust refusal {:?}: {}", record.reason, record.detail)
            }
            WorkerTrustErrorV1::InvalidEnvelope(detail) => {
                write!(formatter, "invalid worker envelope: {detail}")
            }
        }
    }
}

impl std::error::Error for WorkerTrustErrorV1 {}

/// The out-of-process trust boundary. Stateful: tracks the last accepted
/// envelope sequence so whole-envelope replays are refused.
#[derive(Clone, Debug)]
pub struct WorkerTrustBoundaryV1 {
    pub context: TrustContextV1,
    pub last_accepted_seq: u64,
}

impl WorkerTrustBoundaryV1 {
    pub fn new(context: TrustContextV1) -> Self {
        Self {
            context,
            last_accepted_seq: 0,
        }
    }

    fn refuse(
        &self,
        envelope_digest: DigestV1,
        reason: WorkerRefusalReasonV1,
        detail: String,
        seq: u64,
    ) -> WorkerTrustErrorV1 {
        WorkerTrustErrorV1::Refused {
            record: WorkerRefusalRecordV1::new(envelope_digest, reason, detail, seq),
        }
    }

    /// Admit a worker envelope, or refuse it loudly with a sealed record.
    /// On acceptance the envelope's seq becomes the boundary's last
    /// accepted seq (replays refused thereafter).
    pub fn admit(
        &mut self,
        envelope: &WorkerEnvelopeV1,
    ) -> Result<WorkerAdmissionReceiptV1, WorkerTrustErrorV1> {
        envelope.validate()?;
        let envelope_digest = envelope.digest()?;

        let refusal = |reason: WorkerRefusalReasonV1, detail: String| {
            self.refuse(envelope_digest, reason, detail, envelope.seq)
        };

        // 1. Digest-pinned identity: the claim must match the context
        //    exactly. A forged or stolen identity cannot pass.
        if envelope.identity.engine != self.context.expected_engine
            || envelope.identity.artifact_digest != self.context.expected_artifact_digest
            || envelope.identity.protocol_digest != self.context.expected_protocol_digest
        {
            return Err(refusal(
                WorkerRefusalReasonV1::IdentityMismatch,
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
                WorkerRefusalReasonV1::ReplayedEnvelope,
                format!(
                    "envelope seq {} is not newer than the last accepted seq {}",
                    envelope.seq, self.last_accepted_seq
                ),
            ));
        }

        // 3. Frames: bounded count, content-addressed payloads, unique
        //    indices (no forged or replayed frames).
        if envelope.frames.len() as u64 > self.context.max_frames {
            return Err(refusal(
                WorkerRefusalReasonV1::FrameBudgetExceeded,
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
                    WorkerRefusalReasonV1::ReplayedFrame {
                        frame_index: frame.frame_index,
                    },
                    format!("frame index {} appears more than once", frame.frame_index),
                ));
            }
            if frame.opcode.is_empty() {
                return Err(refusal(
                    WorkerRefusalReasonV1::ForgedFrame {
                        frame_index: frame.frame_index,
                    },
                    "frame opcode must be nonempty".to_owned(),
                ));
            }
            let actual = zero_abi::sha256(&frame.payload);
            if DigestV1::from_bytes(actual) != frame.payload_digest {
                return Err(refusal(
                    WorkerRefusalReasonV1::ForgedFrame {
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
                WorkerRefusalReasonV1::TraceTokenBudgetExceeded,
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
                    WorkerRefusalReasonV1::ForgedTrace {
                        trace_index: trace.trace_index,
                    },
                    format!("trace index {} appears more than once", trace.trace_index),
                ));
            }
            if trace.stage.is_empty() || trace.root == DigestV1::ZERO {
                return Err(refusal(
                    WorkerRefusalReasonV1::ForgedTrace {
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
                WorkerRefusalReasonV1::TraceTokenBudgetExceeded,
                format!(
                    "trace token total {total_tokens} exceeds the bound {}",
                    self.context.max_trace_tokens
                ),
            ));
        }

        self.last_accepted_seq = envelope.seq;
        Ok(WorkerAdmissionReceiptV1 {
            schema_version: WORKER_TRUST_SCHEMA_VERSION_V1,
            envelope_digest,
            seq: envelope.seq,
            frames: envelope.frames.len() as u64,
            traces: envelope.traces.len() as u64,
            trace_tokens: total_tokens,
            admitted_at_unix_ns: now_unix_ns(),
            abi_version: WORKER_TRUST_ABI_VERSION_V1.to_owned(),
        })
    }
}

/// The frozen contract manifest for the worker trust boundary (ZS-OPS-005).
pub fn worker_trust_contract_v1() -> serde_json::Value {
    serde_json::json!({
        "schema_version": WORKER_TRUST_SCHEMA_VERSION_V1,
        "boundary": {
            "identity": "digest-pinned engine/artifact/protocol; forged or stolen identity refused",
            "frames": "content-addressed payloads; forged or replayed frames refused; bounded",
            "traces": "well-typed and token-bounded; forged traces refused",
            "envelope_order": "seq monotonic; replayed envelopes refused",
        },
        "refusals": "fail-loud with sealed WorkerRefusalRecordV1; never silent",
        "authority": "admission is NOT authority; cache authority requires CacheAdmissionGateV1, commit authority requires ProjectRootGateV1",
        "abi_version": WORKER_TRUST_ABI_VERSION_V1,
    })
}
