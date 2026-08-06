//! Persistent aggregate CodeMode session over raw-worker v2 clients.
#![forbid(unsafe_code)]

use crate::worker::{
    CancellationSignal, StaticWorkerFactory, WorkerAdapterError, WorkerClient, WorkerClientConfig,
    WorkerContext, WorkerRegistry,
};
use crate::{
    CapabilityDescriptor, Connector, ConnectorError, DispatchContext, GlobalRegistration, Host,
    HostError, HostLimits,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zero_abi::raw_worker::EngineIdentity;
use zero_abi::{ApprovalState, CallRequest, WorkerResult, WorkerTrace};
use zero_store::{Engine, ResolvedStore, ensure_layout};

pub const SESSION_PROTOCOL: &str = "zerostack-session/v1";
pub const MAX_SESSION_FRAME: usize = 1_048_576;
pub const SESSION_SOCKET_ENV: &str = "ZEROSTACK_SESSION_SOCKET";
pub const SESSION_TOKEN_ENV: &str = "ZEROSTACK_SESSION_TOKEN";
pub const SESSION_SHUTDOWN_TOKEN_ENV: &str = "ZEROSTACK_SESSION_SHUTDOWN_TOKEN";
const RAW_WORKER_PROTOCOL_ENV: &str = "ZEROSTACK_RAW_WORKER_PROTOCOL";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionReplacementReason {
    SessionStart,
    BeforeSwitch,
    BeforeFork,
    WorkerRevisionChange,
    Manual,
}

impl SessionReplacementReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "session_start",
            Self::BeforeSwitch => "before_switch",
            Self::BeforeFork => "before_fork",
            Self::WorkerRevisionChange => "worker_revision_change",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionRequest {
    Hello {
        protocol: String,
        token: String,
    },
    Execute {
        id: u64,
        generation: u64,
        root: String,
        source: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    Replace {
        id: u64,
        generation: u64,
        token: String,
        reason: SessionReplacementReason,
    },
    Shutdown {
        id: u64,
        token: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionResponse {
    pub protocol: String,
    pub id: Option<u64>,
    pub ok: bool,
    pub generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}
impl SessionResponse {
    pub fn ok(id: Option<u64>, generation: u64, result: Value) -> Self {
        Self {
            protocol: SESSION_PROTOCOL.into(),
            id,
            ok: true,
            generation,
            result: Some(result),
            error: None,
            code: None,
            retry_after_ms: None,
        }
    }
    pub fn error(id: Option<u64>, generation: u64, error: impl Into<String>) -> Self {
        Self::typed_error(id, generation, "internal", error)
    }

    pub fn typed_error(
        id: Option<u64>,
        generation: u64,
        error_code: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self::typed_error_with_retry(id, generation, error_code, error, None)
    }

    pub fn typed_error_with_retry(
        id: Option<u64>,
        generation: u64,
        error_code: impl Into<String>,
        error: impl Into<String>,
        retry_after_ms: Option<u64>,
    ) -> Self {
        let error = error.into();
        Self {
            protocol: SESSION_PROTOCOL.into(),
            id,
            ok: false,
            generation,
            result: None,
            error: Some(crate::finalize_visible_error(&error)),
            code: Some(error_code.into()),
            retry_after_ms,
        }
    }
}

const METHODS: &[(&str, &str)] = &[
    ("fs", "plan"),
    ("fs", "structural"),
    ("fs", "compound"),
    ("fs", "read_many"),
    ("fs", "list_many"),
    ("fs", "search_many"),
    ("fs", "ast_search_many"),
    ("graph", "blast"),
    ("graph", "query"),
    ("graph", "orient"),
    ("graph", "recall"),
    ("graph", "verify"),
    ("graph", "snap"),
    ("graph", "reserve"),
    ("graph", "index"),
    ("graph", "remember"),
    ("token", "compact"),
    ("token", "expand"),
    ("token", "find"),
    ("token", "shell"),
];

// Fixed session-owned dispatchers keep admission bounded and block on the
// channel while idle. Bursts may launch at most this many raw workers total.
const AGGREGATE_DISPATCH_THREADS: usize = 8;
// Retain one warm raw worker per engine; surplus burst workers shut down before
// their completion is published, so idle process count never grows with use.
const MAX_IDLE_WORKERS_PER_ENGINE: usize = 1;

struct AggregateWorkerState {
    registry: WorkerRegistry,
    workers: Mutex<BTreeMap<EngineIdentity, Vec<WorkerClient>>>,
    worker_config: WorkerClientConfig,
    root: PathBuf,
    session_id: String,
    pins: BTreeMap<EngineIdentity, (String, String)>,
    cancellation: CancellationSignal,
}

struct AggregateDispatch {
    engine: EngineIdentity,
    request: CallRequest,
    revision: String,
    contract_digest: String,
    context: DispatchContext,
    completion: crate::ConnectorCompletion,
}

struct AggregateConnector {
    state: Arc<AggregateWorkerState>,
    dispatch_sender: Option<SyncSender<AggregateDispatch>>,
    dispatchers: Vec<JoinHandle<()>>,
    sequence: AtomicU64,
}

impl AggregateConnector {
    fn new(
        root: PathBuf,
        session_id: String,
        cancellation: CancellationSignal,
    ) -> Result<Self, HostError> {
        let mut registry = WorkerRegistry::new();
        let mut configured_pins = BTreeMap::new();
        for (engine, binary_env, fallback_env) in [
            (
                EngineIdentity::FsZero,
                "ZERO_FSZERO_RAW_BIN",
                Some("ZERO_FSZERO_BIN"),
            ),
            (EngineIdentity::GraphZero, "ZERO_GRAPHZERO_RAW_BIN", None),
            (
                EngineIdentity::TokenZero,
                "ZERO_TOKENZERO_RAW_BIN",
                Some("ZERO_TOKENZERO_BIN"),
            ),
        ] {
            let binary = std::env::var(binary_env)
                .or_else(|_| {
                    fallback_env
                        .map(std::env::var)
                        .unwrap_or(Err(std::env::VarError::NotPresent))
                })
                .map_err(|_| {
                    HostError::Connector(format!("missing raw worker binary: {binary_env}"))
                })?;
            let test_mode = cfg!(feature = "worker-fixture")
                && std::env::var("ZEROSTACK_TEST_MODE").as_deref() == Ok("1");
            let key = engine.as_str().to_ascii_uppercase();
            let (binary, binary_hash) = pinned_worker_binary(Path::new(&binary), &key)?;
            let probed_contract = probe_contract(engine, &binary, test_mode)?;
            let revision = std::env::var(format!("ZEROSTACK_{}_WORKER_REVISION", key))
                .unwrap_or_else(|_| {
                    if test_mode {
                        "fixture-revision".into()
                    } else {
                        binary_hash.clone()
                    }
                });
            let contract = std::env::var(format!("ZEROSTACK_{}_CONTRACT_DIGEST", key))
                .unwrap_or_else(|_| probed_contract.clone());
            if contract != probed_contract {
                return Err(HostError::Connector(format!(
                    "{key} capability probe contract mismatch"
                )));
            }
            let registry_digest = std::env::var(format!("ZEROSTACK_{}_REGISTRY_DIGEST", key))
                .unwrap_or_else(|_| contract.clone());
            let mut factory = StaticWorkerFactory::new(
                binary,
                revision.clone(),
                contract.clone(),
                registry_digest,
            )
            .env("ZEROSTACK_WORKER_REVISION", revision.clone());
            configured_pins.insert(engine, (revision, contract));
            if test_mode {
                let key = format!(
                    "ZEROSTACK_{}_RAW_ARGS",
                    engine.as_str().to_ascii_uppercase()
                );
                if let Ok(args) = std::env::var(key) {
                    for arg in args.split('\u{1f}') {
                        factory = factory.arg(arg);
                    }
                }
            } else {
                match engine {
                    EngineIdentity::FsZero => {
                        factory = factory
                            .arg("--raw-worker")
                            .arg("--root")
                            .arg(root.to_string_lossy().as_ref());
                    }
                    EngineIdentity::TokenZero => {
                        factory = factory
                            .arg("raw-worker")
                            .arg("--root")
                            .arg(root.to_string_lossy().as_ref());
                    }
                    EngineIdentity::GraphZero => {}
                }
            }
            registry
                .register(engine, Arc::new(factory))
                .map_err(worker_error)?;
        }
        let worker_config = WorkerClientConfig::default();
        let mut workers = BTreeMap::new();
        for engine in [
            EngineIdentity::FsZero,
            EngineIdentity::GraphZero,
            EngineIdentity::TokenZero,
        ] {
            let client = registry
                .launch(
                    WorkerContext {
                        engine,
                        store_root: root.clone(),
                        session_id: session_id.clone(),
                    },
                    worker_config.clone(),
                )
                .map_err(worker_error)?;
            workers.insert(engine, vec![client]);
        }
        let state = Arc::new(AggregateWorkerState {
            registry,
            workers: Mutex::new(workers),
            worker_config,
            root,
            session_id,
            pins: configured_pins,
            cancellation,
        });
        let (dispatch_sender, dispatch_receiver) =
            mpsc::sync_channel(crate::MAX_INFLIGHT_CONNECTOR_CALLS);
        let dispatch_receiver = Arc::new(Mutex::new(dispatch_receiver));
        let mut dispatchers: Vec<JoinHandle<()>> = Vec::with_capacity(AGGREGATE_DISPATCH_THREADS);
        for index in 0..AGGREGATE_DISPATCH_THREADS {
            let state = Arc::clone(&state);
            let receiver = Arc::clone(&dispatch_receiver);
            let handle = match thread::Builder::new()
                .name(format!("zerostack-dispatch-{index}"))
                .spawn(move || aggregate_dispatch_loop(state, receiver))
            {
                Ok(handle) => handle,
                Err(error) => {
                    drop(dispatch_sender);
                    for dispatcher in dispatchers {
                        let _ = dispatcher.join();
                    }
                    return Err(HostError::Connector(format!(
                        "cannot start aggregate dispatcher: {error}"
                    )));
                }
            };
            dispatchers.push(handle);
        }
        Ok(Self {
            state,
            dispatch_sender: Some(dispatch_sender),
            dispatchers,
            sequence: AtomicU64::new(1),
        })
    }
}

impl Drop for AggregateConnector {
    fn drop(&mut self) {
        self.state.cancellation.cancel();
        self.dispatch_sender.take();
        for dispatcher in self.dispatchers.drain(..) {
            let _ = dispatcher.join();
        }
    }
}

fn pinned_worker_binary(path: &Path, key: &str) -> Result<(PathBuf, String), HostError> {
    let canonical = path.canonicalize().map_err(|error| {
        HostError::Connector(format!("cannot resolve {key} raw worker: {error}"))
    })?;
    let metadata = canonical.metadata().map_err(|error| {
        HostError::Connector(format!("cannot inspect {key} raw worker: {error}"))
    })?;
    if !metadata.is_file() {
        return Err(HostError::Connector(format!(
            "{key} raw worker is not a regular file"
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(HostError::Connector(format!(
                "{key} raw worker is not executable"
            )));
        }
    }
    let mut file = std::fs::File::open(&canonical)
        .map_err(|error| HostError::Connector(format!("cannot open {key} raw worker: {error}")))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            HostError::Connector(format!("cannot hash {key} raw worker: {error}"))
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let digest_bytes: [u8; 32] = digest.finalize().into();
    let actual = digest_bytes
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            use std::fmt::Write;
            let _ = write!(out, "{b:02x}");
            out
        });
    let variable = format!("ZEROSTACK_{key}_WORKER_SHA256");
    let expected = std::env::var(&variable).unwrap_or_else(|_| actual.clone());
    if expected.len() != 64
        || !expected.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !actual.eq_ignore_ascii_case(&expected)
    {
        return Err(HostError::Connector(format!(
            "{key} raw worker SHA-256 mismatch"
        )));
    }
    Ok((canonical, actual))
}

fn probe_output(
    program: &Path,
    args: &[&str],
    key: &str,
    remove_env: Option<&str>,
) -> Result<String, HostError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env_remove(SESSION_TOKEN_ENV)
        .env_remove(SESSION_SHUTDOWN_TOKEN_ENV);
    if let Some(name) = remove_env {
        command.env_remove(name);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| HostError::Connector(format!("cannot probe {key}: {error}")))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child.wait_with_output().map_err(|error| {
                    HostError::Connector(format!("cannot read {key} probe: {error}"))
                })?;
                let bytes = if output.stdout.is_empty() {
                    output.stderr
                } else {
                    output.stdout
                };
                if !status.success() && bytes.is_empty() {
                    return Err(HostError::Connector(format!(
                        "{key} capability probe failed without output"
                    )));
                }
                return String::from_utf8(bytes).map_err(|error| {
                    HostError::Connector(format!("{key} probe was not UTF-8: {error}"))
                });
            }
            Ok(None) if started.elapsed() < Duration::from_secs(2) => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HostError::Connector(format!(
                    "{key} capability probe timed out"
                )));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HostError::Connector(format!(
                    "cannot wait for {key} probe: {error}"
                )));
            }
        }
    }
}

