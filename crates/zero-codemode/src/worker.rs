//! Harness-neutral raw-worker v2 process ownership and dispatch.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use command_group::{CommandGroup, GroupChild};

use zero_abi::raw_worker::EngineIdentity;
use zero_abi::{
    CallRequest, CancelRequest, EngineStageTimelineV1, FrameCodecError, HandshakeAck,
    HandshakeRequest, ProtocolLimits, RAW_WORKER_PROTOCOL_VERSION, ShutdownRequest,
    TIMELINE_CLOSURE_TOLERANCE_NS_V1, TelemetryRequestV1, WorkerRequestFrame, WorkerResponseFrame,
    WorkerResult, WorkerTokenAccountingV1, decode_response_frame, encode_frame,
    raw_worker_protocol_digest_hex, validate_handshake_request,
};

pub const STORE_ROOT_ENV: &str = "ZEROSTACK_STORE_ROOT";
pub const SESSION_ID_ENV: &str = "ZEROSTACK_SESSION_ID";
pub const ENGINE_ENV: &str = "ZEROSTACK_ENGINE";
const GRAPHZERO_REPO_ENV: &str = "GRAPHZERO_REPO";
const CANCEL_POLL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerContext {
    pub engine: EngineIdentity,
    pub store_root: PathBuf,
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerSpec {
    pub engine: EngineIdentity,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub store_root: PathBuf,
    pub session_id: String,
    pub expected_worker_revision: String,
    pub expected_contract_digest: String,
    pub expected_registry_digest: String,
}

impl WorkerSpec {
    fn handshake(&self) -> HandshakeRequest {
        HandshakeRequest {
            protocol_version: RAW_WORKER_PROTOCOL_VERSION.into(),
            root: self.store_root.to_str().expect("validated UTF-8").into(),
            session_id: self.session_id.clone(),
            expected_engine: self.engine,
            expected_worker_revision: Some(self.expected_worker_revision.clone()),
            expected_contract_digest: self.expected_contract_digest.clone(),
            expected_registry_digest: Some(self.expected_registry_digest.clone()),
        }
    }
}

pub trait WorkerFactory: Send + Sync {
    fn spec(&self, context: &WorkerContext) -> Result<WorkerSpec, WorkerAdapterError>;
}

#[derive(Clone)]
pub struct StaticWorkerFactory {
    program: PathBuf,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    revision: String,
    contract_digest: String,
    registry_digest: String,
}

impl StaticWorkerFactory {
    pub fn new(
        program: impl Into<PathBuf>,
        revision: impl Into<String>,
        contract_digest: impl Into<String>,
        registry_digest: impl Into<String>,
    ) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            revision: revision.into(),
            contract_digest: contract_digest.into(),
            registry_digest: registry_digest.into(),
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }
}

impl WorkerFactory for StaticWorkerFactory {
    fn spec(&self, context: &WorkerContext) -> Result<WorkerSpec, WorkerAdapterError> {
        let mut env = self.env.clone();
        if context.engine == EngineIdentity::GraphZero {
            let root = context.store_root.to_str().ok_or_else(|| {
                WorkerAdapterError::Configuration("store_root must be valid UTF-8".into())
            })?;
            env.insert(GRAPHZERO_REPO_ENV.into(), root.into());
        }
        Ok(WorkerSpec {
            engine: context.engine,
            program: self.program.clone(),
            args: self.args.clone(),
            env,
            store_root: context.store_root.clone(),
            session_id: context.session_id.clone(),
            expected_worker_revision: self.revision.clone(),
            expected_contract_digest: self.contract_digest.clone(),
            expected_registry_digest: self.registry_digest.clone(),
        })
    }
}

#[derive(Default)]
pub struct WorkerRegistry {
    factories: BTreeMap<EngineIdentity, Arc<dyn WorkerFactory>>,
}

impl WorkerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        engine: EngineIdentity,
        factory: Arc<dyn WorkerFactory>,
    ) -> Result<(), WorkerAdapterError> {
        if self.factories.contains_key(&engine) {
            return Err(WorkerAdapterError::DuplicateRegistration(engine));
        }
        self.factories.insert(engine, factory);
        Ok(())
    }

    pub fn registered(&self) -> impl Iterator<Item = EngineIdentity> + '_ {
        self.factories.keys().copied()
    }

    pub fn launch(
        &self,
        context: WorkerContext,
        config: WorkerClientConfig,
    ) -> Result<WorkerClient, WorkerAdapterError> {
        let factory = self
            .factories
            .get(&context.engine)
            .ok_or(WorkerAdapterError::UnknownRegistration(context.engine))?;
        WorkerClient::spawn(factory.spec(&context)?, config)
    }
}

#[derive(Clone)]
pub struct WorkerClientConfig {
    pub limits: ProtocolLimits,
    pub handshake_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub max_stderr_bytes: usize,
    pub observer: Option<Arc<dyn Fn(&WorkerObservation) + Send + Sync>>,
}

