#![forbid(unsafe_code)]

use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zero_abi::raw_worker::EngineIdentity;
use zero_abi::{
    ApprovalMetadata, ApprovalState, CallRequest, DEFAULT_MAX_FRAME_BYTES, EffectClass,
    EngineStageSpanV1, EngineStageTimelineV1, HandshakeAck, ProtocolLimits,
    RAW_WORKER_PROTOCOL_VERSION, RefOwnership, RevertMetadata, WorkerBinding, WorkerCapabilities,
    WorkerError, WorkerRequestFrame, WorkerResponseFrame, WorkerResult, WorkerResultMetadata,
    WorkerTokenAccountingV1, WorkerTokenCountKind, decode_request_frame, encode_frame,
    raw_worker_protocol_digest_hex,
};
use zero_codemode::session::{SESSION_SHUTDOWN_TOKEN_ENV, SESSION_TOKEN_ENV};
use zero_codemode::worker::{ENGINE_ENV, SESSION_ID_ENV, STORE_ROOT_ENV};

fn main() {
    for name in [SESSION_TOKEN_ENV, SESSION_SHUTDOWN_TOKEN_ENV] {
        if std::env::var_os(name).is_some() {
            eprintln!("session capability leaked into raw worker: {name}");
            std::process::exit(70);
        }
    }
    let mode = std::env::args().nth(1).unwrap_or_else(|| "normal".into());
    if mode == "tree-descendant" {
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    if mode == "stop-reading-startup" {
        spawn_descendant();
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    if mode == "spawn-descendant-normal" {
        spawn_descendant();
    }
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    let mut pending: Option<CallRequest> = None;
    let mut cancellation_race_done = false;
    for line in stdin.lock().lines() {
        let line = line.expect("read request");
        let frame = decode_request_frame(line.as_bytes(), DEFAULT_MAX_FRAME_BYTES)
            .expect("valid fixture request");
        match frame {
            WorkerRequestFrame::Handshake { request } => {
                let mut engine = parse_engine();
                let mut contract = request.expected_contract_digest.clone();
                let mut registry = request.expected_registry_digest.clone().unwrap();
                let mut revision = request.expected_worker_revision.clone().unwrap();
                let mut scheme = ref_scheme(engine).to_owned();
                match mode.as_str() {
                    "skew-engine" => {
                        engine = match engine {
                            EngineIdentity::FsZero => EngineIdentity::GraphZero,
                            _ => EngineIdentity::FsZero,
                        };
                        scheme = ref_scheme(engine).to_owned();
                    }
                    "skew-contract" => contract = "c".repeat(64),
                    "skew-registry" => registry = "d".repeat(64),
                    "skew-revision" => revision = "wrong-revision".into(),
                    "skew-ref" => scheme = "wrong://".into(),
                    _ => {}
                }
                send(
                    &mut stdout,
                    &WorkerResponseFrame::HandshakeAck {
                        ack: HandshakeAck {
                            protocol_version: RAW_WORKER_PROTOCOL_VERSION.into(),
                            binding: WorkerBinding {
                                engine,
                                root: std::env::var(STORE_ROOT_ENV).unwrap(),
                                session_id: std::env::var(SESSION_ID_ENV).unwrap(),
                                worker_revision: revision,
                                semantic_contract_version: "fixture.v1".into(),
                                semantic_contract_digest: contract,
                                operation_registry_digest: registry,
                                ref_scheme: scheme,
                            },
                            capabilities: WorkerCapabilities {
                                cancellation: true,
                                deadlines: true,
                                approvals: true,
                                revert: true,
                                snapshots: true,
                            },
                            limits: ProtocolLimits::default(),
                            protocol_digest: raw_worker_protocol_digest_hex(),
                        },
                    },
                );
                if mode == "inherited-open" {
                    spawn_descendant();
                }
                if mode == "stop-reading-after-handshake" {
                    spawn_descendant();
                    std::thread::sleep(Duration::from_secs(30));
                    return;
                }
            }
            WorkerRequestFrame::Call { request } => {
                if mode.starts_with("tree-") {
                    spawn_descendant();
                }
                if mode == "hold"
                    || mode == "tree-cancel"
                    || mode == "ignore-cancel"
                    || mode == "cancel-false-result"
                    || (!cancellation_race_done
                        && matches!(
                            mode.as_str(),
                            "result-first-cancel-false" | "error-first-cancel-false"
                        ))
                {
                    assert!(pending.replace(request).is_none(), "only one pending call");
                    continue;
                }
                if mode == "spin" {
                    loop {
                        std::hint::spin_loop();
                    }
                }
                if mode == "crash" || mode == "tree-crash" {
                    eprintln!("fixture crash");
                    std::process::exit(17);
                }
                if mode == "sleep" || mode == "tree-deadline" {
                    std::thread::sleep(Duration::from_secs(2));
                }
                if mode == "large-output" {
                    writeln!(stdout, "{}", "x".repeat(2048)).unwrap();
                    stdout.flush().unwrap();
                    continue;
                }
                if mode == "malformed" {
                    writeln!(stdout, "not-json").unwrap();
                    stdout.flush().unwrap();
                    continue;
                }
                if mode == "remote-error" {
                    let (engine_timeline, worker_token_accounting) =
                        transport_telemetry(&request, &mode);
                    send(
                        &mut stdout,
                        &WorkerResponseFrame::Error {
                            request_id: Some(request.request_id.clone()),
                            error: WorkerError {
                                kind: "fixture".into(),
                                message: "remote error".into(),
                                retryable: false,
                                details: Some(json!({"fixture_detail":"preserved"})),
                            },
                            trace: Some(request.trace),
                            engine_timeline,
                            worker_token_accounting,
                        },
                    );
                    continue;
                }
                if mode == "mismatch-error" {
                    send(
                        &mut stdout,
                        &WorkerResponseFrame::Error {
                            request_id: Some("wrong-request".into()),
                            error: WorkerError {
                                kind: "fixture".into(),
                                message: "wrong id".into(),
                                retryable: false,
                                details: None,
                            },
                            trace: Some(request.trace),
                            engine_timeline: None,
                            worker_token_accounting: None,
                        },
                    );
                    continue;
                }
                if mode == "mismatch-error-trace" {
                    let mut trace = request.trace;
                    trace.request_id = "wrong-request".into();
                    send(
                        &mut stdout,
                        &WorkerResponseFrame::Error {
                            request_id: Some(request.request_id.clone()),
                            error: WorkerError {
                                kind: "fixture".into(),
                                message: "wrong trace id".into(),
                                retryable: false,
                                details: None,
                            },
                            trace: Some(trace),
                            engine_timeline: None,
                            worker_token_accounting: None,
                        },
                    );
                    continue;
                }
                let response_id = if mode == "mismatch-result" {
                    "wrong-request".into()
                } else {
                    request.request_id.clone()
                };
                let (engine_timeline, worker_token_accounting) =
                    transport_telemetry(&request, &mode);
                send(
                    &mut stdout,
                    &WorkerResponseFrame::Result {
                        request_id: response_id,
                        result: result(request),
                        engine_timeline,
                        worker_token_accounting,
                    },
                );
            }
            WorkerRequestFrame::Cancel { request } => {
                if mode == "ignore-cancel" {
                    continue;
                }
                if !cancellation_race_done
                    && matches!(
                        mode.as_str(),
                        "result-first-cancel-false" | "error-first-cancel-false"
                    )
                {
                    let call = pending.take().expect("pending cancellation race");
                    if mode == "result-first-cancel-false" {
                        let request_id = call.request_id.clone();
                        let (engine_timeline, worker_token_accounting) =
                            transport_telemetry(&call, &mode);
                        send(
                            &mut stdout,
                            &WorkerResponseFrame::Result {
                                request_id,
                                result: result(call),
                                engine_timeline,
                                worker_token_accounting,
                            },
                        );
                    } else {
                        let (engine_timeline, worker_token_accounting) =
                            transport_telemetry(&call, &mode);
                        send(
                            &mut stdout,
                            &WorkerResponseFrame::Error {
                                request_id: Some(call.request_id.clone()),
                                error: WorkerError {
                                    kind: "fixture".into(),
                                    message: "remote error before cancel ack".into(),
                                    retryable: false,
                                    details: Some(json!({"fixture_detail":"preserved"})),
                                },
                                trace: Some(call.trace),
                                engine_timeline,
                                worker_token_accounting,
                            },
                        );
                    }
                    send(
                        &mut stdout,
                        &WorkerResponseFrame::CancelAck {
                            request_id: request.request_id,
                            cancelled: false,
                        },
                    );
                    cancellation_race_done = true;
                    continue;
                }
                if mode == "cancel-false-result" {
                    send(
                        &mut stdout,
                        &WorkerResponseFrame::CancelAck {
                            request_id: request.request_id,
                            cancelled: false,
                        },
                    );
                    if let Some(call) = pending.take() {
                        let request_id = call.request_id.clone();
                        let (engine_timeline, worker_token_accounting) =
                            transport_telemetry(&call, &mode);
                        send(
                            &mut stdout,
                            &WorkerResponseFrame::Result {
                                request_id,
                                result: result(call),
                                engine_timeline,
                                worker_token_accounting,
                            },
                        );
                    }
                    continue;
                }
                let cancelled = pending
                    .as_ref()
                    .is_some_and(|call| call.request_id == request.request_id);
                if cancelled {
                    pending = None;
                }
                let response_id = if mode == "mismatch-cancel" {
                    "wrong-request".into()
                } else {
                    request.request_id
                };
                send(
                    &mut stdout,
                    &WorkerResponseFrame::CancelAck {
                        request_id: response_id,
                        cancelled,
                    },
                );
            }
            WorkerRequestFrame::Shutdown { .. } => {
                send(&mut stdout, &WorkerResponseFrame::ShutdownAck);
                break;
            }
        }
    }
}

fn transport_telemetry(
    request: &CallRequest,
    mode: &str,
) -> (
    Option<EngineStageTimelineV1>,
    Option<WorkerTokenAccountingV1>,
) {
    let requested_timeline = request
        .telemetry_request
        .as_ref()
        .is_some_and(|value| value.engine_stage_timeline);
    let requested_accounting = request
        .telemetry_request
        .as_ref()
        .is_some_and(|value| value.worker_token_accounting);
    let emit_timeline =
        mode == "unsolicited-telemetry" || (mode != "omit-telemetry" && requested_timeline);
    let emit_accounting =
        mode == "unsolicited-telemetry" || (mode != "omit-telemetry" && requested_accounting);
    let timeline = emit_timeline.then(|| {
        let total_ns = if mode == "unclosable-telemetry" {
            1_000_000_000
        } else {
            300
        };
        EngineStageTimelineV1 {
            total_ns,
            spans: vec![
                EngineStageSpanV1 {
                    stage: "fixture_decode".into(),
                    start_ns: 0,
                    duration_ns: 100,
                },
                EngineStageSpanV1 {
                    stage: "fixture_execute".into(),
                    start_ns: 100,
                    duration_ns: total_ns - 100,
                },
            ],
        }
    });
    let accounting = emit_accounting.then(|| WorkerTokenAccountingV1 {
        tokenizer_id: "fixture-tokenizer-v1".into(),
        count_kind: WorkerTokenCountKind::Exact,
        raw_tokens: 8,
        visible_tokens: 4,
        recovery_tokens: 0,
        billed_tokens: 8,
        cached_tokens: 2,
        exact_ref_tokens: Some(0),
    });
    (timeline, accounting)
}

fn result(request: CallRequest) -> WorkerResult {
    let engine = parse_engine();
    let expose_approval = request.args["__approval_fixture"] == true;
    let approval_grant = request.approval_grant.clone();
    let (mut value, refs) = opaque_chain_result(engine, &request).unwrap_or_else(|| {
        (
            json!({
                "args": request.args,
                "store_root": std::env::var(STORE_ROOT_ENV).unwrap(),
                "session_id": std::env::var(SESSION_ID_ENV).unwrap(),
            }),
            Vec::new(),
        )
    });
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
                engine,
                session_id: std::env::var(SESSION_ID_ENV).unwrap(),
                refs,
                snapshot: None,
            },
            trace: request.trace,
        },
    }
}