fn digest_from_json(output: &str) -> Option<String> {
    serde_json::from_str::<Value>(output)
        .ok()?
        .get("semantic_contract_digest")?
        .as_str()
        .map(str::to_owned)
}

fn fszero_contract_from_json(output: &str) -> Option<String> {
    serde_json::from_str::<Value>(output)
        .ok()?
        .get("package")?
        .get("abi_digest")?
        .as_str()
        .map(str::to_owned)
}

fn probe_contract(
    engine: EngineIdentity,
    binary: &Path,
    test_mode: bool,
) -> Result<String, HostError> {
    if test_mode {
        return Ok("0".repeat(64));
    }
    let key = engine.as_str().to_ascii_uppercase();
    let digest = match engine {
        EngineIdentity::FsZero => fszero_contract_from_json(&probe_output(
            binary,
            &["capabilities", "--json"],
            &key,
            None,
        )?),
        EngineIdentity::TokenZero => {
            // TokenZero uses this selector to enter the long-lived v2 serve loop.
            // The one-shot capability probe must not inherit it; the later worker
            // launch still inherits the selector through StaticWorkerFactory.
            digest_from_json(&probe_output(
                binary,
                &["raw-worker", "--handshake"],
                &key,
                Some(RAW_WORKER_PROTOCOL_ENV),
            )?)
        }
        EngineIdentity::GraphZero => {
            let sibling = binary.with_file_name("graphzero-codemode");
            let output = probe_output(&sibling, &["--help"], &key, None)?;
            output.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("semantic_contract_digest:")
                    .map(str::trim)
                    .map(str::to_owned)
            })
        }
    }
    .ok_or_else(|| HostError::Connector(format!("{key} probe omitted contract digest")))?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(HostError::Connector(format!(
            "{key} probe returned invalid contract digest"
        )));
    }
    Ok(digest)
}