impl Default for WorkerClientConfig {
    fn default() -> Self {
        Self {
            limits: ProtocolLimits::default(),
            handshake_timeout: Duration::from_secs(5),
            shutdown_timeout: Duration::from_secs(2),
            max_stderr_bytes: 65_536,
            observer: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerEvent {
    Started,
    Handshake,
    Dispatch,
    Cancel,
    Shutdown,
    Crash,
    BoundsError,
    Deadline,
    ProtocolError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerSettlementReceiptV1 {
    pub raw_worker_result_settlement_ns: u64,
    pub residual_transport_ns: u64,
    pub total_ns: u64,
    pub closure_error_ns: u64,
    pub engine_timeline: Option<EngineStageTimelineV1>,
    pub worker_token_accounting: Option<WorkerTokenAccountingV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerObservation {
    pub event: WorkerEvent,
    pub engine: EngineIdentity,
    pub request_id: Option<String>,
    pub elapsed_ms: u64,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub stderr_bytes: u64,
    pub settlement: Option<WorkerSettlementReceiptV1>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerAccounting {
    pub requests: u64,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub stderr_bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub struct CancellationSignal(Arc<AtomicBool>);

impl CancellationSignal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
pub enum WorkerAdapterError {
    Configuration(String),
    DuplicateRegistration(EngineIdentity),
    UnknownRegistration(EngineIdentity),
    Spawn(std::io::Error),
    Io(std::io::Error),
    WriterBusy,
    WriterTimeout,
    WriterClosed,
    Protocol(FrameCodecError),
    Handshake(String),
    Bounds {
        stream: &'static str,
        actual: usize,
        maximum: usize,
    },
    Deadline {
        request_id: Option<String>,
    },
    DeadlineOverflow {
        request_id: Option<String>,
    },
    Cancelled {
        request_id: String,
    },
    Crash {
        status: Option<ExitStatus>,
        stderr: StderrCapture,
    },
    Remote {
        request_id: Option<String>,
        kind: String,
        message: String,
        retryable: bool,
        details: Option<Box<serde_json::Value>>,
        trace: Option<Box<zero_abi::WorkerTrace>>,
    },
}

impl fmt::Display for WorkerAdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(v) | Self::Handshake(v) => f.write_str(v),
            Self::DuplicateRegistration(e) => {
                write!(f, "duplicate worker registration: {}", e.as_str())
            }
            Self::UnknownRegistration(e) => {
                write!(f, "unknown worker registration: {}", e.as_str())
            }
            Self::Spawn(e) | Self::Io(e) => write!(f, "worker I/O: {e}"),
            Self::WriterBusy => f.write_str("worker stdin queue is full"),
            Self::WriterTimeout => f.write_str("worker stdin write acknowledgement timed out"),
            Self::WriterClosed => f.write_str("worker stdin writer is closed"),
            Self::Protocol(e) => write!(f, "worker protocol: {e}"),
            Self::Bounds {
                stream,
                actual,
                maximum,
            } => write!(f, "worker {stream} exceeded bound: {actual} > {maximum}"),
            Self::Deadline { request_id } => write!(f, "worker deadline exceeded: {request_id:?}"),
            Self::DeadlineOverflow { request_id } => {
                write!(f, "worker deadline cannot be represented: {request_id:?}")
            }
            Self::Cancelled { request_id } => write!(f, "worker request cancelled: {request_id}"),
            Self::Crash { status, stderr } => write!(f, "worker crashed ({status:?}): {stderr}"),
            Self::Remote { kind, message, .. } => write!(f, "worker error {kind}: {message}"),
        }
    }
}

impl std::error::Error for WorkerAdapterError {}

enum OutputEvent {
    Frame { bytes: Vec<u8>, arrived_at: Instant },
    Bounds(usize),
    Eof,
    Io(std::io::Error),
}

enum WriterRequest {
    Write {
        bytes: Vec<u8>,
        acknowledgement: mpsc::SyncSender<Result<(), std::io::Error>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StderrCapture {
    pub text: String,
    pub observed_bytes: u64,
    pub complete: bool,
    pub truncated: bool,
}

impl fmt::Display for StderrCapture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} [observed_bytes={}, complete={}, truncated={}]",
            self.text, self.observed_bytes, self.complete, self.truncated
        )
    }
}

#[derive(Default)]
struct StderrState {
    bytes: Vec<u8>,
    total: u64,
    complete: bool,
}

struct SettlementSeed {
    arrived_at: Instant,
    engine_timeline: Option<EngineStageTimelineV1>,
    worker_token_accounting: Option<WorkerTokenAccountingV1>,
}

pub struct WorkerClient {
    engine: EngineIdentity,
    child: GroupChild,
    writer: mpsc::SyncSender<WriterRequest>,
    output: mpsc::Receiver<OutputEvent>,
    stderr: Arc<(Mutex<StderrState>, Condvar)>,
    config: WorkerClientConfig,
    negotiated_limits: ProtocolLimits,
    accounting: WorkerAccounting,
    last_output_bytes: u64,
    last_frame_arrival: Option<Instant>,
    terminal_status: Option<ExitStatus>,
    terminal: bool,
    shutdown: bool,
}

impl WorkerClient {
    pub fn spawn(spec: WorkerSpec, config: WorkerClientConfig) -> Result<Self, WorkerAdapterError> {
        validate_spec(&spec)?;
        validate_config(&config)?;
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .envs(&spec.env)
            .env(STORE_ROOT_ENV, &spec.store_root)
            .env(SESSION_ID_ENV, &spec.session_id)
            .env(ENGINE_ENV, spec.engine.as_str())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.group_spawn().map_err(WorkerAdapterError::Spawn)?;
        let stdin = match child.inner().stdin.take() {
            Some(pipe) => pipe,
            None => {
                return Err(cleanup_partial(
                    &mut child,
                    config.shutdown_timeout,
                    "worker stdin unavailable",
                ));
            }
        };
        let stdout = match child.inner().stdout.take() {
            Some(pipe) => pipe,
            None => {
                return Err(cleanup_partial(
                    &mut child,
                    config.shutdown_timeout,
                    "worker stdout unavailable",
                ));
            }
        };
        let stderr = match child.inner().stderr.take() {
            Some(pipe) => pipe,
            None => {
                return Err(cleanup_partial(
                    &mut child,
                    config.shutdown_timeout,
                    "worker stderr unavailable",
                ));
            }
        };

        let (writer, writer_requests) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut stdin = stdin;
            while let Ok(WriterRequest::Write {
                bytes,
                acknowledgement,
            }) = writer_requests.recv()
            {
                let result = stdin.write_all(&bytes).and_then(|_| stdin.flush());
                let failed = result.is_err();
                let _ = acknowledgement.send(result);
                if failed {
                    break;
                }
            }
        });

        let (tx, output) = mpsc::sync_channel(1);
        let max_frame = config.limits.max_frame_bytes as usize;
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                match reader
                    .by_ref()
                    .take((max_frame.saturating_add(2)) as u64)
                    .read_until(b'\n', &mut line)
                {
                    Ok(0) => {
                        let _ = tx.send(OutputEvent::Eof);
                        break;
                    }
                    Ok(_) => {
                        let actual = line.strip_suffix(b"\n").unwrap_or(&line).len();
                        if actual > max_frame {
                            let _ = tx.send(OutputEvent::Bounds(actual));
                            break;
                        }
                        if tx
                            .send(OutputEvent::Frame {
                                bytes: line,
                                arrived_at: Instant::now(),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(OutputEvent::Io(error));
                        break;
                    }
                }
            }
        });

