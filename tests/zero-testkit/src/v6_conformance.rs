//! V6 cross-transport semantic replay + schema-identity harness
//! (ZS-ADAPTER-001/002/009/010/011).
//!
//! Transports are FIXTURE ADAPTERS: pure, in-process projections of one
//! semantic envelope into each transport's wire representation -- CLI
//! (JSON stdout envelope), RPC (raw-worker-v2 NDJSON frame through the shared
//! frame codec), native (addon envelope), MCP (JSON-RPC 2.0 result). The hub
//! replays the same canonical vectors from
//! `conformance/fixtures/v6_cross_transport_vectors.json` through at least
//! three of these transports and asserts byte-identical protected fields
//! (kind, project root, ledger root, continuation handle, audit range) plus
//! equivalent cancellation/timeout/Unknown/fallback semantics. Real engines
//! run the same vectors against their own transports in their own repos.
//!
//! Violation handling is fail-closed: a tampered projection (relabeled kind,
//! swapped ledger root, injected unknown field) must be refused loudly --
//! recovery or validation errors, never a silently laundered envelope.

use serde_json::{Value, json};
use zero_abi::{
    AuditEventRangeV1, DEFAULT_MAX_FRAME_BYTES, EffectClass, EngineIdentity, RefOwnership,
    RevertMetadata, WorkerResponseFrame, WorkerResult, WorkerResultMetadata,
    ZeroExecuteKindV6, ZeroExecuteResultV6, decode_response_frame, encode_frame,
};

/// The four fixture transports of the V6 adapter conformance suite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum V6Transport {
    Cli,
    Rpc,
    Native,
    Mcp,
}

impl V6Transport {
    pub const ALL: [V6Transport; 4] = [
        V6Transport::Cli,
        V6Transport::Rpc,
        V6Transport::Native,
        V6Transport::Mcp,
    ];

    pub fn name(self) -> &'static str {
        match self {
            V6Transport::Cli => "cli",
            V6Transport::Rpc => "rpc",
            V6Transport::Native => "native",
            V6Transport::Mcp => "mcp",
        }
    }

    /// Project one V6 envelope into this transport's wire representation.
    pub fn project(self, envelope: &ZeroExecuteResultV6, request_id: u64) -> Value {
        let envelope_value =
            serde_json::to_value(envelope).expect("V6 envelope always serializes");
        match self {
            V6Transport::Cli => envelope_value,
            V6Transport::Rpc => {
                let frame = WorkerResponseFrame::Result {
                    request_id: request_id.to_string(),
                    result: WorkerResult {
                        value: envelope_value,
                        metadata: WorkerResultMetadata {
                            effect: EffectClass::ReadOnly,
                            approval: zero_abi::ApprovalMetadata {
                                state: zero_abi::ApprovalState::NotRequired,
                                approval_id: None,
                                policy: None,
                            },
                            revert: RevertMetadata {
                                supported: false,
                                journal_id: None,
                                rollback_op: None,
                            },
                            ownership: RefOwnership {
                                engine: EngineIdentity::FsZero,
                                session_id: "fixture-v6-conformance".into(),
                                refs: Vec::new(),
                                snapshot: None,
                            },
                            trace: zero_abi::WorkerTrace {
                                runtime_id: "fixture-v6-conformance".into(),
                                cell_id: "cm://cell/fixture-v6-conformance".into(),
                                request_id: request_id.to_string(),
                                trace_id: format!("fixture-v6-conformance-{request_id}"),
                                parent_span_id: None,
                                worker_revision: "fixture-revision".into(),
                                contract_digest: "0".repeat(64),
                            },
                        },
                    },
                    engine_timeline: None,
                    worker_token_accounting: None,
                };
                let bytes = encode_frame(&frame, DEFAULT_MAX_FRAME_BYTES)
                    .expect("V6 envelope fits the frame bound");
                json!({
                    "transport": "rpc",
                    "frame": String::from_utf8(bytes).expect("frame is UTF-8 JSON"),
                })
            }
            V6Transport::Native => json!({
                "protocol": "zerostack.zsx.v1",
                "ok": true,
                "generation": 1,
                "request_id": request_id,
                "result": envelope_value,
            }),
            V6Transport::Mcp => json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": envelope_value,
            }),
        }
    }

    /// Recover the typed envelope from this transport's wire representation.
    /// Any shape deviation is a loud error -- never a silent approximation.
    pub fn recover(self, projection: &Value) -> Result<ZeroExecuteResultV6, String> {
        let envelope_value = match self {
            V6Transport::Cli => projection.clone(),
            V6Transport::Rpc => {
                let frame = projection
                    .get("frame")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "rpc projection missing frame string".to_owned())?;
                let frames = frame
                    .split('\n')
                    .filter(|line| !line.is_empty())
                    .map(|line| {
                        decode_response_frame(line.as_bytes(), DEFAULT_MAX_FRAME_BYTES)
                            .map_err(|error| format!("rpc frame decode failed: {error}"))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let frame = frames
                    .first()
                    .ok_or_else(|| "rpc projection has an empty transcript".to_owned())?;
                match frame {
                    WorkerResponseFrame::Result { result, .. } => result.value.clone(),
                    other => {
                        return Err(format!(
                            "rpc frame is not a Result frame: {other:?}"
                        ));
                    }
                }
            }
            V6Transport::Native => projection
                .get("result")
                .cloned()
                .ok_or_else(|| "native projection missing result envelope".to_owned())?,
            V6Transport::Mcp => projection
                .get("result")
                .cloned()
                .ok_or_else(|| "mcp projection missing result envelope".to_owned())?,
        };
        let envelope: ZeroExecuteResultV6 = serde_json::from_value(envelope_value).map_err(
            |error| format!("envelope recovery failed: {error}"),
        )?;
        envelope
            .validate()
            .map_err(|error| format!("recovered envelope failed validation: {error}"))?;
        Ok(envelope)
    }
}