fn worker_error(error: WorkerAdapterError) -> HostError {
    HostError::Connector(error.to_string())
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
fn engine_for(surface: &str) -> Result<EngineIdentity, ConnectorError> {
    match surface {
        "fs" => Ok(EngineIdentity::FsZero),
        "graph" => Ok(EngineIdentity::GraphZero),
        "token" => Ok(EngineIdentity::TokenZero),
        _ => Err(ConnectorError::new("unknown aggregate surface")),
    }
}
fn positional_args(input: &Value, first_key: &str, second_key: Option<&str>) -> Value {
    let Some(items) = input.as_array() else {
        if input.is_object() {
            return input.clone();
        }
        let mut object = serde_json::Map::new();
        object.insert(first_key.into(), input.clone());
        return Value::Object(object);
    };
    let mut object = serde_json::Map::new();
    if let Some(options) = items.get(1).and_then(Value::as_object) {
        object.extend(options.clone());
    }
    if let Some(first) = items.first() {
        object.insert(first_key.into(), first.clone());
    }
    if let (Some(key), Some(second)) = (second_key, items.get(1))
        && !second.is_object()
    {
        object.insert(key.into(), second.clone());
    }
    Value::Object(object)
}

fn vector_args(input: &Value, key: &str) -> Value {
    let mut object = serde_json::Map::new();
    if let Some(arguments) = input.as_array() {
        if arguments.len() == 2
            && arguments.first().is_some_and(Value::is_array)
            && arguments.get(1).is_some_and(Value::is_object)
        {
            object.extend(arguments[1].as_object().cloned().unwrap_or_default());
            object.insert(key.into(), arguments[0].clone());
        } else {
            object.insert(key.into(), input.clone());
        }
    } else {
        object.insert(key.into(), input.clone());
    }
    Value::Object(object)
}

fn lower(
    surface: &str,
    method: &str,
    input: Value,
) -> Result<(EngineIdentity, String, Value), ConnectorError> {
    let engine = engine_for(surface)?;
    if surface == "token" && method == "expand" {
        let reference = input
            .as_array()
            .and_then(|values| values.first())
            .and_then(Value::as_str)
            .or_else(|| input.get("ref").and_then(Value::as_str))
            .or_else(|| input.as_str())
            .ok_or_else(|| ConnectorError::new("token.expand requires ref"))?;
        let (engine, op, key) = if reference.starts_with("fz://") {
            (EngineIdentity::FsZero, "fs.expand", "ref")
        } else if reference.starts_with("gz://") {
            (EngineIdentity::GraphZero, "expand", "reference")
        } else if reference.starts_with("tz://") {
            (EngineIdentity::TokenZero, "expand", "ref")
        } else {
            return Err(ConnectorError::new("unsupported ref scheme"));
        };
        let mut args = serde_json::Map::new();
        args.insert(key.into(), Value::String(reference.into()));
        return Ok((engine, op.into(), Value::Object(args)));
    }
    if surface == "fs" && method == "plan" {
        let goal = input
            .as_array()
            .and_then(|values| values.first())
            .or_else(|| input.get("goal"))
            .and_then(Value::as_str)
            .or_else(|| input.as_str())
            .unwrap_or_default();
        let stop = [
            "about",
            "context",
            "discover",
            "entrypoint",
            "files",
            "find",
            "from",
            "into",
            "load",
            "locate",
            "map",
            "repo",
            "repository",
            "the",
            "this",
            "with",
        ];
        let queries: Vec<Value> = goal
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .map(str::to_ascii_lowercase)
            .filter(|term| term.len() >= 3 && !stop.contains(&term.as_str()))
            .take(8)
            .map(Value::String)
            .collect();
        if queries.is_empty() {
            return Ok((engine, "fs.ls".into(), serde_json::json!({"arg":"."})));
        }
        return Ok((
            engine,
            "fs.searchMany".into(),
            serde_json::json!({"queries":queries}),
        ));
    }
    if surface == "fs" && method == "structural" {
        let values = input.as_array();
        let query = values
            .and_then(|items| items.first())
            .or_else(|| input.get("query"))
            .and_then(Value::as_str)
            .or_else(|| input.as_str())
            .ok_or_else(|| ConnectorError::new("fs.structural requires query"))?;
        let target = values
            .and_then(|items| items.get(1))
            .or_else(|| input.get("target"))
            .and_then(Value::as_str);
        let query = target
            .map(|target| format!("{query}:{target}"))
            .unwrap_or_else(|| query.to_owned());
        return Ok((
            engine,
            "fs.search".into(),
            serde_json::json!({"query":query}),
        ));
    }
    if surface == "fs" && method == "compound" {
        let (name, compound_args) = if let Some(items) = input.as_array() {
            (
                items
                    .first()
                    .and_then(Value::as_str)
                    .ok_or_else(|| ConnectorError::new("fs.compound requires name"))?,
                items
                    .get(1)
                    .cloned()
                    .unwrap_or(Value::Object(Default::default())),
            )
        } else {
            (
                input
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ConnectorError::new("fs.compound requires name"))?,
                input
                    .get("args")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default())),
            )
        };
        let op = match name {
            "read" => "fs.read",
            "search" | "find" | "grep" => "fs.search",
            "list" | "tree" | "inventory" => "fs.ls",
            "mutate" | "edit" | "verifiedEdit" => "fs.edit",
            "write" => "fs.write",
            "resolve" => "fs.resolve",
            _ => {
                return Err(ConnectorError::new(
                    "unsupported planner-free fs.compound operation",
                ));
            }
        };
        return Ok((engine, op.into(), compound_args));
    }
    if surface == "fs" {
        let (op, key) = match method {
            "read_many" => ("fs.readMany", "paths"),
            "list_many" => ("fs.listMany", "items"),
            "search_many" => ("fs.searchMany", "queries"),
            "ast_search_many" => ("fs.astSearchMany", "items"),
            _ => return Err(ConnectorError::new("unsupported fs method")),
        };
        return Ok((engine, op.into(), vector_args(&input, key)));
    }
    if surface == "graph" {
        let args = match method {
            "blast" => positional_args(&input, "intent", None),
            "query" | "orient" => positional_args(&input, "surface", Some("query")),
            "recall" => positional_args(&input, "query", None),
            "verify" => positional_args(&input, "target", Some("claim")),
            "snap" => positional_args(&input, "query", Some("budget")),
            "reserve" => positional_args(&input, "action", None),
            "remember" => {
                let fact = input
                    .as_array()
                    .and_then(|values| values.first())
                    .cloned()
                    .unwrap_or(input.clone());
                if fact.is_object() {
                    fact
                } else {
                    serde_json::json!({"text":fact})
                }
            }
            "index" => Value::Object(Default::default()),
            _ => return Err(ConnectorError::new("unsupported graph method")),
        };
        return Ok((engine, method.into(), args));
    }
    let (op, args) = match method {
        "compact" => {
            let value = input
                .as_array()
                .and_then(|values| values.first())
                .cloned()
                .unwrap_or(input);
            let text = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            ("ingest", serde_json::json!({"text":text}))
        }
        "find" => ("find", positional_args(&input, "query", Some("path"))),
        "shell" => ("shell", positional_args(&input, "command", None)),
        _ => return Err(ConnectorError::new("unsupported token method")),
    };
    Ok((engine, op.into(), args))
}