        let stderr_state = Arc::new((Mutex::new(StderrState::default()), Condvar::new()));
        let stderr_writer = stderr_state.clone();
        let stderr_max = config.max_stderr_bytes;
        let stderr_observer = config.observer.clone();
        let stderr_engine = spec.engine;
        thread::spawn(move || {
            let mut reader = stderr;
            let mut buf = [0_u8; 4096];
            let mut reported = false;
            loop {
                let count = match reader.read(&mut buf) {
                    Ok(0) => {
                        if let Ok(mut state) = stderr_writer.0.lock() {
                            state.complete = true;
                            stderr_writer.1.notify_all();
                        }
                        break;
                    }
                    Err(_) => {
                        stderr_writer.1.notify_all();
                        break;
                    }
                    Ok(count) => count,
                };
                let mut state = match stderr_writer.0.lock() {
                    Ok(state) => state,
                    Err(_) => break,
                };
                state.total = state.total.saturating_add(count as u64);
                let take = stderr_max.saturating_sub(state.bytes.len()).min(count);
                state.bytes.extend_from_slice(&buf[..take]);
                stderr_writer.1.notify_all();
                if state.total > stderr_max as u64 && !reported {
                    reported = true;
                    if let Some(observer) = &stderr_observer {
                        observer(&WorkerObservation {
                            event: WorkerEvent::BoundsError,
                            engine: stderr_engine,
                            request_id: None,
                            elapsed_ms: 0,
                            input_bytes: 0,
                            output_bytes: 0,
                            stderr_bytes: state.total,
                            settlement: None,
                        });
                    }
                }
            }
        });