fn opaque_chain_result(
    engine: EngineIdentity,
    request: &CallRequest,
) -> Option<(Value, Vec<String>)> {
    let root = opaque_chain_root();
    match (engine, request.op.as_str()) {
        (EngineIdentity::FsZero, "fs.read") if request.args["__opaque_chain_fixture"] == true => {
            let payload_hex = request.args["payload_hex"]
                .as_str()
                .expect("opaque fixture requires payload_hex");
            let bytes = decode_hex(payload_hex).expect("opaque fixture payload_hex must be exact");
            let digest = sha256_hex(&bytes);
            let reference = format!("fz://blob/{digest}");
            write_exact(&root.join("bytes").join(&digest), &bytes);
            Some((
                json!({"ref":reference,"sha256":digest,"length":bytes.len()}),
                vec![reference],
            ))
        }
        (EngineIdentity::GraphZero, "remember")
            if request.args["__opaque_chain_fixture"] == true =>
        {
            let source = request.args["source_ref"]
                .as_str()
                .expect("opaque fixture requires source_ref");
            let source_digest = canonical_ref_digest(source, "fz://blob/")
                .expect("opaque fixture requires a canonical fz ref");
            assert!(
                root.join("bytes").join(source_digest).is_file(),
                "opaque fixture fz object is unavailable"
            );
            let digest = sha256_hex(source.as_bytes());
            let reference = format!("gz://blob/{digest}");
            write_exact(
                &root.join("graph").join(format!("{digest}.ref")),
                source.as_bytes(),
            );
            Some((json!({"ref":reference}), vec![reference]))
        }
        (EngineIdentity::TokenZero, "ingest") => {
            let envelope: Value = serde_json::from_str(request.args["text"].as_str()?).ok()?;
            if envelope["__opaque_chain_fixture"] != true {
                return None;
            }
            let source = envelope["source_ref"]
                .as_str()
                .expect("opaque fixture requires source_ref");
            let source_digest = canonical_ref_digest(source, "gz://blob/")
                .expect("opaque fixture requires a canonical gz ref");
            assert!(
                root.join("graph")
                    .join(format!("{source_digest}.ref"))
                    .is_file(),
                "opaque fixture gz object is unavailable"
            );
            let digest = sha256_hex(source.as_bytes());
            let reference = format!("tz://blob/{digest}");
            write_exact(
                &root.join("token").join(format!("{digest}.ref")),
                source.as_bytes(),
            );
            Some((json!({"ref":reference}), vec![reference]))
        }
        (EngineIdentity::TokenZero, "expand") => {
            let reference = request.args["ref"].as_str()?;
            let token_digest = canonical_ref_digest(reference, "tz://blob/")?;
            let token_path = root.join("token").join(format!("{token_digest}.ref"));
            if !token_path.exists() {
                return None;
            }
            let graph_ref = fs::read_to_string(token_path).expect("read opaque fixture tz object");
            assert_eq!(
                sha256_hex(graph_ref.as_bytes()),
                token_digest,
                "opaque fixture tz descriptor digest mismatch"
            );
            let graph_digest = canonical_ref_digest(&graph_ref, "gz://blob/")
                .expect("opaque fixture tz object must contain a canonical gz ref");
            let fs_ref = fs::read_to_string(root.join("graph").join(format!("{graph_digest}.ref")))
                .expect("read opaque fixture gz object");
            assert_eq!(
                sha256_hex(fs_ref.as_bytes()),
                graph_digest,
                "opaque fixture gz descriptor digest mismatch"
            );
            let payload_digest = canonical_ref_digest(&fs_ref, "fz://blob/")
                .expect("opaque fixture gz object must contain a canonical fz ref");
            let bytes = fs::read(root.join("bytes").join(payload_digest))
                .expect("read opaque fixture fz object");
            assert_eq!(
                sha256_hex(&bytes),
                payload_digest,
                "opaque fixture payload digest mismatch"
            );
            Some((
                json!({
                    "payload_hex":encode_hex(&bytes),
                    "sha256":payload_digest,
                    "length":bytes.len(),
                }),
                vec![reference.to_owned()],
            ))
        }
        _ => None,
    }
}