impl Connector for AggregateConnector {
    fn dispatch(
        &self,
        capability: &CapabilityDescriptor,
        args_json: &str,
        context: DispatchContext,
        completion: crate::ConnectorCompletion,
    ) -> Result<(), ConnectorError> {
        let input: Value = serde_json::from_str(args_json)
            .map_err(|error| ConnectorError::new(error.to_string()))?;
        let (engine, op, args) = lower(&capability.surface, &capability.method, input)?;
        if context.is_expired() || self.state.cancellation.is_cancelled() {
            return Err(ConnectorError::new(
                "aggregate dispatch deadline or cancellation",
            ));
        }
        let id = format!(
            "{}-{}",
            self.state.session_id,
            self.sequence.fetch_add(1, Ordering::Relaxed)
        );
        let (revision, contract_digest) = self
            .state
            .pins
            .get(&engine)
            .cloned()
            .ok_or_else(|| ConnectorError::new("worker pin missing"))?;
        let trace = WorkerTrace {
            runtime_id: self.state.session_id.clone(),
            cell_id: self.state.session_id.clone(),
            request_id: id.clone(),
            trace_id: id.clone(),
            parent_span_id: None,
            worker_revision: revision.clone(),
            contract_digest: contract_digest.clone(),
        };
        let request = CallRequest {
            request_id: id,
            op,
            args,
            deadline_unix_ms: Some(
                now_ms().saturating_add(
                    context
                        .remaining()
                        .as_millis()
                        .try_into()
                        .unwrap_or(u64::MAX),
                ),
            ),
            trace,
            approval_grant: None,
            telemetry_request: None,
        };
        let dispatch = AggregateDispatch {
            engine,
            request,
            revision,
            contract_digest,
            context,
            completion,
        };
        let sender = self
            .dispatch_sender
            .as_ref()
            .ok_or_else(|| ConnectorError::new("aggregate dispatcher closed"))?;
        match sender.try_send(dispatch) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                Err(ConnectorError::new("aggregate dispatch capacity exhausted"))
            }
            Err(TrySendError::Disconnected(_)) => {
                Err(ConnectorError::new("aggregate dispatcher closed"))
            }
        }
    }
}

fn aggregate_dispatch_loop(
    state: Arc<AggregateWorkerState>,
    receiver: Arc<Mutex<Receiver<AggregateDispatch>>>,
) {
    loop {
        let dispatch = match receiver.lock() {
            Ok(receiver) => receiver.recv(),
            Err(poisoned) => poisoned.into_inner().recv(),
        };
        let Ok(dispatch) = dispatch else {
            break;
        };
        let result = run_aggregate_dispatch(&state, &dispatch);
        let _ = dispatch.completion.complete(result);
    }
}

fn run_aggregate_dispatch(
    state: &AggregateWorkerState,
    dispatch: &AggregateDispatch,
) -> Result<String, ConnectorError> {
    if dispatch.context.is_expired() || state.cancellation.is_cancelled() {
        return Err(ConnectorError::new(
            "aggregate dispatch deadline or cancellation",
        ));
    }
    let mut worker = checkout_worker(state, dispatch.engine)?;
    if worker.is_terminal() {
        worker = launch_worker(state, dispatch.engine)?;
    }
    let result = worker
        .dispatch_with_cancel(dispatch.request.clone(), &state.cancellation)
        .map_err(|error| ConnectorError::new(error.to_string()));
    let reusable = !worker.is_terminal();
    let checkin = if reusable {
        checkin_worker(state, dispatch.engine, worker)
    } else {
        Ok(())
    };
    let result: WorkerResult = result?;
    checkin?;
    if matches!(
        result.metadata.approval.state,
        ApprovalState::Required | ApprovalState::Denied
    ) {
        return Err(ConnectorError::new("worker approval required or denied"));
    }
    if result.metadata.ownership.engine != dispatch.engine
        || result.metadata.ownership.session_id != state.session_id
        || result.metadata.trace.runtime_id != state.session_id
        || result.metadata.trace.request_id != result.metadata.trace.trace_id
        || result.metadata.trace.worker_revision != dispatch.revision
        || result.metadata.trace.contract_digest != dispatch.contract_digest
    {
        return Err(ConnectorError::new("worker result binding mismatch"));
    }
    serde_json::to_string(&serde_json::json!({"value": result.value, "metadata": result.metadata}))
        .map_err(|error| ConnectorError::new(error.to_string()))
}

fn checkout_worker(
    state: &AggregateWorkerState,
    engine: EngineIdentity,
) -> Result<WorkerClient, ConnectorError> {
    let worker = state
        .workers
        .lock()
        .map_err(|_| ConnectorError::new("worker pool lock poisoned"))?
        .entry(engine)
        .or_default()
        .pop();
    match worker {
        Some(worker) => Ok(worker),
        None => launch_worker(state, engine),
    }
}