        let mut client = Self {
            engine: spec.engine,
            child,
            writer,
            output,
            stderr: stderr_state,
            config: config.clone(),
            negotiated_limits: config.limits.clone(),
            accounting: WorkerAccounting::default(),
            last_output_bytes: 0,
            last_frame_arrival: None,
            terminal_status: None,
            terminal: false,
            shutdown: false,
        };
        client.observe(WorkerEvent::Started, None, Duration::ZERO, 0, 0);
        let request = spec.handshake();
        let started = Instant::now();
        let handshake_deadline = match checked_deadline(config.handshake_timeout, None) {
            Ok(deadline) => deadline,
            Err(error) => {
                client.kill_and_reap();
                return Err(error);
            }
        };
        if let Err(error) = client.send(
            &WorkerRequestFrame::Handshake {
                request: request.clone(),
            },
            handshake_deadline,
        ) {
            client.kill_and_reap();
            return Err(error);
        }
        let response = match client.receive_until(handshake_deadline, None) {
            Ok(response) => response,
            Err(error) => {
                client.kill_and_reap();
                return Err(error);
            }
        };
        let ack = match response {
            WorkerResponseFrame::HandshakeAck { ack } => ack,
            other => {
                client.kill_and_reap();
                return Err(WorkerAdapterError::Handshake(format!(
                    "expected handshake_ack, got {other:?}"
                )));
            }
        };
        if let Err(error) = client.validate_handshake(&request, &ack) {
            client.kill_and_reap();
            return Err(error);
        }
        client.negotiated_limits.max_frame_bytes = client
            .negotiated_limits
            .max_frame_bytes
            .min(ack.limits.max_frame_bytes);
        client.negotiated_limits.max_output_bytes = client
            .negotiated_limits
            .max_output_bytes
            .min(ack.limits.max_output_bytes);
        client.negotiated_limits.max_in_flight = 1;
        client.negotiated_limits.default_deadline_ms = client
            .negotiated_limits
            .default_deadline_ms
            .min(ack.limits.default_deadline_ms);
        client.observe(
            WorkerEvent::Handshake,
            None,
            started.elapsed(),
            0,
            client.last_output_bytes,
        );
        Ok(client)
    }

    fn validate_handshake(
        &self,
        request: &HandshakeRequest,
        ack: &HandshakeAck,
    ) -> Result<(), WorkerAdapterError> {
        validate_handshake_request(request, &ack.binding).map_err(WorkerAdapterError::Protocol)?;
        if ack.protocol_version != RAW_WORKER_PROTOCOL_VERSION {
            return Err(WorkerAdapterError::Handshake(
                "protocol version mismatch".into(),
            ));
        }
        if ack.protocol_digest != raw_worker_protocol_digest_hex() {
            return Err(WorkerAdapterError::Handshake(
                "protocol digest mismatch".into(),
            ));
        }
        if ack.binding.ref_scheme != ref_scheme(self.engine) {
            return Err(WorkerAdapterError::Handshake(format!(
                "ref_scheme mismatch: expected={} actual={}",
                ref_scheme(self.engine),
                ack.binding.ref_scheme
            )));
        }
        if ack.limits.max_frame_bytes == 0
            || ack.limits.max_output_bytes == 0
            || ack.limits.max_in_flight == 0
            || ack.limits.default_deadline_ms == 0
        {
            return Err(WorkerAdapterError::Handshake(
                "worker advertised zero protocol limit".into(),
            ));
        }
        Ok(())
    }

    pub fn dispatch(&mut self, request: CallRequest) -> Result<WorkerResult, WorkerAdapterError> {
        self.dispatch_inner(request, None)
    }

    pub fn dispatch_with_cancel(
        &mut self,
        request: CallRequest,
        cancellation: &CancellationSignal,
    ) -> Result<WorkerResult, WorkerAdapterError> {
        self.dispatch_inner(request, Some(cancellation))
    }

    fn dispatch_inner(
        &mut self,
        request: CallRequest,
        cancellation: Option<&CancellationSignal>,
    ) -> Result<WorkerResult, WorkerAdapterError> {
        if self.terminal {
            return Err(WorkerAdapterError::Configuration(
                "worker is terminal".into(),
            ));
        }
        if request.trace.request_id != request.request_id {
            return Err(WorkerAdapterError::Configuration(
                "request and trace ids differ".into(),
            ));
        }
        let now = unix_ms();
        if request
            .deadline_unix_ms
            .is_some_and(|deadline| deadline <= now)
        {
            return self.terminate_with(WorkerAdapterError::Deadline {
                request_id: Some(request.request_id),
            });
        }
        let timeout = request
            .deadline_unix_ms
            .map(|deadline| Duration::from_millis(deadline.saturating_sub(now)))
            .unwrap_or(Duration::from_millis(
                self.negotiated_limits.default_deadline_ms,
            ));
        let id = request.request_id.clone();
        let telemetry_request = request.telemetry_request.clone();
        let deadline = match checked_deadline(timeout, Some(id.clone())) {
            Ok(deadline) => deadline,
            Err(error) => return self.terminate_with(error),
        };
        let input = match self.send(&WorkerRequestFrame::Call { request }, deadline) {
            Ok(count) => count,
            Err(error) => return self.terminate_with(error),
        };
        let started = Instant::now();
        let mut cancel_sent = false;
        let mut cancel_rejected = false;
        let mut pending_completion: Option<(
            Result<WorkerResult, WorkerAdapterError>,
            u64,
            SettlementSeed,
        )> = None;
        loop {
            if cancellation.is_some_and(CancellationSignal::is_cancelled) && !cancel_sent {
                if let Err(error) = self.send(
                    &WorkerRequestFrame::Cancel {
                        request: CancelRequest {
                            request_id: id.clone(),
                            reason: Some("external cancellation".into()),
                        },
                    },
                    deadline,
                ) {
                    return self.terminate_with(error);
                }
                cancel_sent = true;
            }
            if Instant::now() >= deadline {
                self.observe(
                    WorkerEvent::Deadline,
                    Some(id.clone()),
                    started.elapsed(),
                    input,
                    0,
                );
                return self.terminate_with(WorkerAdapterError::Deadline {
                    request_id: Some(id),
                });
            }
            let wait = if cancellation.is_some() {
                CANCEL_POLL.min(deadline.saturating_duration_since(Instant::now()))
            } else {
                deadline.saturating_duration_since(Instant::now())
            };
            let response = match self.receive_once(wait, Some(id.clone())) {
                Ok(Some(response)) => response,
                Ok(None) => continue,
                Err(error) => return self.terminate_with(error),
            };
            match response {
                WorkerResponseFrame::Result {
                    request_id,
                    result,
                    engine_timeline,
                    worker_token_accounting,
                } if request_id == id => {
                    if let Some(message) = telemetry_response_mismatch(
                        telemetry_request.as_ref(),
                        engine_timeline.as_ref(),
                        worker_token_accounting.as_ref(),
                        true,
                    ) {
                        return self.protocol_terminate(message);
                    }
                    let Some(arrived_at) = self.last_frame_arrival.take() else {
                        return self
                            .protocol_terminate("result frame missing arrival timestamp".into());
                    };
                    let seed = SettlementSeed {
                        arrived_at,
                        engine_timeline,
                        worker_token_accounting,
                    };
                    let completion = Ok(result);
                    let completion_bytes = self.last_output_bytes;
                    if cancel_sent && !cancel_rejected {
                        if pending_completion
                            .replace((completion, completion_bytes, seed))
                            .is_some()
                        {
                            return self.protocol_terminate(
                                "duplicate completion before cancel acknowledgement".into(),
                            );
                        }
                        continue;
                    }
                    self.accounting.requests = self.accounting.requests.saturating_add(1);
                    let settlement = match finalize_settlement(seed, started) {
                        Ok(receipt) => receipt,
                        Err(message) => return self.protocol_terminate(message),
                    };
                    self.observe_with_settlement(
                        WorkerEvent::Dispatch,
                        Some(id),
                        started.elapsed(),
                        input,
                        self.last_output_bytes,
                        Some(settlement),
                    );
                    return completion;
                }
                WorkerResponseFrame::Error {
                    request_id: Some(request_id),
                    error,
                    trace,
                    engine_timeline,
                    worker_token_accounting,
                } if request_id == id => {
                    if trace.as_ref().is_some_and(|trace| trace.request_id != id) {
                        return self.protocol_terminate(
                            "mismatched dispatch error trace request id".into(),
                        );
                    }
                    if let Some(message) = telemetry_response_mismatch(
                        telemetry_request.as_ref(),
                        engine_timeline.as_ref(),
                        worker_token_accounting.as_ref(),
                        false,
                    ) {
                        return self.protocol_terminate(message);
                    }
                    let Some(arrived_at) = self.last_frame_arrival.take() else {
                        return self
                            .protocol_terminate("error frame missing arrival timestamp".into());
                    };
                    let seed = SettlementSeed {
                        arrived_at,
                        engine_timeline,
                        worker_token_accounting,
                    };
                    let completion = Err(WorkerAdapterError::Remote {
                        request_id: Some(request_id),
                        kind: error.kind,
                        message: error.message,
                        retryable: error.retryable,
                        details: error.details.map(Box::new),
                        trace: trace.map(Box::new),
                    });
                    let completion_bytes = self.last_output_bytes;
                    if cancel_sent && !cancel_rejected {
                        if pending_completion
                            .replace((completion, completion_bytes, seed))
                            .is_some()
                        {
                            return self.protocol_terminate(
                                "duplicate completion before cancel acknowledgement".into(),
                            );
                        }
                        continue;
                    }
                    self.accounting.requests = self.accounting.requests.saturating_add(1);
                    let settlement = match finalize_settlement(seed, started) {
                        Ok(receipt) => receipt,
                        Err(message) => return self.protocol_terminate(message),
                    };
                    self.observe_with_settlement(
                        WorkerEvent::Dispatch,
                        Some(id),
                        started.elapsed(),
                        input,
                        self.last_output_bytes,
                        Some(settlement),
                    );
                    return completion;
                }
                WorkerResponseFrame::CancelAck {
                    request_id,
                    cancelled: true,
                } if cancel_sent && request_id == id => {
                    self.observe(
                        WorkerEvent::Cancel,
                        Some(id.clone()),
                        started.elapsed(),
                        input,
                        self.last_output_bytes,
                    );
                    return self.terminate_with(WorkerAdapterError::Cancelled { request_id: id });
                }
                WorkerResponseFrame::CancelAck {
                    request_id,
                    cancelled: false,
                } if cancel_sent && request_id == id && !cancel_rejected => {
                    cancel_rejected = true;
                    if let Some((completion, completion_bytes, seed)) = pending_completion.take() {
                        self.accounting.requests = self.accounting.requests.saturating_add(1);
                        let settlement = match finalize_settlement(seed, started) {
                            Ok(receipt) => receipt,
                            Err(message) => return self.protocol_terminate(message),
                        };
                        self.observe_with_settlement(
                            WorkerEvent::Dispatch,
                            Some(id),
                            started.elapsed(),
                            input,
                            completion_bytes,
                            Some(settlement),
                        );
                        return completion;
                    }
                }
                other => {
                    return self.protocol_terminate(format!(
                        "mismatched or unexpected dispatch response: {other:?}"
                    ));
                }
            }
        }
    }

    pub fn cancel(
        &mut self,
        request_id: impl Into<String>,
        reason: Option<String>,
    ) -> Result<bool, WorkerAdapterError> {
        let id = request_id.into();
        let started = Instant::now();
        let deadline = match checked_deadline(
            Duration::from_millis(self.negotiated_limits.default_deadline_ms),
            Some(id.clone()),
        ) {
            Ok(deadline) => deadline,
            Err(error) => return self.terminate_with(error),
        };
        let input = match self.send(
            &WorkerRequestFrame::Cancel {
                request: CancelRequest {
                    request_id: id.clone(),
                    reason,
                },
            },
            deadline,
        ) {
            Ok(count) => count,
            Err(error) => return self.terminate_with(error),
        };
        let response = match self.receive_until(deadline, Some(id.clone())) {
            Ok(response) => response,
            Err(error) => return self.terminate_with(error),
        };
        match response {
            WorkerResponseFrame::CancelAck {
                request_id,
                cancelled,
            } if request_id == id => {
                self.observe(
                    WorkerEvent::Cancel,
                    Some(id),
                    started.elapsed(),
                    input,
                    self.last_output_bytes,
                );
                Ok(cancelled)
            }
            other => self.protocol_terminate(format!("mismatched cancel response: {other:?}")),
        }
    }

    pub fn accounting(&self) -> WorkerAccounting {
        let mut accounting = self.accounting.clone();
        accounting.stderr_bytes = self
            .stderr
            .0
            .lock()
            .map(|state| state.total)
            .unwrap_or(u64::MAX);
        accounting
    }

    pub fn negotiated_limits(&self) -> &ProtocolLimits {
        &self.negotiated_limits
    }
    pub fn process_id(&self) -> u32 {
        self.child.id()
    }
    pub fn terminal_status(&self) -> Option<ExitStatus> {
        self.terminal_status
    }
    pub fn is_reaped(&self) -> bool {
        self.terminal_status.is_some()
    }
    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    pub fn shutdown(&mut self) -> Result<(), WorkerAdapterError> {
        if self.terminal {
            self.shutdown = true;
            return if self.terminal_status.is_some() {
                Ok(())
            } else {
                Err(WorkerAdapterError::Deadline { request_id: None })
            };
        }
        if self.shutdown {
            return Ok(());
        }
        self.shutdown = true;
        let started = Instant::now();
        let deadline = match checked_deadline(self.config.shutdown_timeout, None) {
            Ok(deadline) => deadline,
            Err(error) => {
                self.kill_and_reap();
                return Err(error);
            }
        };
        let sent = self.send(
            &WorkerRequestFrame::Shutdown {
                request: ShutdownRequest {
                    reason: "client shutdown".into(),
                },
            },
            deadline,
        );
        if sent.is_ok() {
            if matches!(
                self.receive_until(deadline, None),
                Ok(WorkerResponseFrame::ShutdownAck)
            ) && self.reap_until(deadline)
            {
                self.observe(
                    WorkerEvent::Shutdown,
                    None,
                    started.elapsed(),
                    0,
                    self.last_output_bytes,
                );
                return Ok(());
            }
        }
        self.kill_and_reap();
        Err(WorkerAdapterError::Deadline { request_id: None })
    }

    fn send<T: serde::Serialize>(
        &mut self,
        frame: &T,
        deadline: Instant,
    ) -> Result<u64, WorkerAdapterError> {
        let bytes = match encode_frame(frame, self.negotiated_limits.max_frame_bytes as usize) {
            Ok(bytes) => bytes,
            Err(FrameCodecError::TooLarge { actual, maximum }) => {
                self.observe(
                    WorkerEvent::BoundsError,
                    None,
                    Duration::ZERO,
                    actual as u64,
                    0,
                );
                return Err(WorkerAdapterError::Bounds {
                    stream: "stdin",
                    actual,
                    maximum,
                });
            }
            Err(error) => return Err(WorkerAdapterError::Protocol(error)),
        };
        let count = bytes.len() as u64;
        let (acknowledgement, result) = mpsc::sync_channel(1);
        match self.writer.try_send(WriterRequest::Write {
            bytes,
            acknowledgement,
        }) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => return Err(WorkerAdapterError::WriterBusy),
            Err(mpsc::TrySendError::Disconnected(_)) => {
                return Err(WorkerAdapterError::WriterClosed);
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(WorkerAdapterError::WriterTimeout);
        }
        match result.recv_timeout(remaining) {
            Ok(Ok(())) => {
                self.accounting.input_bytes = self.accounting.input_bytes.saturating_add(count);
                Ok(count)
            }
            Ok(Err(error)) => Err(WorkerAdapterError::Io(error)),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(WorkerAdapterError::WriterTimeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(WorkerAdapterError::WriterClosed),
        }
    }

    fn receive_until(
        &mut self,
        deadline: Instant,
        request_id: Option<String>,
    ) -> Result<WorkerResponseFrame, WorkerAdapterError> {
        match self.receive_once(
            deadline.saturating_duration_since(Instant::now()),
            request_id.clone(),
        )? {
            Some(response) => Ok(response),
            None => Err(WorkerAdapterError::Deadline { request_id }),
        }
    }

    fn receive_once(
        &mut self,
        timeout: Duration,
        request_id: Option<String>,
    ) -> Result<Option<WorkerResponseFrame>, WorkerAdapterError> {
        match self.output.recv_timeout(timeout) {
            Ok(OutputEvent::Frame { bytes, arrived_at }) => {
                self.last_frame_arrival = Some(arrived_at);
                let payload = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
                let maximum =
                    self.negotiated_limits
                        .max_output_bytes
                        .min(self.negotiated_limits.max_frame_bytes) as usize;
                if payload.len() > maximum {
                    self.observe(
                        WorkerEvent::BoundsError,
                        request_id,
                        Duration::ZERO,
                        0,
                        payload.len() as u64,
                    );
                    return Err(WorkerAdapterError::Bounds {
                        stream: "stdout",
                        actual: payload.len(),
                        maximum,
                    });
                }
                self.last_output_bytes = bytes.len() as u64;
                self.accounting.output_bytes = self
                    .accounting
                    .output_bytes
                    .saturating_add(self.last_output_bytes);
                match decode_response_frame(&bytes, self.negotiated_limits.max_frame_bytes as usize)
                {
                    Ok(frame) => Ok(Some(frame)),
                    Err(FrameCodecError::TooLarge { actual, maximum }) => {
                        self.observe(
                            WorkerEvent::BoundsError,
                            request_id,
                            Duration::ZERO,
                            0,
                            actual as u64,
                        );
                        Err(WorkerAdapterError::Bounds {
                            stream: "stdout",
                            actual,
                            maximum,
                        })
                    }
                    Err(error) => {
                        self.observe(
                            WorkerEvent::ProtocolError,
                            request_id,
                            Duration::ZERO,
                            0,
                            self.last_output_bytes,
                        );
                        Err(WorkerAdapterError::Protocol(error))
                    }
                }
            }
            Ok(OutputEvent::Bounds(actual)) => {
                let maximum = self.negotiated_limits.max_frame_bytes as usize;
                self.observe(
                    WorkerEvent::BoundsError,
                    request_id,
                    Duration::ZERO,
                    0,
                    actual as u64,
                );
                Err(WorkerAdapterError::Bounds {
                    stream: "stdout",
                    actual,
                    maximum,
                })
            }
            Ok(OutputEvent::Io(error)) => Err(WorkerAdapterError::Io(error)),
            Ok(OutputEvent::Eof) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(self.crash_error())
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
        }
    }

    fn protocol_terminate<T>(&mut self, message: String) -> Result<T, WorkerAdapterError> {
        self.observe(
            WorkerEvent::ProtocolError,
            None,
            Duration::ZERO,
            0,
            self.last_output_bytes,
        );
        self.terminate_with(WorkerAdapterError::Handshake(message))
    }

    fn terminate_with<T>(&mut self, error: WorkerAdapterError) -> Result<T, WorkerAdapterError> {
        self.kill_and_reap();
        Err(error)
    }

    fn crash_error(&mut self) -> WorkerAdapterError {
        self.kill_and_reap();
        let stderr = self.stderr_capture_wait(Duration::from_millis(50));
        self.observe(WorkerEvent::Crash, None, Duration::ZERO, 0, 0);
        WorkerAdapterError::Crash {
            status: self.terminal_status,
            stderr,
        }
    }

    pub fn stderr_capture(&self) -> StderrCapture {
        self.stderr_capture_wait(Duration::ZERO)
    }

    fn stderr_capture_wait(&self, timeout: Duration) -> StderrCapture {
        let (lock, ready) = &*self.stderr;
        let mut state = match lock.lock() {
            Ok(state) => state,
            Err(_) => {
                return StderrCapture {
                    text: String::new(),
                    observed_bytes: u64::MAX,
                    complete: false,
                    truncated: true,
                };
            }
        };
        let deadline = Instant::now().checked_add(timeout);
        while !state.complete && !timeout.is_zero() {
            let Some(deadline) = deadline else { break };
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match ready.wait_timeout(state, remaining) {
                Ok((next, result)) => {
                    state = next;
                    if result.timed_out() {
                        break;
                    }
                }
                Err(_) => {
                    return StderrCapture {
                        text: String::new(),
                        observed_bytes: u64::MAX,
                        complete: false,
                        truncated: true,
                    };
                }
            }
        }
        StderrCapture {
            text: String::from_utf8_lossy(&state.bytes).into_owned(),
            observed_bytes: state.total,
            complete: state.complete,
            truncated: state.total > state.bytes.len() as u64,
        }
    }

    fn reap_until(&mut self, deadline: Instant) -> bool {
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    let _ = self.child.kill();
                    self.terminal_status = Some(status);
                    self.terminal = true;
                    return true;
                }
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                Ok(None) | Err(_) => return false,
            }
        }
    }

    fn kill_and_reap(&mut self) {
        self.terminal = true;
        if self.terminal_status.is_some() {
            let _ = self.child.kill();
            return;
        }
        terminate_process_tree(&mut self.child);
        if let Ok(status) = self.child.wait() {
            self.terminal_status = Some(status);
        }
    }

    fn observe(
        &self,
        event: WorkerEvent,
        request_id: Option<String>,
        elapsed: Duration,
        input_bytes: u64,
        output_bytes: u64,
    ) {
        self.observe_with_settlement(event, request_id, elapsed, input_bytes, output_bytes, None);
    }

    fn observe_with_settlement(
        &self,
        event: WorkerEvent,
        request_id: Option<String>,
        elapsed: Duration,
        input_bytes: u64,
        output_bytes: u64,
        settlement: Option<WorkerSettlementReceiptV1>,
    ) {
        if let Some(observer) = &self.config.observer {
            let stderr_bytes = self
                .stderr
                .0
                .lock()
                .map(|state| state.total)
                .unwrap_or(u64::MAX);
            observer(&WorkerObservation {
                event,
                engine: self.engine,
                request_id,
                elapsed_ms: elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
                input_bytes,
                output_bytes,
                stderr_bytes,
                settlement,
            });
        }
    }
}