fn opaque_chain_root() -> PathBuf {
    Path::new(&std::env::var(STORE_ROOT_ENV).unwrap()).join(".zerostack-opaque-chain-fixture")
}

fn write_exact(path: &Path, bytes: &[u8]) {
    if let Ok(existing) = fs::read(path) {
        assert_eq!(existing, bytes, "opaque fixture digest collision");
        return;
    }
    fs::create_dir_all(path.parent().unwrap()).expect("create opaque fixture directory");
    fs::write(path, bytes).expect("write opaque fixture object");
}

fn canonical_ref_digest<'a>(reference: &'a str, prefix: &str) -> Option<&'a str> {
    let digest = reference.strip_prefix(prefix)?;
    (digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    .then_some(digest)
}

fn sha256_hex(bytes: &[u8]) -> String {
    encode_hex(&Sha256::digest(bytes))
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[allow(
    clippy::manual_is_multiple_of,
    reason = "integer is_multiple_of requires Rust 1.87; workspace MSRV is 1.85"
)]
fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect()
}

#[allow(
    clippy::zombie_processes,
    reason = "fixture deliberately leaves a descendant for process-tree reap mutants"
)]
fn spawn_descendant() {
    let child = Command::new(std::env::current_exe().unwrap())
        .arg("tree-descendant")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    if let Ok(path) = std::env::var("ZEROSTACK_DESCENDANT_PID_FILE") {
        std::fs::write(path, child.id().to_string()).unwrap();
    }
}

fn parse_engine() -> EngineIdentity {
    match std::env::var(ENGINE_ENV).unwrap().as_str() {
        "fszero" => EngineIdentity::FsZero,
        "graphzero" => EngineIdentity::GraphZero,
        "tokenzero" => EngineIdentity::TokenZero,
        other => panic!("unknown engine {other}"),
    }
}

fn ref_scheme(engine: EngineIdentity) -> &'static str {
    match engine {
        EngineIdentity::FsZero => "fz://",
        EngineIdentity::GraphZero => "gz://",
        EngineIdentity::TokenZero => "tz://",
    }
}

fn send(stdout: &mut impl Write, frame: &WorkerResponseFrame) {
    stdout
        .write_all(&encode_frame(frame, DEFAULT_MAX_FRAME_BYTES).unwrap())
        .unwrap();
    stdout.flush().unwrap();
}
