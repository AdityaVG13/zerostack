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
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zero_abi::raw_worker::EngineIdentity;
use zero_abi::{ApprovalState, CallRequest, WorkerResult, WorkerTrace};

pub const SESSION_PROTOCOL: &str = "zerostack-session/v1";
pub const MAX_SESSION_FRAME: usize = 1_048_576;
pub const SESSION_SOCKET_ENV: &str = "ZEROSTACK_SESSION_SOCKET";
pub const SESSION_TOKEN_ENV: &str = "ZEROSTACK_SESSION_TOKEN";
pub const SESSION_SHUTDOWN_TOKEN_ENV: &str = "ZEROSTACK_SESSION_SHUTDOWN_TOKEN";

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
        }
    }
    pub fn error(id: Option<u64>, generation: u64, error: impl Into<String>) -> Self {
        Self {
            protocol: SESSION_PROTOCOL.into(),
            id,
            ok: false,
            generation,
            result: None,
            error: Some(error.into()),
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

struct AggregateConnector {
    registry: WorkerRegistry,
    workers: Mutex<BTreeMap<EngineIdentity, WorkerClient>>,
    worker_config: WorkerClientConfig,
    root: PathBuf,
    session_id: String,
    pins: BTreeMap<EngineIdentity, (String, String)>,
    sequence: AtomicU64,
    cancellation: CancellationSignal,
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
        let config = WorkerClientConfig::default();
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
                    config.clone(),
                )
                .map_err(worker_error)?;
            workers.insert(engine, client);
        }
        Ok(Self {
            registry,
            workers: Mutex::new(workers),
            worker_config: config,
            root,
            session_id,
            pins: configured_pins,
            sequence: AtomicU64::new(1),
            cancellation,
        })
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
    let actual = format!("{:x}", digest.finalize());
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

fn probe_output(program: &Path, args: &[&str], key: &str) -> Result<String, HostError> {
    let mut child = Command::new(program)
        .args(args)
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
        EngineIdentity::FsZero => {
            fszero_contract_from_json(&probe_output(binary, &["capabilities", "--json"], &key)?)
        }
        EngineIdentity::TokenZero => {
            digest_from_json(&probe_output(binary, &["raw-worker", "--handshake"], &key)?)
        }
        EngineIdentity::GraphZero => {
            let sibling = binary.with_file_name("graphzero-codemode");
            let output = probe_output(&sibling, &["--help"], &key)?;
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
    if let (Some(key), Some(second)) = (second_key, items.get(1)) {
        if !second.is_object() {
            object.insert(key.into(), second.clone());
        }
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
                ))
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
    fn call(
        &self,
        capability: &CapabilityDescriptor,
        args_json: &str,
        context: DispatchContext,
    ) -> Result<String, ConnectorError> {
        let input: Value =
            serde_json::from_str(args_json).map_err(|e| ConnectorError::new(e.to_string()))?;
        let (engine, op, args) = lower(&capability.surface, &capability.method, input)?;
        if context.is_expired() {
            return Err(ConnectorError::new("aggregate dispatch deadline exceeded"));
        }
        let id = format!(
            "{}-{}",
            self.session_id,
            self.sequence.fetch_add(1, Ordering::Relaxed)
        );
        let (revision, contract_digest) = self
            .pins
            .get(&engine)
            .cloned()
            .ok_or_else(|| ConnectorError::new("worker pin missing"))?;
        let trace = WorkerTrace {
            runtime_id: self.session_id.clone(),
            cell_id: self.session_id.clone(),
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
        };
        let mut workers = self
            .workers
            .lock()
            .map_err(|_| ConnectorError::new("worker registry lock poisoned"))?;
        let worker = workers
            .get_mut(&engine)
            .ok_or_else(|| ConnectorError::new("worker unavailable"))?;
        if worker.is_terminal() {
            *worker = self
                .registry
                .launch(
                    WorkerContext {
                        engine,
                        store_root: self.root.clone(),
                        session_id: self.session_id.clone(),
                    },
                    self.worker_config.clone(),
                )
                .map_err(|error| ConnectorError::new(error.to_string()))?;
        }
        let result: WorkerResult = worker
            .dispatch_with_cancel(request, &self.cancellation)
            .map_err(|e| ConnectorError::new(e.to_string()))?;
        if matches!(
            result.metadata.approval.state,
            ApprovalState::Required | ApprovalState::Denied
        ) {
            return Err(ConnectorError::new("worker approval required or denied"));
        }
        if result.metadata.ownership.engine != engine
            || result.metadata.ownership.session_id != self.session_id
            || result.metadata.trace.runtime_id != self.session_id
            || result.metadata.trace.request_id != result.metadata.trace.trace_id
            || result.metadata.trace.worker_revision != revision
            || result.metadata.trace.contract_digest != contract_digest
        {
            return Err(ConnectorError::new("worker result binding mismatch"));
        }
        serde_json::to_string(
            &serde_json::json!({"value": result.value, "metadata": result.metadata}),
        )
        .map_err(|e| ConnectorError::new(e.to_string()))
    }
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
        let root = std::env::var("ZEROSTACK_SESSION_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let session_id = std::env::var(crate::worker::SESSION_ID_ENV)
            .unwrap_or_else(|_| format!("session-{}", std::process::id()));
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
        let host = Host::new(limits, registration)?;
        Ok(Self {
            host,
            connector,
            cancelled: Arc::new(AtomicBool::new(false)),
        })
    }
    pub fn execute(&self, source: &str, timeout: Duration) -> Result<Value, HostError> {
        if self.cancelled.load(Ordering::Acquire) || self.connector.cancellation.is_cancelled() {
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
            worker: self.connector.cancellation.clone(),
        }
    }
    pub fn cancel(&self) {
        self.cancellation().cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