impl Drop for WorkerClient {
    fn drop(&mut self) {
        if !self.terminal {
            let _ = self.shutdown();
        }
        if self.terminal_status.is_none() {
            self.kill_and_reap();
        } else {
            let _ = self.child.kill();
        }
    }
}

fn validate_spec(spec: &WorkerSpec) -> Result<(), WorkerAdapterError> {
    if spec.program.as_os_str().is_empty() {
        return Err(WorkerAdapterError::Configuration(
            "worker program must be non-empty".into(),
        ));
    }
    let root = spec.store_root.to_str().ok_or_else(|| {
        WorkerAdapterError::Configuration("store_root must be valid UTF-8".into())
    })?;
    if root.is_empty() || spec.session_id.is_empty() {
        return Err(WorkerAdapterError::Configuration(
            "store_root and session_id must be non-empty".into(),
        ));
    }
    if spec.session_id.contains('\0') {
        return Err(WorkerAdapterError::Configuration(
            "session_id must not contain NUL".into(),
        ));
    }
    if spec.expected_worker_revision.is_empty() {
        return Err(WorkerAdapterError::Configuration(
            "expected_worker_revision must be non-empty".into(),
        ));
    }
    for (field, digest) in [
        ("expected_contract_digest", &spec.expected_contract_digest),
        ("expected_registry_digest", &spec.expected_registry_digest),
    ] {
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(WorkerAdapterError::Configuration(format!(
                "{field} must be a 64-character lowercase hex digest"
            )));
        }
    }
    Ok(())
}