fn launch_worker(
    state: &AggregateWorkerState,
    engine: EngineIdentity,
) -> Result<WorkerClient, ConnectorError> {
    state
        .registry
        .launch(
            WorkerContext {
                engine,
                store_root: state.root.clone(),
                session_id: state.session_id.clone(),
            },
            state.worker_config.clone(),
        )
        .map_err(|error| ConnectorError::new(error.to_string()))
}

fn checkin_worker(
    state: &AggregateWorkerState,
    engine: EngineIdentity,
    mut worker: WorkerClient,
) -> Result<(), ConnectorError> {
    let mut workers = state
        .workers
        .lock()
        .map_err(|_| ConnectorError::new("worker pool lock poisoned"))?;
    let idle = workers.entry(engine).or_default();
    if idle.len() < MAX_IDLE_WORKERS_PER_ENGINE {
        idle.push(worker);
        return Ok(());
    }
    drop(workers);
    worker
        .shutdown()
        .map_err(|error| ConnectorError::new(format!("surplus worker shutdown failed: {error}")))
}

#[derive(Clone)]
pub struct SessionCancellation {
    host: Arc<AtomicBool>,
    worker: CancellationSignal,
}
impl SessionCancellation {
    pub fn cancel(&self) {
        self.host.store(true, Ordering::Release);
        self.worker.cancel();
    }
}

pub struct SessionExecutor {
    host: Host,
    connector: Rc<AggregateConnector>,
    cancelled: Arc<AtomicBool>,
}
impl SessionExecutor {
    pub fn new() -> Result<Self, HostError> {
        let root = std::env::var("ZEROSTACK_SESSION_ROOT").map_err(|_| {
            HostError::Connector("missing explicit ZEROSTACK_SESSION_ROOT authorization".into())
        })?;
        let session_id = std::env::var(crate::worker::SESSION_ID_ENV)
            .ok()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                HostError::Connector("missing explicit ZeroStack session identity".into())
            })?;
        Self::new_authorized(PathBuf::from(root), session_id)
    }

    fn new_authorized(root: PathBuf, session_id: String) -> Result<Self, HostError> {
        if session_id.is_empty() {
            return Err(HostError::Connector(
                "missing explicit ZeroStack session identity".into(),
            ));
        }
        let root = root.canonicalize().map_err(|error| {
            HostError::Connector(format!("cannot resolve authorized session root: {error}"))
        })?;
        let resolved_store = ResolvedStore::resolve_from_process(&root, Engine::TokenZero, &[]);
        ensure_layout(&resolved_store).map_err(|error| {
            HostError::Connector(format!("cannot prepare session result store: {error}"))
        })?;
        let result_spill_root = resolved_store.cas_host().to_path_buf();
        let cancellation = CancellationSignal::new();
        let connector = Rc::new(AggregateConnector::new(
            root,
            session_id,
            cancellation.clone(),
        )?);
        let registration = GlobalRegistration::zero(
            METHODS
                .iter()
                .map(|(s, m)| CapabilityDescriptor::new(*s, *m))
                .collect(),
        );
        let limits = HostLimits::new(
            128 * 1024 * 1024,
            1024 * 1024,
            Duration::from_secs(30),
            10_000_000,
            16_384,
            256 * 1024,
            16 * 1024 * 1024,
        )
        .map_err(HostError::Limits)?;
        let host = Host::new(limits, registration)?
            .with_visible_result_budget(crate::DEFAULT_MAX_VISIBLE_RESULT_BYTES)?
            .with_result_spill(result_spill_root);
        Ok(Self {
            host,
            connector,
            cancelled: Arc::new(AtomicBool::new(false)),
        })
    }
    pub fn execute(&self, source: &str, timeout: Duration) -> Result<Value, HostError> {
        if self.cancelled.load(Ordering::Acquire)
            || self.connector.state.cancellation.is_cancelled()
        {
            return Err(HostError::Connector("session cancelled".into()));
        }
        self.host.execute_with_cancel_timeout(
            source,
            self.connector.clone(),
            self.cancelled.clone(),
            timeout,
        )
    }
    pub fn cancellation(&self) -> SessionCancellation {
        SessionCancellation {
            host: self.cancelled.clone(),
            worker: self.connector.state.cancellation.clone(),
        }
    }
    pub fn cancel(&self) {
        self.cancellation().cancel();
    }
}

pub const SESSION_EXECUTION_QUEUE_CAPACITY: usize = 8;
pub const SESSION_REPLACEMENT_SETTLE_TIMEOUT: Duration = Duration::from_secs(1);
pub const SESSION_EXECUTOR_START_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateSessionFailureCode {
    InvalidGeneration,
    StaleGeneration,
    DuplicateRequestId,
    Backpressure,
    ReplacementInProgress,
    Terminating,
    GenerationExhausted,
    BackendUnavailable,
    MethodNotFound,
    SurfaceNotFound,
    BackendExecution,
    Internal,
}

impl AggregateSessionFailureCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidGeneration => "invalid_generation",
            Self::StaleGeneration => "stale_generation",
            Self::DuplicateRequestId => "duplicate_request_id",
            Self::Backpressure => "backpressure",
            Self::ReplacementInProgress => "replacement_in_progress",
            Self::Terminating => "session_terminating",
            Self::GenerationExhausted => "generation_exhausted",
            Self::BackendUnavailable => "backend_unavailable",
            Self::MethodNotFound => "method_not_found",
            Self::SurfaceNotFound => "surface_not_found",
            Self::BackendExecution => "backend_execution",
            Self::Internal => "internal",
        }
    }
}

fn backend_failure_code(error: &HostError) -> AggregateSessionFailureCode {
    match error {
        HostError::MethodNotFound(_) => AggregateSessionFailureCode::MethodNotFound,
        HostError::SurfaceNotFound(_) => AggregateSessionFailureCode::SurfaceNotFound,
        _ => AggregateSessionFailureCode::BackendExecution,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AggregateSessionError {
    pub code: AggregateSessionFailureCode,
    pub generation: u64,
    pub request_id: Option<u64>,
    pub detail: String,
    pub retry_after_ms: Option<u64>,
}

impl AggregateSessionError {
    fn new(
        code: AggregateSessionFailureCode,
        generation: u64,
        request_id: Option<u64>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            code,
            generation,
            request_id,
            detail: detail.into(),
            retry_after_ms: None,
        }
    }

    fn backpressure(generation: u64, request_id: u64) -> Self {
        Self {
            code: AggregateSessionFailureCode::Backpressure,
            generation,
            request_id: Some(request_id),
            detail: format!(
                "session execution queue is full (capacity {})",
                SESSION_EXECUTION_QUEUE_CAPACITY
            ),
            retry_after_ms: Some(1),
        }
    }
}

impl std::fmt::Display for AggregateSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}

impl std::error::Error for AggregateSessionError {}

