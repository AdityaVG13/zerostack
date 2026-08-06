#![forbid(unsafe_code)]

use std::io::{self, BufRead, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::json;
use zero_abi::raw_worker::EngineIdentity;
use zero_abi::{
    ApprovalMetadata, ApprovalState, CallRequest, DEFAULT_MAX_FRAME_BYTES, EffectClass,
    HandshakeAck, ProtocolLimits, RAW_WORKER_PROTOCOL_VERSION, RefOwnership, RevertMetadata,
    WorkerBinding, WorkerCapabilities, WorkerError, WorkerRequestFrame, WorkerResponseFrame,
    WorkerResult, WorkerResultMetadata, decode_request_frame, encode_frame,
    raw_worker_protocol_digest_hex,
};
use zero_codemode::worker::{ENGINE_ENV, SESSION_ID_ENV, STORE_ROOT_ENV};

fn main() {
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
                    send(
                        &mut stdout,
                        &WorkerResponseFrame::Error {
                            request_id: Some(request.request_id.clone()),
                            error: WorkerError {
                                kind: "fixture".into(),
                                message: "remote error".into(),
                                retryable: false,
                                details: None,
                            },
                            trace: Some(request.trace),
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
                        },
                    );
                    continue;
                }
                let response_id = if mode == "mismatch-result" {
                    "wrong-request".into()
                } else {
                    request.request_id.clone()
                };
                send(
                    &mut stdout,
                    &WorkerResponseFrame::Result {
                        request_id: response_id,
                        result: result(request),
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
                        send(
                            &mut stdout,
                            &WorkerResponseFrame::Result {
                                request_id,
                                result: result(call),
                            },
                        );
                    } else {
                        send(
                            &mut stdout,
                            &WorkerResponseFrame::Error {
                                request_id: Some(call.request_id.clone()),
                                error: WorkerError {
                                    kind: "fixture".into(),
                                    message: "remote error before cancel ack".into(),
                                    retryable: false,
                                    details: None,
                                },
                                trace: Some(call.trace),
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
                        send(
                            &mut stdout,
                            &WorkerResponseFrame::Result {
                                request_id,
                                result: result(call),
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

fn result(request: CallRequest) -> WorkerResult {
    WorkerResult {
        value: json!({
            "args": request.args,
            "store_root": std::env::var(STORE_ROOT_ENV).unwrap(),
            "session_id": std::env::var(SESSION_ID_ENV).unwrap(),
        }),
        metadata: WorkerResultMetadata {
            effect: EffectClass::ReadOnly,
            approval: ApprovalMetadata {
                state: ApprovalState::NotRequired,
                approval_id: None,
                policy: None,
            },
            revert: RevertMetadata {
                supported: false,
                journal_id: None,
                rollback_op: None,
            },
            ownership: RefOwnership {
                engine: parse_engine(),
                session_id: std::env::var(SESSION_ID_ENV).unwrap(),
                refs: Vec::new(),
                snapshot: None,
            },
            trace: request.trace,
        },
    }
}

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