fn validate_config(config: &WorkerClientConfig) -> Result<(), WorkerAdapterError> {
    if config.limits.max_frame_bytes == 0
        || config.limits.max_output_bytes == 0
        || config.limits.max_in_flight == 0
        || config.limits.default_deadline_ms == 0
        || config.handshake_timeout.is_zero()
        || config.shutdown_timeout.is_zero()
    {
        return Err(WorkerAdapterError::Configuration(
            "worker limits and lifecycle timeouts must be non-zero".into(),
        ));
    }
    Ok(())
}

fn cleanup_partial(
    child: &mut GroupChild,
    _timeout: Duration,
    message: &str,
) -> WorkerAdapterError {
    terminate_process_tree(child);
    let _ = child.wait();
    WorkerAdapterError::Configuration(message.into())
}

fn telemetry_response_mismatch(
    request: Option<&TelemetryRequestV1>,
    engine_timeline: Option<&EngineStageTimelineV1>,
    worker_token_accounting: Option<&WorkerTokenAccountingV1>,
    require_requested_accounting: bool,
) -> Option<String> {
    let timeline_requested = request.is_some_and(|value| value.engine_stage_timeline);
    if timeline_requested != engine_timeline.is_some() {
        return Some(format!(
            "engine timeline presence mismatch: requested={timeline_requested} present={}",
            engine_timeline.is_some()
        ));
    }
    let accounting_requested = request.is_some_and(|value| value.worker_token_accounting);
    if worker_token_accounting.is_some() && !accounting_requested {
        return Some("unsolicited worker token accounting".into());
    }
    if require_requested_accounting && accounting_requested && worker_token_accounting.is_none() {
        return Some("requested worker token accounting is missing".into());
    }
    None
}

