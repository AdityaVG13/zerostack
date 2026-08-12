//! In-process fixture domain adapters (feature `fixture-adapters`).
//!
//! These adapters mirror the raw-worker-v2 fixture semantics in memory: they
//! echo the canonical request (args, store root, session id), honor
//! cancellation and deadlines, emit typed telemetry on request, and support
//! the approval and CAS-reachability fixture hooks. They exist so the hub can
//! prove the single-process ZSX path end-to-end without engine repositories;
//! real FSZero/GraphZero/TokenZero adapters implement [`DomainAdapter`]
//! directly and register through the same builder.

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use serde_json::json;
use zero_abi::{
    ApprovalMetadata, ApprovalState, CallRequest, EffectClass, EngineIdentity, EngineStageSpanV1,
    EngineStageTimelineV1, RefOwnership, RevertMetadata, WorkerResult, WorkerResultMetadata,
    WorkerTokenAccountingV1, WorkerTokenCountKind,
};
use zero_store::SharedCas;

use crate::adapter::{AdapterBinding, AdapterCall, AdapterError, AdapterResponse, DomainAdapter};

/// One in-process fixture adapter for a single engine.
#[derive(Clone)]
pub struct FixtureAdapter {
    engine: EngineIdentity,
    root: PathBuf,
    session_id: String,
    binding: AdapterBinding,
    calls: Arc<AtomicU64>,
}

impl FixtureAdapter {
    /// Build a fixture adapter for `engine` rooted at `root`.
    pub fn new(engine: EngineIdentity, root: impl Into<PathBuf>, session_id: &str) -> Self {
        let root = root.into();
        let binding = match engine {
            EngineIdentity::FsZero => AdapterBinding::new(
                engine,
                "fixture-revision",
                "fixture.v1",
                "0".repeat(64),
                "0".repeat(64),
                "fz://",
            )
            .expect("fixture binding is valid"),
            EngineIdentity::GraphZero => AdapterBinding::new(
                engine,
                "fixture-revision",
                "fixture.v1",
                "0".repeat(64),
                "0".repeat(64),
                "gz://",
            )
            .expect("fixture binding is valid"),
            EngineIdentity::TokenZero => AdapterBinding::new(
                engine,
                "fixture-revision",
                "fixture.v1",
                "0".repeat(64),
                "0".repeat(64),
                "tz://",
            )
            .expect("fixture binding is valid"),
        };
        Self {
            engine,
            root,
            session_id: session_id.to_owned(),
            binding,
            calls: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Number of in-process dispatches this adapter has served.
    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }

    fn result(&self, request: &CallRequest) -> WorkerResult {
        let expose_approval = request.args["__approval_fixture"] == true;
        let approval_grant = request.approval_grant.clone();
        let mut value = if self.engine == EngineIdentity::TokenZero
            && request.op == "shell"
            && request.args["background"] == true
        {
            json!({"job":"fixture-job","cursor":0,"version":0})
        } else if self.engine == EngineIdentity::TokenZero
            && request.op == zero_abi::TOKEN_JOB_OPERATION_V1
        {
            json!({
                "id":request.args["id"],
                "status":"exited",
                "exitCode":0,
                "tail":"ok",
                "tailUtf8Lossless":true,
                "tailBytes":2,
                "logBytes":2,
                "cursor":2,
                "version":1,
                "changed":true
            })
        } else {
            json!({
                "args": request.args,
                "store_root": self.root,
                "session_id": self.session_id,
            })
        };
        let mut refs = Vec::new();
        if let Some(reference) = request.args["__reachability_ref_fixture"].as_str() {
            refs.push(reference.to_owned());
        }
        if expose_approval {
            value["approval_grant"] = serde_json::to_value(&approval_grant).unwrap();
        }
        let (effect, approval) = if expose_approval {
            (
                EffectClass::ApprovalRequiredMutation,
                match approval_grant {
                    Some(grant) => ApprovalMetadata {
                        state: ApprovalState::Granted,
                        approval_id: Some(grant.grant_id),
                        policy: Some("fixture-approval-required".into()),
                    },
                    None => ApprovalMetadata {
                        state: ApprovalState::Required,
                        approval_id: None,
                        policy: Some("fixture-approval-required".into()),
                    },
                },
            )
        } else {
            (
                EffectClass::ReadOnly,
                ApprovalMetadata {
                    state: ApprovalState::NotRequired,
                    approval_id: None,
                    policy: None,
                },
            )
        };
        WorkerResult {
            value,
            metadata: WorkerResultMetadata {
                effect,
                approval,
                revert: RevertMetadata {
                    supported: false,
                    journal_id: None,
                    rollback_op: None,
                },
                ownership: RefOwnership {
                    engine: self.engine,
                    session_id: self.session_id.clone(),
                    refs,
                    snapshot: None,
                },
                trace: request.trace.clone(),
            },
        }
    }
}

impl DomainAdapter for FixtureAdapter {
    fn engine(&self) -> EngineIdentity {
        self.engine
    }

