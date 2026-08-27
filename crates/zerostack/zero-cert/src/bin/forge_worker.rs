//! Out-of-process malicious-worker fixture (ZS-OPS-005 / V6-R14).
//!
//! Reads one JSON `ForgeSpec` line from
//! stdin and prints the forged `WorkerTrustEnvelope` canonical JSON line to
//! stdout. The parent feeds the envelope bytes to
//! `WorkerTrustBoundary::admit` exactly as it would receive output from a
//! remote producer -- the boundary never sees the in-process construction,
//! which is the point of the out-of-process fixture.
//!
//! Forge kinds:
//! - `honest`: an envelope that passes the boundary.
//! - `forged_frame`: a frame whose payload does not hash to its declared
//!   digest.
//! - `replayed_frame`: two frames with the same index.
//! - `forged_trace`: a trace with an empty stage and zero root.
//! - `stolen_identity`: engine matches, artifact digest does not.
//! - `replay_envelope`: seq 1 (parent admits seq 1 first, then feeds this).

use std::io::Read;

use serde::{Deserialize, Serialize};

use zero_cert::worker_trust::{
    TrustContext, WorkerTrustEnvelope, WorkerFrame, WorkerIdentityClaim, WorkerTrustTrace,
};
use zero_abi::{Sha256Digest, canonical_json};

const PINNED_ENGINE: &str = "fixture-engine";
const PINNED_ARTIFACT: u8 = 0xAA;
const PINNED_PROTOCOL: u8 = 0xBB;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeSpec {
    pub kind: String,
    pub seq: u64,
}

fn digest(byte: u8) -> Sha256Digest {
    Sha256Digest::from_bytes([byte; 32])
}

fn honest_identity() -> WorkerIdentityClaim {
    WorkerIdentityClaim {
        engine: PINNED_ENGINE.to_owned(),
        artifact_digest: digest(PINNED_ARTIFACT),
        protocol_digest: digest(PINNED_PROTOCOL),
    }
}

fn honest_frame(payload: &[u8]) -> WorkerFrame {
    WorkerFrame {
        frame_index: 0,
        opcode: "op".to_owned(),
        payload: payload.to_vec(),
        payload_digest: Sha256Digest::from_bytes(zero_abi::sha256(payload)),
    }
}

fn honest_trace() -> WorkerTrustTrace {
    WorkerTrustTrace {
        trace_index: 0,
        stage: "execute".to_owned(),
        tokens: 10,
        root: digest(0xCC),
    }
}

fn build(spec: &ForgeSpec) -> WorkerTrustEnvelope {
    match spec.kind.as_str() {
        "honest" => WorkerTrustEnvelope::new(
            spec.seq,
            honest_identity(),
            vec![honest_frame(b"honest payload")],
            vec![honest_trace()],
        )
        .expect("honest envelope"),
        "forged_frame" => {
            // Declared digest does not match the payload bytes.
            let frame = WorkerFrame {
                frame_index: 0,
                opcode: "op".to_owned(),
                payload: b"forged payload".to_vec(),
                payload_digest: digest(0xEE),
            };
            WorkerTrustEnvelope::new(
                spec.seq,
                honest_identity(),
                vec![frame],
                vec![honest_trace()],
            )
            .expect("forged-frame envelope")
        }
        "replayed_frame" => {
            let frame = honest_frame(b"payload");
            WorkerTrustEnvelope::new(
                spec.seq,
                honest_identity(),
                vec![frame.clone(), frame],
                vec![honest_trace()],
            )
            .expect("replayed-frame envelope")
        }
        "forged_trace" => {
            let trace = WorkerTrustTrace {
                trace_index: 0,
                stage: String::new(),
                tokens: 5,
                root: Sha256Digest::ZERO,
            };
            WorkerTrustEnvelope::new(
                spec.seq,
                honest_identity(),
                vec![honest_frame(b"payload")],
                vec![trace],
            )
            .expect("forged-trace envelope")
        }
        "stolen_identity" => {
            let identity = WorkerIdentityClaim {
                engine: PINNED_ENGINE.to_owned(),
                artifact_digest: digest(0x99), // not the pinned artifact
                protocol_digest: digest(PINNED_PROTOCOL),
            };
            WorkerTrustEnvelope::new(
                spec.seq,
                identity,
                vec![honest_frame(b"payload")],
                vec![honest_trace()],
            )
            .expect("stolen-identity envelope")
        }
        "replay_envelope" => WorkerTrustEnvelope::new(
            spec.seq,
            honest_identity(),
            vec![honest_frame(b"payload")],
            vec![honest_trace()],
        )
        .expect("replay envelope"),
        other => panic!("unknown forge kind: {other}"),
    }
}

fn main() {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .expect("read forge spec");
    let spec: ForgeSpec =
        serde_json::from_str(input.trim()).expect("forge spec must be JSON ForgeSpec");
    let envelope = build(&spec);
    let json = serde_json::to_value(&envelope).expect("envelope serializable");
    println!("{}", canonical_json(&json));
}

/// The pinned context the fixture binaries build against; the parent uses
/// the same values so the honest envelope passes and forgeries are refused.
pub fn fixture_trust_context() -> TrustContext {
    TrustContext::new(
        PINNED_ENGINE,
        digest(PINNED_ARTIFACT),
        digest(PINNED_PROTOCOL),
        8,
        8,
        1000,
    )
    .expect("fixture context")
}