fn duration_ns(value: Duration) -> u64 {
    value.as_nanos().min(u128::from(u64::MAX)) as u64
}

fn finalize_settlement(
    seed: SettlementSeed,
    call_started: Instant,
) -> Result<WorkerSettlementReceiptV1, String> {
    let raw_worker_result_settlement_ns = duration_ns(seed.arrived_at.elapsed());
    let engine_internal_ns = seed
        .engine_timeline
        .as_ref()
        .map_or(0, |timeline| timeline.total_ns);
    let total_ns = duration_ns(call_started.elapsed());
    let known_ns = u128::from(engine_internal_ns) + u128::from(raw_worker_result_settlement_ns);
    let residual_transport_ns = u128::from(total_ns)
        .saturating_sub(known_ns)
        .min(u128::from(u64::MAX)) as u64;
    let partitioned_ns = known_ns + u128::from(residual_transport_ns);
    let closure_error_ns = u128::from(total_ns)
        .abs_diff(partitioned_ns)
        .min(u128::from(u64::MAX)) as u64;
    if closure_error_ns > TIMELINE_CLOSURE_TOLERANCE_NS_V1 {
        return Err(format!(
            "worker settlement does not close: engine_internal_ns={engine_internal_ns} raw_worker_result_settlement_ns={raw_worker_result_settlement_ns} residual_transport_ns={residual_transport_ns} total_ns={total_ns} closure_error_ns={closure_error_ns} tolerance_ns={TIMELINE_CLOSURE_TOLERANCE_NS_V1}"
        ));
    }
    Ok(WorkerSettlementReceiptV1 {
        raw_worker_result_settlement_ns,
        residual_transport_ns,
        total_ns,
        closure_error_ns,
        engine_timeline: seed.engine_timeline,
        worker_token_accounting: seed.worker_token_accounting,
    })
}

fn checked_deadline(
    timeout: Duration,
    request_id: Option<String>,
) -> Result<Instant, WorkerAdapterError> {
    if timeout.as_millis() > i64::MAX as u128 {
        return Err(WorkerAdapterError::DeadlineOverflow { request_id });
    }
    Instant::now()
        .checked_add(timeout)
        .ok_or(WorkerAdapterError::DeadlineOverflow { request_id })
}

fn terminate_process_tree(child: &mut GroupChild) {
    let _ = child.kill();
}

fn ref_scheme(engine: EngineIdentity) -> &'static str {
    match engine {
        EngineIdentity::FsZero => "fz://",
        EngineIdentity::GraphZero => "gz://",
        EngineIdentity::TokenZero => "tz://",
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