#[derive(Debug)]
pub struct SessionExecutionResult {
    pub generation: u64,
    pub request_id: u64,
    pub value: Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionReplacementReceipt {
    pub previous_generation: u64,
    pub generation: u64,
    pub reason: SessionReplacementReason,
}

#[derive(Debug)]
struct AggregateSessionState {
    generation: u64,
    accepting: bool,
    replacing: bool,
    terminating: bool,
    shutdown_sent: bool,
    worker_stopped: bool,
    seen_request_ids: BTreeSet<u64>,
    active_request_ids: BTreeSet<u64>,
}

enum SessionCommand {
    Execute {
        source: String,
        timeout: Duration,
        reply: SyncSender<Result<Value, HostError>>,
    },
    Replace {
        generation: u64,
        reply: SyncSender<Result<(), String>>,
    },
    Shutdown {
        reply: SyncSender<()>,
    },
}

pub struct AggregateSession {
    state: Arc<Mutex<AggregateSessionState>>,
    commands: SyncSender<SessionCommand>,
    cancellation: Arc<Mutex<Option<SessionCancellation>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub struct AggregateSessionCancellation {
    state: Arc<Mutex<AggregateSessionState>>,
    cancellation: Arc<Mutex<Option<SessionCancellation>>>,
}

impl AggregateSessionCancellation {
    pub fn cancel(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.accepting = false;
            state.terminating = true;
            if let Some(next) = state.generation.checked_add(1) {
                state.generation = next;
                state.seen_request_ids.clear();
            }
        }
        cancel_backend(&self.cancellation);
    }
}

impl AggregateSession {
    pub fn new(initial_generation: u64) -> Result<Self, AggregateSessionError> {
        if initial_generation == 0 {
            return Err(AggregateSessionError::new(
                AggregateSessionFailureCode::InvalidGeneration,
                initial_generation,
                None,
                "initial generation must be nonzero",
            ));
        }
        let root = std::env::var("ZEROSTACK_SESSION_ROOT").map_err(|_| {
            AggregateSessionError::new(
                AggregateSessionFailureCode::BackendUnavailable,
                initial_generation,
                None,
                "missing explicit ZEROSTACK_SESSION_ROOT authorization",
            )
        })?;
        Self::new_authorized(initial_generation, PathBuf::from(root))
    }

    pub fn new_authorized(
        initial_generation: u64,
        root: PathBuf,
    ) -> Result<Self, AggregateSessionError> {
        if initial_generation == 0 {
            return Err(AggregateSessionError::new(
                AggregateSessionFailureCode::InvalidGeneration,
                initial_generation,
                None,
                "initial generation must be nonzero",
            ));
        }
        let (commands, receiver) = mpsc::sync_channel(SESSION_EXECUTION_QUEUE_CAPACITY);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let cancellation = Arc::new(Mutex::new(None));
        let worker_cancellation = Arc::clone(&cancellation);
        let worker = thread::Builder::new()
            .name("zerostack-session-executor".into())
            .spawn(move || {
                session_worker(
                    initial_generation,
                    root,
                    receiver,
                    worker_cancellation,
                    ready_tx,
                )
            })
            .map_err(|error| {
                AggregateSessionError::new(
                    AggregateSessionFailureCode::BackendUnavailable,
                    initial_generation,
                    None,
                    format!("failed to spawn session executor: {error}"),
                )
            })?;
        match ready_rx.recv_timeout(SESSION_EXECUTOR_START_TIMEOUT) {
            Ok(Ok(())) => {}
            Ok(Err(detail)) => {
                let _ = worker.join();
                return Err(AggregateSessionError::new(
                    AggregateSessionFailureCode::BackendUnavailable,
                    initial_generation,
                    None,
                    detail,
                ));
            }
            Err(error) => {
                return Err(AggregateSessionError::new(
                    AggregateSessionFailureCode::BackendUnavailable,
                    initial_generation,
                    None,
                    format!(
                        "session executor did not start within {}ms: {error}",
                        SESSION_EXECUTOR_START_TIMEOUT.as_millis()
                    ),
                ));
            }
        }
        Ok(Self {
            state: Arc::new(Mutex::new(AggregateSessionState {
                generation: initial_generation,
                accepting: true,
                replacing: false,
                terminating: false,
                shutdown_sent: false,
                worker_stopped: false,
                seen_request_ids: BTreeSet::new(),
                active_request_ids: BTreeSet::new(),
            })),
            commands,
            cancellation,
            worker: Mutex::new(Some(worker)),
        })
    }

    pub fn generation(&self) -> Result<u64, AggregateSessionError> {
        self.state
            .lock()
            .map(|state| state.generation)
            .map_err(|_| {
                AggregateSessionError::new(
                    AggregateSessionFailureCode::Internal,
                    0,
                    None,
                    "session lifecycle state is poisoned",
                )
            })
    }

    pub fn cancellation(&self) -> AggregateSessionCancellation {
        AggregateSessionCancellation {
            state: Arc::clone(&self.state),
            cancellation: Arc::clone(&self.cancellation),
        }
    }

    pub fn execute(
        &self,
        generation: u64,
        request_id: u64,
        source: impl Into<String>,
        timeout: Duration,
    ) -> Result<SessionExecutionResult, AggregateSessionError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        {
            let mut state = self.lock_state(Some(request_id))?;
            if generation != state.generation {
                return Err(AggregateSessionError::new(
                    AggregateSessionFailureCode::StaleGeneration,
                    state.generation,
                    Some(request_id),
                    format!(
                        "request generation {generation} does not match active generation {}",
                        state.generation
                    ),
                ));
            }
            if state.terminating || !state.accepting {
                return Err(AggregateSessionError::new(
                    AggregateSessionFailureCode::Terminating,
                    state.generation,
                    Some(request_id),
                    "session is not accepting execution",
                ));
            }
            if !state.seen_request_ids.insert(request_id) {
                return Err(AggregateSessionError::new(
                    AggregateSessionFailureCode::DuplicateRequestId,
                    state.generation,
                    Some(request_id),
                    "request id was already admitted in this generation",
                ));
            }
            state.active_request_ids.insert(request_id);
        }
        match self.commands.try_send(SessionCommand::Execute {
            source: source.into(),
            timeout,
            reply: reply_tx,
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.release_unadmitted(generation, request_id);
                return Err(AggregateSessionError::backpressure(generation, request_id));
            }
            Err(TrySendError::Disconnected(_)) => {
                self.release_active(request_id);
                return Err(AggregateSessionError::new(
                    AggregateSessionFailureCode::BackendUnavailable,
                    generation,
                    Some(request_id),
                    "session executor is unavailable",
                ));
            }
        }
        let backend_result = reply_rx.recv().map_err(|error| {
            AggregateSessionError::new(
                AggregateSessionFailureCode::BackendUnavailable,
                generation,
                Some(request_id),
                format!("session executor dropped the result: {error}"),
            )
        });
        let (current, terminating) = {
            let mut state = self.lock_state(Some(request_id))?;
            state.active_request_ids.remove(&request_id);
            (state.generation, state.terminating)
        };
        if current != generation || terminating {
            return Err(AggregateSessionError::new(
                AggregateSessionFailureCode::StaleGeneration,
                current,
                Some(request_id),
                "execution settled after its generation was replaced",
            ));
        }
        let value = backend_result?.map_err(|error| {
            AggregateSessionError::new(
                backend_failure_code(&error),
                generation,
                Some(request_id),
                error.to_string(),
            )
        })?;
        Ok(SessionExecutionResult {
            generation,
            request_id,
            value,
        })
    }