    fn binding(&self) -> AdapterBinding {
        self.binding.clone()
    }

    fn call(&self, call: AdapterCall<'_>) -> Result<AdapterResponse, AdapterError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let request = call.request;
        if request.args["__fixture_fail"] == true {
            return Err(AdapterError::new(
                "fixture_failure",
                "fixture adapter failed by request",
                false,
                Some(request.trace.clone()),
            ));
        }
        if call.cancellation.is_cancelled() {
            return Err(AdapterError::new(
                "cancelled",
                "fixture adapter cancelled",
                false,
                Some(request.trace.clone()),
            ));
        }
        // Cooperative cancellation + deadline during a fixture delay.
        if let Some(delay_ms) = request.args["__fixture_delay_ms"].as_u64() {
            let started = Instant::now();
            let budget = Duration::from_millis(delay_ms);
            let deadline = request.deadline_unix_ms;
            loop {
                if call.cancellation.is_cancelled() {
                    return Err(AdapterError::new(
                        "cancelled",
                        "fixture adapter cancelled during delay",
                        false,
                        Some(request.trace.clone()),
                    ));
                }
                if let Some(deadline) = deadline
                    && crate::connector::now_ms() >= deadline
                {
                    return Err(AdapterError::new(
                        "deadline",
                        "fixture adapter deadline exceeded",
                        false,
                        Some(request.trace.clone()),
                    ));
                }
                if started.elapsed() >= budget {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let requested_timeline = request
            .telemetry_request
            .as_ref()
            .is_some_and(|value| value.engine_stage_timeline);
        let requested_accounting = request
            .telemetry_request
            .as_ref()
            .is_some_and(|value| value.worker_token_accounting);
        let engine_timeline = requested_timeline.then(|| EngineStageTimelineV1 {
            total_ns: 300,
            spans: vec![
                EngineStageSpanV1 {
                    stage: "fixture_decode".into(),
                    start_ns: 0,
                    duration_ns: 100,
                },
                EngineStageSpanV1 {
                    stage: "fixture_execute".into(),
                    start_ns: 100,
                    duration_ns: 200,
                },
            ],
        });
        let worker_token_accounting = (requested_accounting
            && request.args["__fixture_accounting"] != "missing")
            .then(|| WorkerTokenAccountingV1 {
                tokenizer_id: if request.args["__fixture_accounting"] == "estimate" {
                    "estimator:fixture-v1".into()
                } else {
                    "fixture-tokenizer-v1".into()
                },
                count_kind: if request.args["__fixture_accounting"] == "estimate" {
                    WorkerTokenCountKind::Estimate
                } else {
                    WorkerTokenCountKind::Exact
                },
                raw_tokens: if request.args["__fixture_accounting"] == "max" {
                    u64::MAX
                } else {
                    8
                },
                visible_tokens: 4,
                recovery_tokens: 0,
                billed_tokens: 8,
                cached_tokens: 2,
                exact_ref_tokens: Some(0),
            });
        Ok(AdapterResponse {
            result: self.result(request),
            engine_timeline,
            worker_token_accounting,
        })
    }
}

/// Register one fixture adapter per engine, all rooted at `root`.
///
/// Returns the three adapters wrapped in `Arc` so callers can observe the
/// in-process call counters after a session runs.
pub fn fixture_adapters(
    root: impl Into<PathBuf>,
    session_id: &str,
) -> (
    Arc<FixtureAdapter>,
    Arc<FixtureAdapter>,
    Arc<FixtureAdapter>,
) {
    let root = root.into();
    (
        Arc::new(FixtureAdapter::new(
            EngineIdentity::FsZero,
            root.clone(),
            session_id,
        )),
        Arc::new(FixtureAdapter::new(
            EngineIdentity::GraphZero,
            root.clone(),
            session_id,
        )),
        Arc::new(FixtureAdapter::new(
            EngineIdentity::TokenZero,
            root,
            session_id,
        )),
    )
}

/// Publish `bytes` into the shared CAS under `root` and return the canonical
/// engine-scoped ref, so fixture tests can exercise reachability retention.
pub fn publish_fixture_blob(
    root: &std::path::Path,
    engine: EngineIdentity,
    bytes: &[u8],
) -> String {
    let hash = SharedCas::open(root)
        .put(bytes)
        .expect("fixture blob publishes to the canonical CAS");
    let scheme = match engine {
        EngineIdentity::FsZero => "fz",
        EngineIdentity::GraphZero => "gz",
        EngineIdentity::TokenZero => "tz",
    };
    format!("{scheme}://blob/{hash}")
}

/// A shared `Value`-based adapter for unit tests that want direct control
/// over results without a store root.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_binding_and_echo_are_typed() {
        let adapter = FixtureAdapter::new(EngineIdentity::TokenZero, "/tmp", "session-fixture");
        assert_eq!(adapter.engine(), EngineIdentity::TokenZero);
        assert_eq!(adapter.binding().ref_scheme, "tz://");
        assert_eq!(adapter.binding().semantic_contract_digest.len(), 64);
    }
}