/// The protected fields that must survive every transport byte-identically:
/// kind, project root, resource ledger root, continuation handle, and audit
/// event range.
pub fn protected_fields(envelope: &ZeroExecuteResultV6) -> Value {
    let range = envelope.audit_event_range();
    json!({
        "abi_version": envelope.abi_version(),
        "kind": envelope.kind().kind_name(),
        "project_root": envelope.project_root(),
        "resource_ledger_root": envelope.resource_ledger_root(),
        "continuation_handle": envelope.continuation_handle(),
        "audit_event_range": {"start": range.start, "end": range.end},
    })
}

/// Build the typed envelope named by a fixture vector. The vector carries the
/// kind plus the semantic fields the per-kind fail-closed constructors need.
pub fn envelope_from_vector(vector: &Value) -> Result<ZeroExecuteResultV6, String> {
    let kind = vector
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "vector missing kind".to_owned())?;
    let fields = vector.get("fields").cloned().unwrap_or_else(|| json!({}));
    let take = |name: &str| -> Option<String> {
        fields.get(name).and_then(Value::as_str).map(str::to_owned)
    };
    let ledger = take("resource_ledger_root")
        .ok_or_else(|| "vector missing resource_ledger_root".to_owned())?;
    let range = AuditEventRangeV1::new(
        fields
            .get("audit_start")
            .and_then(Value::as_u64)
            .ok_or_else(|| "vector missing audit_start".to_owned())?,
        fields
            .get("audit_end")
            .and_then(Value::as_u64)
            .ok_or_else(|| "vector missing audit_end".to_owned())?,
    )
    .map_err(|error| format!("invalid audit range: {error}"))?;
    let base = zero_abi::ZeroExecuteFieldsV6 {
        continuation_handle: take("continuation_handle"),
        project_root: take("project_root"),
        decision_view_root: take("decision_view_root"),
        question: take("question"),
        choices: fields
            .get("choices")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        unknown_reasons: fields
            .get("unknown_reasons")
            .and_then(Value::as_array)
            .map(|reasons| {
                reasons
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        successor_root: take("successor_root"),
        result_root: take("result_root"),
        exact_delta_root: take("exact_delta_root"),
        verification_receipt_root: take("verification_receipt_root"),
        successor_certificate_root: take("successor_certificate_root"),
        cache_report_root: take("cache_report_root"),
        no_mutation_receipt_root: take("no_mutation_receipt_root"),
    };
    let kind_enum = match kind {
        "Completed" => ZeroExecuteKindV6::Completed,
        "DecisionRequired" => ZeroExecuteKindV6::DecisionRequired,
        "EvidenceExpansionRequired" => ZeroExecuteKindV6::EvidenceExpansionRequired,
        "VerificationUnknown" => ZeroExecuteKindV6::VerificationUnknown,
        "BaselineFallbackRequired" => ZeroExecuteKindV6::BaselineFallbackRequired,
        "RejectedNoMutation" => ZeroExecuteKindV6::RejectedNoMutation,
        "Cancelled" => ZeroExecuteKindV6::Cancelled,
        "FailedNoAuthority" => ZeroExecuteKindV6::FailedNoAuthority,
        other => return Err(format!("unknown kind {other}")),
    };
    match kind_enum {
        ZeroExecuteKindV6::Completed => ZeroExecuteResultV6::completed(
            zero_abi::SafetyVerdictV1::from_premises(&[
                zero_abi::PremiseV1::new("p1", Some(true)).unwrap(),
                zero_abi::PremiseV1::new("p2", Some(true)).unwrap(),
            ]),
            base,
            ledger,
            range,
        )
        .map_err(|error| format!("completed vector failed: {error}")),
        ZeroExecuteKindV6::DecisionRequired => {
            ZeroExecuteResultV6::decision_required(base, ledger, range)
                .map_err(|error| format!("decision vector failed: {error}"))
        }
        ZeroExecuteKindV6::EvidenceExpansionRequired => {
            ZeroExecuteResultV6::evidence_expansion_required(base, ledger, range)
                .map_err(|error| format!("evidence vector failed: {error}"))
        }
        ZeroExecuteKindV6::VerificationUnknown => {
            ZeroExecuteResultV6::verification_unknown(base, ledger, range)
                .map_err(|error| format!("verification-unknown vector failed: {error}"))
        }
        ZeroExecuteKindV6::BaselineFallbackRequired => {
            ZeroExecuteResultV6::baseline_fallback_required(base, ledger, range)
                .map_err(|error| format!("baseline-fallback vector failed: {error}"))
        }
        ZeroExecuteKindV6::RejectedNoMutation => {
            ZeroExecuteResultV6::rejected_no_mutation(base, ledger, range)
                .map_err(|error| format!("rejected-no-mutation vector failed: {error}"))
        }
        ZeroExecuteKindV6::Cancelled => {
            ZeroExecuteResultV6::cancelled(base, ledger, range)
                .map_err(|error| format!("cancelled vector failed: {error}"))
        }
        ZeroExecuteKindV6::FailedNoAuthority => {
            ZeroExecuteResultV6::failed_no_authority(base, ledger, range)
                .map_err(|error| format!("failed-no-authority vector failed: {error}"))
        }
    }
}

/// One fixture-named violation: the mechanical mutation the fixture says must
/// be refused loudly. The mutation targets the semantic envelope wherever the
/// transport carries it (CLI: the projection itself; native/MCP: the result
/// member; RPC: the decoded frame's carried value, re-encoded afterwards).
pub fn apply_violation(projection: &mut Value, mutation: &str) -> Result<(), String> {
    if projection.get("frame").is_some() {
        // RPC: decode the NDJSON frame, mutate the carried envelope value,
        // and re-encode the frame so the tampered projection stays on the
        // wire representation.
        let frame = projection["frame"]
            .as_str()
            .ok_or_else(|| "rpc projection missing frame string".to_owned())?;
        let frames = frame
            .split('\n')
            .filter(|line| !line.is_empty())
            .map(|line| {
                decode_response_frame(line.as_bytes(), DEFAULT_MAX_FRAME_BYTES)
                    .map_err(|error| format!("rpc frame decode failed: {error}"))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let WorkerResponseFrame::Result {
            request_id,
            result,
            engine_timeline,
            worker_token_accounting,
        } = frames
            .into_iter()
            .next()
            .ok_or_else(|| "rpc projection has an empty transcript".to_owned())?
        else {
            return Err("rpc frame is not a Result frame".to_owned());
        };
        let mut envelope = result.value;
        mutate_envelope(
            envelope
                .as_object_mut()
                .ok_or_else(|| "rpc envelope is not an object".to_owned())?,
            mutation,
        )?;
        let frame = WorkerResponseFrame::Result {
            request_id,
            result: WorkerResult {
                value: envelope,
                metadata: result.metadata,
            },
            engine_timeline,
            worker_token_accounting,
        };
        let bytes = encode_frame(&frame, DEFAULT_MAX_FRAME_BYTES)
            .map_err(|error| format!("rpc re-encode failed: {error}"))?;
        projection["frame"] = json!(String::from_utf8(bytes)
            .map_err(|_| "rpc re-encoded frame is not UTF-8".to_owned())?);
        return Ok(());
    }
    let envelope = if let Some(result) = projection.get_mut("result") {
        result
    } else {
        projection
    };
    mutate_envelope(
        envelope
            .as_object_mut()
            .ok_or_else(|| "transport envelope is not an object".to_owned())?,
        mutation,
    )
}

fn mutate_envelope(
    envelope: &mut serde_json::Map<String, Value>,
    mutation: &str,
) -> Result<(), String> {
    match mutation {
        "relabel_kind" => {
            envelope.insert("kind".into(), json!("Completed"));
            Ok(())
        }
        "swap_ledger" => {
            envelope.insert("resource_ledger_root".into(), json!("fz://blob/swapped-ledger"));
            Ok(())
        }
        "inject_unknown_field" => {
            envelope.insert("future_field".into(), json!(1));
            Ok(())
        }
        other => Err(format!("unknown violation mutation {other}")),
    }
}

/// Recursive shape signature: key sets at every level plus value type tags.
/// Byte-identical across all instances iff the envelope schema is fixed.
pub fn shape_signature(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut parts = map
                .iter()
                .map(|(key, value)| format!("{key}:{}", shape_signature(value)))
                .collect::<Vec<_>>();
            parts.sort();
            format!("obj{{{}}}", parts.join(","))
        }
        Value::Array(items) => format!(
            "arr[{}]",
            items
                .iter()
                .map(shape_signature)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Null => "null".into(),
        Value::Bool(_) => "bool".into(),
        Value::Number(number) => {
            if number.is_i64() || number.is_u64() {
                "int".into()
            } else {
                "float".into()
            }
        }
        Value::String(_) => "string".into(),
    }
}