    pub fn replace(
        &self,
        expected_generation: u64,
        reason: SessionReplacementReason,
    ) -> Result<SessionReplacementReceipt, AggregateSessionError> {
        let next_generation = {
            let mut state = self.lock_state(None)?;
            if expected_generation != state.generation {
                return Err(AggregateSessionError::new(
                    AggregateSessionFailureCode::StaleGeneration,
                    state.generation,
                    None,
                    format!(
                        "replacement generation {expected_generation} does not match active generation {}",
                        state.generation
                    ),
                ));
            }
            if state.terminating {
                return Err(AggregateSessionError::new(
                    AggregateSessionFailureCode::Terminating,
                    state.generation,
                    None,
                    "session is terminating",
                ));
            }
            if state.replacing {
                return Err(AggregateSessionError::new(
                    AggregateSessionFailureCode::ReplacementInProgress,
                    state.generation,
                    None,
                    "another replacement is already in progress",
                ));
            }
            let next = state.generation.checked_add(1).ok_or_else(|| {
                AggregateSessionError::new(
                    AggregateSessionFailureCode::GenerationExhausted,
                    state.generation,
                    None,
                    "session generation cannot advance",
                )
            })?;
            state.accepting = false;
            state.replacing = true;
            state.generation = next;
            state.seen_request_ids.clear();
            next
        };
        cancel_backend(&self.cancellation);
        let control_started = Instant::now();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        if let Err(detail) = send_control_with_deadline(
            &self.commands,
            SessionCommand::Replace {
                generation: next_generation,
                reply: reply_tx,
            },
            SESSION_REPLACEMENT_SETTLE_TIMEOUT,
        ) {
            self.finish_failed_replacement(next_generation);
            return Err(AggregateSessionError::new(
                AggregateSessionFailureCode::BackendUnavailable,
                next_generation,
                None,
                detail,
            ));
        }
        let settle_remaining =
            SESSION_REPLACEMENT_SETTLE_TIMEOUT.saturating_sub(control_started.elapsed());
        let replacement = reply_rx.recv_timeout(settle_remaining).map_err(|error| {
            self.finish_failed_replacement(next_generation);
            AggregateSessionError::new(
                AggregateSessionFailureCode::BackendUnavailable,
                next_generation,
                None,
                format!(
                    "session replacement did not settle within {}ms: {error}",
                    SESSION_REPLACEMENT_SETTLE_TIMEOUT.as_millis()
                ),
            )
        })?;
        let mut state = self.lock_state(None)?;
        state.replacing = false;
        match replacement {
            Ok(()) if state.generation == next_generation && !state.terminating => {
                state.accepting = true;
                Ok(SessionReplacementReceipt {
                    previous_generation: expected_generation,
                    generation: next_generation,
                    reason,
                })
            }
            Ok(()) => Err(AggregateSessionError::new(
                AggregateSessionFailureCode::StaleGeneration,
                state.generation,
                None,
                "replacement completed after lifecycle state advanced",
            )),
            Err(detail) => Err(AggregateSessionError::new(
                AggregateSessionFailureCode::BackendUnavailable,
                state.generation,
                None,
                detail,
            )),
        }
    }

    pub fn shutdown(&self) -> Result<u64, AggregateSessionError> {
        let (generation, should_send) = {
            let mut state = self.lock_state(None)?;
            if state.worker_stopped || state.shutdown_sent {
                return Ok(state.generation);
            }
            state.accepting = false;
            state.terminating = true;
            state.replacing = false;
            state.shutdown_sent = true;
            (state.generation, true)
        };
        cancel_backend(&self.cancellation);
        if should_send {
            let control_started = Instant::now();
            let (reply_tx, reply_rx) = mpsc::sync_channel(1);
            send_control_with_deadline(
                &self.commands,
                SessionCommand::Shutdown { reply: reply_tx },
                SESSION_REPLACEMENT_SETTLE_TIMEOUT,
            )
            .map_err(|detail| {
                AggregateSessionError::new(
                    AggregateSessionFailureCode::BackendUnavailable,
                    generation,
                    None,
                    detail,
                )
            })?;
            let settle_remaining =
                SESSION_REPLACEMENT_SETTLE_TIMEOUT.saturating_sub(control_started.elapsed());
            reply_rx.recv_timeout(settle_remaining).map_err(|error| {
                AggregateSessionError::new(
                    AggregateSessionFailureCode::BackendUnavailable,
                    generation,
                    None,
                    format!(
                        "session shutdown did not settle within {}ms: {error}",
                        SESSION_REPLACEMENT_SETTLE_TIMEOUT.as_millis()
                    ),
                )
            })?;
        }
        if let Ok(mut worker) = self.worker.lock()
            && let Some(handle) = worker.take()
        {
            handle.join().map_err(|_| {
                AggregateSessionError::new(
                    AggregateSessionFailureCode::BackendUnavailable,
                    generation,
                    None,
                    "session executor panicked during shutdown",
                )
            })?;
        }
        if let Ok(mut state) = self.state.lock() {
            state.worker_stopped = true;
        }
        Ok(generation)
    }

    fn lock_state(
        &self,
        request_id: Option<u64>,
    ) -> Result<std::sync::MutexGuard<'_, AggregateSessionState>, AggregateSessionError> {
        self.state.lock().map_err(|_| {
            AggregateSessionError::new(
                AggregateSessionFailureCode::Internal,
                0,
                request_id,
                "session lifecycle state is poisoned",
            )
        })
    }

    fn release_unadmitted(&self, generation: u64, request_id: u64) {
        if let Ok(mut state) = self.state.lock() {
            state.active_request_ids.remove(&request_id);
            if state.generation == generation {
                state.seen_request_ids.remove(&request_id);
            }
        }
    }

    fn release_active(&self, request_id: u64) {
        if let Ok(mut state) = self.state.lock() {
            state.active_request_ids.remove(&request_id);
        }
    }

    fn finish_failed_replacement(&self, generation: u64) {
        if let Ok(mut state) = self.state.lock()
            && state.generation == generation
        {
            state.replacing = false;
            state.accepting = false;
        }
    }
}

impl Drop for AggregateSession {
    fn drop(&mut self) {
        self.cancellation().cancel();
        if let Ok(state) = self.state.lock()
            && (state.shutdown_sent || state.worker_stopped)
        {
            return;
        }
        let (reply, _) = mpsc::sync_channel(1);
        let _ = self.commands.try_send(SessionCommand::Shutdown { reply });
    }
}

fn send_control_with_deadline(
    commands: &SyncSender<SessionCommand>,
    mut command: SessionCommand,
    timeout: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        match commands.try_send(command) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => {
                return Err("session executor is unavailable".into());
            }
            Err(TrySendError::Full(returned)) => {
                if started.elapsed() >= timeout {
                    return Err(format!(
                        "session control queue did not admit within {}ms",
                        timeout.as_millis()
                    ));
                }
                command = returned;
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

fn cancel_backend(cancellation: &Arc<Mutex<Option<SessionCancellation>>>) {
    if let Ok(slot) = cancellation.lock()
        && let Some(signal) = slot.as_ref()
    {
        signal.cancel();
    }
}

fn session_worker(
    initial_generation: u64,
    root: PathBuf,
    commands: Receiver<SessionCommand>,
    cancellation: Arc<Mutex<Option<SessionCancellation>>>,
    ready: SyncSender<Result<(), String>>,
) {
    let mut executor = match start_session_executor(initial_generation, &root) {
        Ok(executor) => {
            if let Ok(mut slot) = cancellation.lock() {
                *slot = Some(executor.cancellation());
            }
            let _ = ready.send(Ok(()));
            Some(executor)
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    while let Ok(command) = commands.recv() {
        match command {
            SessionCommand::Execute {
                source,
                timeout,
                reply,
            } => {
                let result = executor
                    .as_ref()
                    .ok_or_else(|| HostError::Runtime("session executor is unavailable".into()))
                    .and_then(|executor| executor.execute(&source, timeout));
                let _ = reply.send(result);
            }
            SessionCommand::Replace { generation, reply } => {
                if let Ok(mut slot) = cancellation.lock() {
                    *slot = None;
                }
                drop(executor.take());
                let result = start_session_executor(generation, &root).map(|next| {
                    if let Ok(mut slot) = cancellation.lock() {
                        *slot = Some(next.cancellation());
                    }
                    executor = Some(next);
                });
                let _ = reply.send(result);
            }
            SessionCommand::Shutdown { reply } => {
                if let Ok(mut slot) = cancellation.lock() {
                    *slot = None;
                }
                drop(executor.take());
                let _ = reply.send(());
                break;
            }
        }
    }
    if let Ok(mut slot) = cancellation.lock() {
        *slot = None;
    }
}

/// Starts one generation-bound executor with explicit authorization context.
/// Worker child processes receive the derived session id via `Command::env`.
fn start_session_executor(generation: u64, root: &Path) -> Result<SessionExecutor, String> {
    let session_id = format!("session-{generation:016x}");
    let executor = SessionExecutor::new_authorized(root.to_path_buf(), session_id)
        .map_err(|error| error.to_string())?;
    executor
        .execute("return null", Duration::from_secs(1))
        .map_err(|error| format!("session prewarm failed: {error}"))?;
    Ok(executor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn typed_error_text_is_bounded_before_wire_serialization() {
        let response =
            SessionResponse::typed_error(Some(1), 2, "backend_execution", "é".repeat(2_000));
        let error = response.error.unwrap();
        assert!(error.len() <= crate::MAX_VISIBLE_ERROR_BYTES);
        assert!(error.ends_with("... [truncated]"));
    }

    fn assert_lower(
        surface: &str,
        method: &str,
        input: Value,
        engine: EngineIdentity,
        op: &str,
        args: Value,
    ) {
        let lowered = lower(surface, method, input).unwrap();
        assert_eq!(lowered.0, engine);
        assert_eq!(lowered.1, op);
        assert_eq!(lowered.2, args);
    }

    #[test]
    fn fs_methods_use_canonical_domain_operations() {
        let plan = lower("fs", "plan", json!("map widget entrypoint")).unwrap();
        assert_eq!(plan.0, EngineIdentity::FsZero);
        assert_eq!(plan.1, "fs.searchMany");
        assert_eq!(plan.2["queries"], json!(["widget"]));
        assert_lower(
            "fs",
            "structural",
            json!(["callers", "Widget"]),
            EngineIdentity::FsZero,
            "fs.search",
            json!({"query":"callers:Widget"}),
        );
        assert_lower(
            "fs",
            "compound",
            json!(["read", {"path":"src/lib.rs"}]),
            EngineIdentity::FsZero,
            "fs.read",
            json!({"path":"src/lib.rs"}),
        );
        assert_lower(
            "fs",
            "read_many",
            json!([["a.rs"], {"max_bytes":32}]),
            EngineIdentity::FsZero,
            "fs.readMany",
            json!({"paths":["a.rs"],"max_bytes":32}),
        );
        assert_lower(
            "fs",
            "search_many",
            json!(["one", "two"]),
            EngineIdentity::FsZero,
            "fs.searchMany",
            json!({"queries":["one","two"]}),
        );
    }

    #[test]
    fn graph_and_token_methods_use_bare_domain_operations() {
        assert_lower(
            "graph",
            "blast",
            json!(["Widget", {"depth":2}]),
            EngineIdentity::GraphZero,
            "blast",
            json!({"intent":"Widget","depth":2}),
        );
        assert_lower(
            "graph",
            "query",
            json!(["symbol", "Widget"]),
            EngineIdentity::GraphZero,
            "query",
            json!({"surface":"symbol","query":"Widget"}),
        );
        assert_lower(
            "token",
            "shell",
            json!(["printf ok", {"timeout_seconds":1}]),
            EngineIdentity::TokenZero,
            "shell",
            json!({"command":"printf ok","timeout_seconds":1}),
        );
        assert_lower(
            "token",
            "find",
            json!("Widget"),
            EngineIdentity::TokenZero,
            "find",
            json!({"query":"Widget"}),
        );
    }

    #[test]
    fn expansion_routes_to_the_ref_owner() {
        for (reference, engine, op, key) in [
            ("fz://blob/00", EngineIdentity::FsZero, "fs.expand", "ref"),
            (
                "gz://blob/00",
                EngineIdentity::GraphZero,
                "expand",
                "reference",
            ),
            ("tz://blob/00", EngineIdentity::TokenZero, "expand", "ref"),
        ] {
            let mut expected = serde_json::Map::new();
            expected.insert(key.into(), Value::String(reference.into()));
            assert_lower(
                "token",
                "expand",
                json!(reference),
                engine,
                op,
                Value::Object(expected),
            );
        }
        assert!(lower("token", "expand", json!("https://invalid")).is_err());
    }
}
