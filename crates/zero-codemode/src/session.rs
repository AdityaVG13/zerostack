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
use zero_abi::raw_worker::{ApprovalGrant as WorkerApprovalGrant, EffectClass, EngineIdentity};
use zero_abi::{
    ApprovalState, CallRequest, TOKEN_JOB_OPERATION_V1, TokenJobPollRequestV1,
    TokenJobPollResultV1, WorkerResult, WorkerTrace,
};
use zerostack_machine_permit::{
    MachinePermit, MachinePermitHeartbeat, PERMIT_HEARTBEAT_INTERVAL, PermitOwnerMetadata,
    try_scoped_permit_base_for,
};

use zero_process::{
    DEFAULT_ACTIVE_CPU_SECONDS, DEFAULT_ACTIVE_TREE_RSS_BYTES, DEFAULT_IDLE_TREE_RSS_BYTES,
    ProcessResourcePolicy, ResourceEnforcement, ResourceReceipt,
};
use zero_ref::{ZeroRefV1, ZeroScheme};
use zero_store::{
    Engine, ResolvedStore, SharedCas, current_reachability_snapshot, ensure_layout, gc_project_id,
    publish_reachability_snapshot,
};

pub const SESSION_PROTOCOL: &str = "zerostack-session/v1";
pub const MAX_SESSION_FRAME: usize = 1_048_576;
pub const SESSION_SOCKET_ENV: &str = "ZEROSTACK_SESSION_SOCKET";
pub const SESSION_TOKEN_ENV: &str = "ZEROSTACK_SESSION_TOKEN";
pub const SESSION_SHUTDOWN_TOKEN_ENV: &str = "ZEROSTACK_SESSION_SHUTDOWN_TOKEN";
const RAW_WORKER_PROTOCOL_ENV: &str = "ZEROSTACK_RAW_WORKER_PROTOCOL";
pub const SESSION_APPROVAL_SCHEMA: &str = "zerostack.session.approval_grant.v1";
pub const MAX_SESSION_APPROVAL_GRANTS: usize = 64;
const MAX_SESSION_APPROVAL_LIFETIME_MS: u64 = 300_000;
const MAX_SESSION_CONSUMED_APPROVALS: usize = 65_536;

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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionApprovalGrantV1 {
    pub schema: String,
    pub grant_id: String,
    pub engine: EngineIdentity,
    pub root: String,
    pub generation: u64,
    pub request_id: u64,
    pub operation: String,
    pub effect: EffectClass,
    pub authority_digest: String,
    pub policy_digest: String,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
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
        #[serde(default)]
        approval_grants: Vec<SessionApprovalGrantV1>,
    },
    Status {
        id: u64,
        generation: u64,
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
    ("token", "read"),
    ("token", "job"),
    ("token", "shell"),
];

// Fixed session-owned dispatchers keep admission bounded and block on the
// channel while idle. Bursts may launch at most this many raw workers total.
const AGGREGATE_DISPATCH_THREADS: usize = 3;
// One prewarmed worker per engine is the full pool. Per-engine serialization
// prevents burst workers from multiplying the aggregate native memory budget.
const AGGREGATE_WORKER_COUNT: u64 = 3;
const MAX_IDLE_WORKERS_PER_ENGINE: usize = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkerResourceReceiptV1 {
    pub engine: String,
    pub platform: String,
    pub enforcement: String,
    pub idle_tree_rss_bytes: u64,
    pub active_tree_rss_bytes: u64,
    pub cpu_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AggregateResourceReceiptV1 {
    pub schema: String,
    pub profile: String,
    pub idle_tree_rss_bytes: u64,
    pub active_tree_rss_bytes: u64,
    pub cpu_seconds: u64,
    pub hard_tree_memory_enforced: bool,
    pub workers: Vec<WorkerResourceReceiptV1>,
}

fn enforcement_name(enforcement: ResourceEnforcement) -> &'static str {
    match enforcement {
        ResourceEnforcement::WindowsJobObject => "windows_job_object",
        ResourceEnforcement::UnixInheritedPerProcess => "unix_inherited_per_process",
        ResourceEnforcement::MacOsInheritedCpu => "macos_inherited_cpu",
        ResourceEnforcement::Unsupported => "unsupported",
    }
}

struct AggregateWorkerState {
    registry: WorkerRegistry,
    workers: Mutex<BTreeMap<EngineIdentity, Vec<WorkerClient>>>,
    resource_receipts: BTreeMap<EngineIdentity, ResourceReceipt>,
    worker_config: WorkerClientConfig,
    root: PathBuf,
    session_id: String,
    pins: BTreeMap<EngineIdentity, (String, String)>,
    reachable_blobs: Mutex<BTreeMap<EngineIdentity, BTreeSet<String>>>,
    cancellation: CancellationSignal,
}

#[derive(Default)]
struct ActiveApprovals {
    grants: Vec<SessionApprovalGrantV1>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AggregateExecutionContext {
    generation: u64,
    request_id: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DispatchPermitClass {
    Analysis,
    Index,
    Heavy,
}

impl DispatchPermitClass {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Analysis => "analysis",
            Self::Index => "index",
            Self::Heavy => "heavy",
        }
    }
}

fn dispatch_permit_class(engine: EngineIdentity, operation: &str) -> Option<DispatchPermitClass> {
    if matches!(
        (engine, operation),
        (EngineIdentity::FsZero, "fs.expand")
            | (EngineIdentity::GraphZero, "expand")
            | (EngineIdentity::TokenZero, "expand")
    ) {
        return None;
    }
    if engine == EngineIdentity::GraphZero && matches!(operation, "index" | "remember") {
        return Some(DispatchPermitClass::Index);
    }
    if matches!(
        (engine, operation),
        (EngineIdentity::FsZero, "fs.edit" | "fs.write")
            | (EngineIdentity::TokenZero, "ingest" | "shell")
    ) {
        return Some(DispatchPermitClass::Heavy);
    }
    Some(DispatchPermitClass::Analysis)
}

fn dispatch_permit_slots(class: DispatchPermitClass, cores: usize) -> usize {
    match class {
        DispatchPermitClass::Analysis => (cores / 4).clamp(1, 8),
        DispatchPermitClass::Index => (cores / 8).clamp(1, 2),
        DispatchPermitClass::Heavy => 1,
    }
}

fn available_cores() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
}

fn execution_session_ref(session_id: &str, context: AggregateExecutionContext) -> String {
    format!(
        "cm://session/{}/generation/{}",
        session_id, context.generation
    )
}

fn execution_cell_ref(session_id: &str, context: AggregateExecutionContext) -> String {
    format!(
        "cm://cell/{}/generation/{}/request/{}",
        session_id, context.generation, context.request_id
    )
}

struct AggregateDispatch {
    engine: EngineIdentity,
    request: CallRequest,
    context: DispatchContext,
    execution: AggregateExecutionContext,
    completion: crate::ConnectorCompletion,
}

struct AggregateConnector {
    state: Arc<AggregateWorkerState>,
    dispatch_sender: Option<SyncSender<AggregateDispatch>>,
    dispatchers: Vec<JoinHandle<()>>,
    sequence: AtomicU64,
    approvals: Mutex<ActiveApprovals>,
    execution_context: Mutex<Option<AggregateExecutionContext>>,
}

impl AggregateConnector {
    fn new(
        root: PathBuf,
        session_id: String,
        cancellation: CancellationSignal,
        fixture_binary: Option<&Path>,
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
            let binary = if let Some(fixture_binary) = fixture_binary {
                fixture_binary.to_path_buf()
            } else {
                PathBuf::from(
                    std::env::var(binary_env)
                        .or_else(|_| {
                            fallback_env
                                .map(std::env::var)
                                .unwrap_or(Err(std::env::VarError::NotPresent))
                        })
                        .map_err(|_| {
                            HostError::Connector(format!("missing raw worker binary: {binary_env}"))
                        })?,
                )
            };
            let test_mode = cfg!(feature = "worker-fixture")
                && (fixture_binary.is_some()
                    || std::env::var("ZEROSTACK_TEST_MODE").as_deref() == Ok("1"));
            let key = engine.as_str().to_ascii_uppercase();
            let (binary, binary_hash) = pinned_worker_binary(&binary, &key)?;
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
            .env("ZEROSTACK_WORKER_REVISION", revision.clone())
            .env(
                RAW_WORKER_PROTOCOL_ENV,
                zero_abi::RAW_WORKER_PROTOCOL_VERSION,
            );
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
        let worker_config = WorkerClientConfig {
            resource_policy: Some(
                ProcessResourcePolicy::default()
                    .share(AGGREGATE_WORKER_COUNT)
                    .map_err(|error| HostError::Connector(error.to_string()))?,
            ),
            ..WorkerClientConfig::default()
        };
        let mut workers = BTreeMap::new();
        let mut resource_receipts = BTreeMap::new();
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
                .map_err(|error| {
                    HostError::Connector(format!("{} worker: {error}", engine.as_str()))
                })?;
            if let Some(receipt) = client.resource_receipt() {
                resource_receipts.insert(engine, receipt.clone());
            }
            workers.insert(engine, vec![client]);
        }
        let state = Arc::new(AggregateWorkerState {
            registry,
            workers: Mutex::new(workers),
            resource_receipts,
            worker_config,
            root,
            session_id,
            pins: configured_pins,
            reachable_blobs: Mutex::new(BTreeMap::new()),
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
            approvals: Mutex::new(ActiveApprovals::default()),
            execution_context: Mutex::new(None),
        })
    }

    fn set_execution_context(&self, context: AggregateExecutionContext) -> Result<(), HostError> {
        let mut active = self
            .execution_context
            .lock()
            .map_err(|_| HostError::Connector("execution context lock poisoned".into()))?;
        *active = Some(context);
        Ok(())
    }

    fn clear_execution_context(&self) {
        if let Ok(mut active) = self.execution_context.lock() {
            *active = None;
        }
    }

    fn execution_context(&self) -> Result<AggregateExecutionContext, ConnectorError> {
        self.execution_context
            .lock()
            .map_err(|_| ConnectorError::new("execution context lock poisoned"))?
            .ok_or_else(|| ConnectorError::new("aggregate execution context missing"))
    }
    fn install_approvals(&self, grants: Vec<SessionApprovalGrantV1>) -> Result<(), HostError> {
        let mut active = self
            .approvals
            .lock()
            .map_err(|_| HostError::Connector("approval state lock poisoned".into()))?;
        if !active.grants.is_empty() {
            return Err(HostError::Connector(
                "approval state was not cleared after the prior execution".into(),
            ));
        }
        active.grants = grants;
        Ok(())
    }

    fn clear_approvals(&self) {
        if let Ok(mut active) = self.approvals.lock() {
            active.grants.clear();
        }
    }

    fn take_approval(
        &self,
        engine: EngineIdentity,
        operation: &str,
        worker_request_id: &str,
    ) -> Result<Option<(WorkerApprovalGrant, SessionApprovalGrantV1)>, ConnectorError> {
        let mut active = self
            .approvals
            .lock()
            .map_err(|_| ConnectorError::new("approval state lock poisoned"))?;
        let Some(index) = active
            .grants
            .iter()
            .position(|grant| grant.engine == engine && grant.operation == operation)
        else {
            return Ok(None);
        };
        let grant = active.grants.remove(index);
        let original = grant.clone();
        Ok(Some((
            WorkerApprovalGrant {
                grant_id: grant.grant_id,
                engine,
                root: grant.root,
                session_id: self.state.session_id.clone(),
                request_id: worker_request_id.to_owned(),
                operation: operation.to_owned(),
                effect: grant.effect,
                authority_digest: grant.authority_digest,
                policy_digest: grant.policy_digest,
                issued_at_unix_ms: grant.issued_at_unix_ms,
                expires_at_unix_ms: grant.expires_at_unix_ms,
            },
            original,
        )))
    }

    fn restore_approval(&self, grant: SessionApprovalGrantV1) -> Result<(), ConnectorError> {
        let mut active = self
            .approvals
            .lock()
            .map_err(|_| ConnectorError::new("approval state lock poisoned"))?;
        if active
            .grants
            .iter()
            .any(|active_grant| active_grant.grant_id == grant.grant_id)
        {
            return Err(ConnectorError::new("approval reservation restore conflict"));
        }
        active.grants.push(grant);
        Ok(())
    }
}

impl Drop for AggregateConnector {
    fn drop(&mut self) {
        self.state.cancellation.cancel();
        self.dispatch_sender.take();
        for dispatcher in self.dispatchers.drain(..) {
            let _ = dispatcher.join();
        }
        let idle_workers = {
            let mut workers = self
                .state
                .workers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *workers)
                .into_values()
                .flatten()
                .collect::<Vec<_>>()
        };
        thread::scope(|scope| {
            for worker in idle_workers {
                scope.spawn(move || drop(worker));
            }
        });
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
        .env_remove(SESSION_SHUTDOWN_TOKEN_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(name) = remove_env {
        command.env_remove(name);
    }
    let owner = format!("capability-probe-{key}");
    let (child, pipes) = zero_process::VerifiedChild::spawn_tree_with_pipes(command, &owner, 0)
        .map_err(|error| HostError::Connector(format!("cannot probe {key}: {error}")))?;
    let stdout = pipes
        .stdout
        .ok_or_else(|| HostError::Connector(format!("cannot capture {key} probe stdout")))?;
    let stderr = pipes
        .stderr
        .ok_or_else(|| HostError::Connector(format!("cannot capture {key} probe stderr")))?;
    let stdout_reader = bounded_probe_reader(stdout);
    let stderr_reader = bounded_probe_reader(stderr);
    if !child.wait_for_exit(Duration::from_secs(2)) {
        let _ = child.signal_graceful_for(&owner, 0, Duration::ZERO);
        let _ = child.revoke();
        let _ = stdout_reader.join();
        let _ = stderr_reader.join();
        return Err(HostError::Connector(format!(
            "{key} capability probe timed out"
        )));
    }
    child
        .signal_graceful_for(&owner, 0, Duration::from_millis(100))
        .map_err(|error| HostError::Connector(format!("cannot settle {key} probe: {error}")))?;
    child
        .revoke()
        .map_err(|error| HostError::Connector(format!("cannot reap {key} probe: {error}")))?;
    let status = child
        .terminal_status()
        .ok_or_else(|| HostError::Connector(format!("cannot read {key} probe exit status")))?;
    let stdout = stdout_reader
        .join()
        .map_err(|_| HostError::Connector(format!("{key} stdout reader panicked")))?
        .map_err(|error| HostError::Connector(format!("cannot read {key} probe: {error}")))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| HostError::Connector(format!("{key} stderr reader panicked")))?
        .map_err(|error| HostError::Connector(format!("cannot read {key} probe: {error}")))?;
    let bytes = if stdout.is_empty() { stderr } else { stdout };
    if !status.success() && bytes.is_empty() {
        return Err(HostError::Connector(format!(
            "{key} capability probe failed without output"
        )));
    }
    String::from_utf8(bytes)
        .map_err(|error| HostError::Connector(format!("{key} probe was not UTF-8: {error}")))
}

fn bounded_probe_reader<R>(mut reader: R) -> JoinHandle<std::io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut bytes = Vec::new();
        reader
            .by_ref()
            .take((MAX_SESSION_FRAME + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_SESSION_FRAME {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "capability probe output exceeds frame bound",
            ));
        }
        Ok(bytes)
    })
}

fn digest_from_json(output: &str) -> Option<String> {
    serde_json::from_str::<Value>(output)
        .ok()?
        .get("semantic_contract_digest")?
        .as_str()
        .map(str::to_owned)
}

fn package_contract_from_json(output: &str) -> Option<String> {
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
        EngineIdentity::FsZero => package_contract_from_json(&probe_output(
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
        EngineIdentity::GraphZero => package_contract_from_json(&probe_output(
            binary,
            &["capabilities", "--json"],
            &key,
            None,
        )?),
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

#[derive(Clone, Copy)]
enum TokenOptionType {
    Bool,
    PositiveInteger,
    String,
    Mode,
}

const TOKEN_READ_OPTIONS: &[(&str, TokenOptionType)] = &[
    ("mode", TokenOptionType::Mode),
    ("start_line", TokenOptionType::PositiveInteger),
    ("end_line", TokenOptionType::PositiveInteger),
    ("raw", TokenOptionType::Bool),
    ("fresh", TokenOptionType::Bool),
    ("max_files", TokenOptionType::PositiveInteger),
    ("max_visible_tokens", TokenOptionType::PositiveInteger),
];

const TOKEN_SHELL_OPTIONS: &[(&str, TokenOptionType)] = &[
    ("cwd", TokenOptionType::String),
    ("mode", TokenOptionType::Mode),
    ("rewrite", TokenOptionType::String),
    ("no_rewrite", TokenOptionType::Bool),
    ("stdin", TokenOptionType::String),
    ("timeout_ms", TokenOptionType::PositiveInteger),
    ("timeout_seconds", TokenOptionType::PositiveInteger),
    ("background", TokenOptionType::Bool),
];

fn token_method_args(
    input: &Value,
    method: &str,
    first_key: &str,
    contract: &[(&str, TokenOptionType)],
) -> Result<Value, ConnectorError> {
    let mut args = serde_json::Map::new();
    if let Some(arguments) = input.as_array() {
        if !arguments.is_empty() && arguments.iter().all(Value::is_string) {
            // The host preserves a single argument's shape, so a string array is
            // the first value itself rather than a positional argument list.
            args.insert(first_key.into(), input.clone());
        } else {
            if arguments.is_empty() || arguments.len() > 2 {
                return Err(ConnectorError::new(format!(
                    "token.{method} requires one value and an optional options object"
                )));
            }
            args.insert(first_key.into(), arguments[0].clone());
            if let Some(options) = arguments.get(1) {
                let options = options.as_object().ok_or_else(|| {
                    ConnectorError::new(format!("token.{method} options must be an object"))
                })?;
                if options.contains_key(first_key) {
                    return Err(ConnectorError::new(format!(
                        "token.{method} options must not repeat {first_key}"
                    )));
                }
                args.extend(options.clone());
            }
        }
    } else if let Some(named) = input.as_object() {
        args = named.clone();
    } else {
        args.insert(first_key.into(), input.clone());
    }

    let first = args
        .get(first_key)
        .ok_or_else(|| ConnectorError::new(format!("token.{method} requires {first_key}")))?;
    let valid_first = first.as_str().is_some_and(|value| !value.is_empty())
        || first.as_array().is_some_and(|values| {
            !values.is_empty()
                && values
                    .iter()
                    .all(|value| value.as_str().is_some_and(|value| !value.is_empty()))
        });
    if !valid_first {
        return Err(ConnectorError::new(format!(
            "token.{method} {first_key} must be a string or non-empty string array"
        )));
    }

    for (key, value) in &args {
        if key == first_key {
            continue;
        }
        let Some((_, expected)) = contract.iter().find(|(name, _)| *name == key) else {
            let supported = contract
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
                .join(", ");
            let advice = if method == "shell" && key == "raw" {
                r#"; use { mode: "exact" } for exact shell output"#
            } else {
                ""
            };
            return Err(ConnectorError::new(format!(
                "token.{method} unknown option '{key}'; supported options: {supported}{advice}"
            )));
        };
        let valid = match expected {
            TokenOptionType::Bool => value.is_boolean(),
            TokenOptionType::PositiveInteger => value.as_u64().is_some_and(|number| number > 0),
            TokenOptionType::String => value.is_string(),
            TokenOptionType::Mode => value.as_str().is_some_and(|mode| {
                matches!(
                    mode,
                    "auto"
                        | "hybrid"
                        | "passthrough"
                        | "diagnostic"
                        | "critical"
                        | "structured"
                        | "fidelity"
                        | "dedupe"
                        | "diff-aware"
                        | "diff_aware"
                        | "diffaware"
                        | "exact"
                        | "lossy"
                )
            }),
        };
        if !valid {
            return Err(ConnectorError::new(format!(
                "token.{method} option '{key}' has an invalid value: {value}"
            )));
        }
    }
    Ok(Value::Object(args))
}

fn token_job_args(input: &Value) -> Result<Value, ConnectorError> {
    let candidate = if let Some(arguments) = input.as_array() {
        if arguments.is_empty() || arguments.len() > 2 {
            return Err(ConnectorError::new(
                "token.job requires an id and optional options object",
            ));
        }
        let id = arguments[0]
            .as_str()
            .ok_or_else(|| ConnectorError::new("token.job id must be a string"))?;
        let mut object = match arguments.get(1) {
            Some(Value::Object(options)) if !options.contains_key("id") => options.clone(),
            Some(Value::Object(_)) => {
                return Err(ConnectorError::new("token.job options must not repeat id"));
            }
            Some(_) => {
                return Err(ConnectorError::new("token.job options must be an object"));
            }
            None => serde_json::Map::new(),
        };
        object.insert("id".into(), Value::String(id.to_owned()));
        Value::Object(object)
    } else if let Some(id) = input.as_str() {
        serde_json::json!({"id":id})
    } else {
        input.clone()
    };
    let request: TokenJobPollRequestV1 = serde_json::from_value(candidate)
        .map_err(|error| ConnectorError::new(format!("invalid token.job arguments: {error}")))?;
    request
        .validate()
        .map_err(|error| ConnectorError::new(format!("invalid token.job arguments: {error}")))?;
    serde_json::to_value(request).map_err(|error| ConnectorError::new(error.to_string()))
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
        "read" => (
            "read",
            token_method_args(&input, "read", "path", TOKEN_READ_OPTIONS)?,
        ),
        "job" => (TOKEN_JOB_OPERATION_V1, token_job_args(&input)?),
        "shell" => (
            "shell",
            token_method_args(&input, "shell", "command", TOKEN_SHELL_OPTIONS)?,
        ),
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
        let execution = self.execution_context()?;
        let id = format!(
            "{}-g{}-r{}-{}",
            self.state.session_id,
            execution.generation,
            execution.request_id,
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
            cell_id: execution_cell_ref(&self.state.session_id, execution),
            request_id: id.clone(),
            trace_id: id.clone(),
            parent_span_id: Some(execution_session_ref(&self.state.session_id, execution)),
            worker_revision: revision.clone(),
            contract_digest: contract_digest.clone(),
        };
        let sender = self
            .dispatch_sender
            .as_ref()
            .ok_or_else(|| ConnectorError::new("aggregate dispatcher closed"))?;
        let taken_approval = self.take_approval(engine, &op, &id)?;
        let approval_grant = taken_approval
            .as_ref()
            .map(|(worker_grant, _)| worker_grant.clone());
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
            approval_grant,
            telemetry_request: None,
        };
        let dispatch = AggregateDispatch {
            engine,
            request,
            context,
            execution,
            completion,
        };
        match sender.try_send(dispatch) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                if let Some((_, grant)) = taken_approval {
                    self.restore_approval(grant)?;
                }
                Err(ConnectorError::new("aggregate dispatch capacity exhausted"))
            }
            Err(TrySendError::Disconnected(_)) => {
                if let Some((_, grant)) = taken_approval {
                    self.restore_approval(grant)?;
                }
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

fn normalize_aggregate_result_value(
    engine: EngineIdentity,
    operation: &str,
    value: Value,
) -> Result<Value, ConnectorError> {
    if engine != EngineIdentity::TokenZero || operation != TOKEN_JOB_OPERATION_V1 {
        return Ok(value);
    }
    let result: TokenJobPollResultV1 = serde_json::from_value(value)
        .map_err(|error| ConnectorError::new(format!("invalid token.job result: {error}")))?;
    result
        .validate()
        .map_err(|error| ConnectorError::new(format!("invalid token.job result: {error}")))?;
    serde_json::to_value(result).map_err(|error| ConnectorError::new(error.to_string()))
}
fn acquire_dispatch_permit(
    state: &AggregateWorkerState,
    dispatch: &AggregateDispatch,
) -> Result<Option<MachinePermitHeartbeat>, ConnectorError> {
    let Some(class) = dispatch_permit_class(dispatch.engine, &dispatch.request.op) else {
        return Ok(None);
    };
    let base = try_scoped_permit_base_for(class.as_str(), Some(&state.root)).map_err(|error| {
        ConnectorError::new(format!("resolve {} permit scope: {error}", class.as_str()))
    })?;
    let owner = PermitOwnerMetadata::new(
        state.root.to_string_lossy(),
        dispatch.request.op.clone(),
        execution_session_ref(&state.session_id, dispatch.execution),
        execution_cell_ref(&state.session_id, dispatch.execution),
    );
    let permit = MachinePermit::acquire_slots_with_owner_metadata(
        &base,
        dispatch_permit_slots(class, available_cores()),
        dispatch.context.deadline,
        owner,
    )
    .map_err(|error| ConnectorError::new(format!("{} permit: {error}", class.as_str())))?;
    permit
        .start_heartbeat(PERMIT_HEARTBEAT_INTERVAL)
        .map(Some)
        .map_err(|error| {
            ConnectorError::new(format!(
                "start {} permit heartbeat: {error}",
                class.as_str()
            ))
        })
}
fn engine_ref_scheme(engine: EngineIdentity) -> ZeroScheme {
    match engine {
        EngineIdentity::FsZero => ZeroScheme::Fz,
        EngineIdentity::GraphZero => ZeroScheme::Gz,
        EngineIdentity::TokenZero => ZeroScheme::Tz,
    }
}

fn retain_worker_reachability(
    state: &AggregateWorkerState,
    engine: EngineIdentity,
    refs: &[String],
) -> Result<(), ConnectorError> {
    let cas = SharedCas::open(&state.root);
    let mut batch = BTreeSet::new();
    for reference in refs {
        if !reference.contains("://blob/") {
            continue;
        }
        let parsed = ZeroRefV1::parse(reference).map_err(|error| {
            ConnectorError::new(format!(
                "invalid portable worker ref {reference:?}: {error}"
            ))
        })?;
        if parsed.scheme != engine_ref_scheme(engine) {
            return Err(ConnectorError::new(format!(
                "worker ref {reference:?} is not owned by {}",
                engine.as_str()
            )));
        }
        cas.get_verified(&parsed.hash).map_err(|error| {
            ConnectorError::new(format!(
                "worker ref {reference:?} is unavailable from authorized CAS: {error}"
            ))
        })?;
        batch.insert(parsed.hash);
    }
    let mut retained = state
        .reachable_blobs
        .lock()
        .map_err(|_| ConnectorError::new("worker reachability lock poisoned"))?;
    retained.entry(engine).or_default().extend(batch);
    Ok(())
}

fn publish_worker_reachability(state: &AggregateWorkerState) -> Result<(), HostError> {
    let project_id = gc_project_id(&state.root)
        .map_err(|error| HostError::Connector(format!("derive GC project identity: {error}")))?;
    let retained = state
        .reachable_blobs
        .lock()
        .map_err(|_| HostError::Connector("worker reachability lock poisoned".into()))?
        .clone();
    let cas = SharedCas::open(&state.root);
    for engine in [
        EngineIdentity::FsZero,
        EngineIdentity::GraphZero,
        EngineIdentity::TokenZero,
    ] {
        let hashes = retained.get(&engine).cloned().unwrap_or_default();
        for hash in &hashes {
            cas.get_verified(hash).map_err(|error| {
                HostError::Connector(format!(
                    "{} reachability object {hash} failed closure verification: {error}",
                    engine.as_str()
                ))
            })?;
        }
        let producer = engine.as_str();
        let epoch = current_reachability_snapshot(&state.root, producer, &project_id)
            .map_err(|error| {
                HostError::Connector(format!("read {producer} reachability epoch: {error}"))
            })?
            .map_or(Ok(1), |snapshot| {
                snapshot.epoch.checked_add(1).ok_or_else(|| {
                    HostError::Connector(format!("{producer} reachability epoch overflow"))
                })
            })?;
        publish_reachability_snapshot(
            &state.root,
            producer,
            &project_id,
            epoch,
            &hashes.into_iter().collect::<Vec<_>>(),
        )
        .map_err(|error| {
            HostError::Connector(format!("publish {producer} reachability: {error}"))
        })?;
    }
    Ok(())
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
    let _permit = acquire_dispatch_permit(state, dispatch)?;
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
    checkin?;
    let result: WorkerResult = result?;
    if matches!(
        result.metadata.approval.state,
        ApprovalState::Required | ApprovalState::Denied
    ) {
        return Err(ConnectorError::new("worker approval required or denied"));
    }
    if result.metadata.ownership.engine != dispatch.engine
        || result.metadata.ownership.session_id != state.session_id
        || result.metadata.trace != dispatch.request.trace
    {
        return Err(ConnectorError::new("worker result binding mismatch"));
    }
    retain_worker_reachability(state, dispatch.engine, &result.metadata.ownership.refs)?;
    let value =
        normalize_aggregate_result_value(dispatch.engine, &dispatch.request.op, result.value)?;
    serde_json::to_string(&serde_json::json!({"value": value, "metadata": result.metadata}))
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
        Self::new_authorized_with_fixture(root, session_id, None)
    }

    #[cfg(feature = "worker-fixture")]
    pub fn new_with_worker_fixture(
        root: PathBuf,
        session_id: String,
        fixture_binary: PathBuf,
    ) -> Result<Self, HostError> {
        Self::new_authorized_with_fixture(root, session_id, Some(fixture_binary))
    }

    fn new_authorized_with_fixture(
        root: PathBuf,
        session_id: String,
        fixture_binary: Option<PathBuf>,
    ) -> Result<Self, HostError> {
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
            fixture_binary.as_deref(),
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
        self.execute_with_context(0, 0, source, timeout, Vec::new())
    }

    pub fn execute_with_approvals(
        &self,
        source: &str,
        timeout: Duration,
        approval_grants: Vec<SessionApprovalGrantV1>,
    ) -> Result<Value, HostError> {
        self.execute_with_context(0, 0, source, timeout, approval_grants)
    }

    pub fn execute_with_context(
        &self,
        generation: u64,
        request_id: u64,
        source: &str,
        timeout: Duration,
        approval_grants: Vec<SessionApprovalGrantV1>,
    ) -> Result<Value, HostError> {
        if self.cancelled.load(Ordering::Acquire)
            || self.connector.state.cancellation.is_cancelled()
        {
            return Err(HostError::Connector("session cancelled".into()));
        }
        self.connector.install_approvals(approval_grants)?;
        if let Err(error) = self
            .connector
            .set_execution_context(AggregateExecutionContext {
                generation,
                request_id,
            })
        {
            self.connector.clear_approvals();
            return Err(error);
        }
        let result = self.host.execute_with_cancel_timeout_context(
            source,
            self.connector.clone(),
            self.cancelled.clone(),
            timeout,
            generation,
            request_id,
        );
        self.connector.clear_execution_context();
        self.connector.clear_approvals();
        result
    }
    pub fn aggregate_resource_receipt(&self) -> AggregateResourceReceiptV1 {
        let workers = self
            .connector
            .state
            .resource_receipts
            .iter()
            .map(|(engine, receipt)| WorkerResourceReceiptV1 {
                engine: engine.as_str().to_owned(),
                platform: receipt.platform.to_owned(),
                enforcement: enforcement_name(receipt.enforcement).to_owned(),
                idle_tree_rss_bytes: receipt.idle_tree_rss_bytes,
                active_tree_rss_bytes: receipt.active_tree_rss_bytes,
                cpu_seconds: receipt.cpu_seconds,
            })
            .collect::<Vec<_>>();
        let hard_tree_memory_enforced = workers.len() == AGGREGATE_WORKER_COUNT as usize
            && self
                .connector
                .state
                .resource_receipts
                .values()
                .all(ResourceReceipt::is_tree_enforced);
        AggregateResourceReceiptV1 {
            schema: "zerostack.session.aggregate_resource_receipt.v1".into(),
            profile: "aggregate-default".into(),
            idle_tree_rss_bytes: DEFAULT_IDLE_TREE_RSS_BYTES,
            active_tree_rss_bytes: DEFAULT_ACTIVE_TREE_RSS_BYTES,
            cpu_seconds: DEFAULT_ACTIVE_CPU_SECONDS,
            hard_tree_memory_enforced,
            workers,
        }
    }

    fn publish_reachability(&self) -> Result<(), HostError> {
        publish_worker_reachability(&self.connector.state)
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
pub const SESSION_REPLACEMENT_SETTLE_TIMEOUT: Duration = Duration::from_secs(5);
pub const SESSION_EXECUTOR_START_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateSessionFailureCode {
    InvalidGeneration,
    StaleGeneration,
    DuplicateRequestId,
    InvalidApproval,
    ApprovalReplay,
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
            Self::InvalidApproval => "invalid_approval",
            Self::ApprovalReplay => "approval_replay",
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
    root: String,
    consumed_approval_ids: BTreeSet<String>,
}

enum SessionCommand {
    Execute {
        generation: u64,
        request_id: u64,
        source: String,
        timeout: Duration,
        approval_grants: Vec<SessionApprovalGrantV1>,
        reply: SyncSender<Result<Value, HostError>>,
    },
    Status {
        reply: SyncSender<AggregateResourceReceiptV1>,
    },
    Replace {
        generation: u64,
        reply: SyncSender<Result<(), String>>,
    },
    Shutdown {
        reply: SyncSender<Result<(), String>>,
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
fn validate_session_approvals(
    state: &AggregateSessionState,
    generation: u64,
    request_id: u64,
    grants: &[SessionApprovalGrantV1],
) -> Result<Vec<String>, AggregateSessionError> {
    let invalid = |detail: String| {
        AggregateSessionError::new(
            AggregateSessionFailureCode::InvalidApproval,
            state.generation,
            Some(request_id),
            detail,
        )
    };
    if grants.len() > MAX_SESSION_APPROVAL_GRANTS {
        return Err(invalid(format!(
            "approval grant count {} exceeds maximum {MAX_SESSION_APPROVAL_GRANTS}",
            grants.len()
        )));
    }
    if state
        .consumed_approval_ids
        .len()
        .saturating_add(grants.len())
        > MAX_SESSION_CONSUMED_APPROVALS
    {
        return Err(invalid(
            "session approval replay ledger capacity exhausted".into(),
        ));
    }
    let now = now_ms();
    let mut ids = BTreeSet::new();
    for grant in grants {
        let lower_hex = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        };
        if grant.schema != SESSION_APPROVAL_SCHEMA
            || grant.grant_id.is_empty()
            || grant.grant_id.len() > 128
            || grant.operation.is_empty()
            || grant.operation.len() > 256
            || !lower_hex(&grant.authority_digest)
            || !lower_hex(&grant.policy_digest)
            || grant.issued_at_unix_ms >= grant.expires_at_unix_ms
            || grant
                .expires_at_unix_ms
                .saturating_sub(grant.issued_at_unix_ms)
                > MAX_SESSION_APPROVAL_LIFETIME_MS
        {
            return Err(invalid(format!(
                "approval grant '{}' is malformed",
                grant.grant_id
            )));
        }
        if grant.root != state.root
            || grant.generation != generation
            || grant.request_id != request_id
        {
            return Err(invalid(format!(
                "approval grant '{}' binding mismatch",
                grant.grant_id
            )));
        }
        if grant.effect != EffectClass::ApprovalRequiredMutation {
            return Err(invalid(format!(
                "approval grant '{}' has wrong effect",
                grant.grant_id
            )));
        }
        if now < grant.issued_at_unix_ms || now >= grant.expires_at_unix_ms {
            return Err(invalid(format!(
                "approval grant '{}' is expired or not yet valid",
                grant.grant_id
            )));
        }
        if !ids.insert(grant.grant_id.clone())
            || state.consumed_approval_ids.contains(&grant.grant_id)
        {
            return Err(AggregateSessionError::new(
                AggregateSessionFailureCode::ApprovalReplay,
                state.generation,
                Some(request_id),
                format!("approval grant '{}' was already consumed", grant.grant_id),
            ));
        }
    }
    Ok(ids.into_iter().collect())
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
        let root = root.canonicalize().map_err(|error| {
            AggregateSessionError::new(
                AggregateSessionFailureCode::BackendUnavailable,
                initial_generation,
                None,
                format!("cannot resolve authorized session root: {error}"),
            )
        })?;
        let root_text = root.to_string_lossy().into_owned();
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
                root: root_text,
                consumed_approval_ids: BTreeSet::new(),
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
        self.execute_with_approvals(generation, request_id, source, timeout, Vec::new())
    }

    pub fn execute_with_approvals(
        &self,
        generation: u64,
        request_id: u64,
        source: impl Into<String>,
        timeout: Duration,
        approval_grants: Vec<SessionApprovalGrantV1>,
    ) -> Result<SessionExecutionResult, AggregateSessionError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let approval_ids = {
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
            if state.seen_request_ids.contains(&request_id) {
                return Err(AggregateSessionError::new(
                    AggregateSessionFailureCode::DuplicateRequestId,
                    state.generation,
                    Some(request_id),
                    "request id was already admitted in this generation",
                ));
            }
            let approval_ids =
                validate_session_approvals(&state, generation, request_id, &approval_grants)?;
            state.seen_request_ids.insert(request_id);
            state.active_request_ids.insert(request_id);
            state
                .consumed_approval_ids
                .extend(approval_ids.iter().cloned());
            approval_ids
        };
        match self.commands.try_send(SessionCommand::Execute {
            generation,
            request_id,
            source: source.into(),
            timeout,
            approval_grants,
            reply: reply_tx,
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.release_unadmitted(generation, request_id, &approval_ids);
                return Err(AggregateSessionError::backpressure(generation, request_id));
            }
            Err(TrySendError::Disconnected(_)) => {
                self.release_unadmitted(generation, request_id, &approval_ids);
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
    pub fn resource_receipt(&self) -> Result<AggregateResourceReceiptV1, AggregateSessionError> {
        let generation = self.generation()?;
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        send_control_with_deadline(
            &self.commands,
            SessionCommand::Status { reply: reply_tx },
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
        reply_rx
            .recv_timeout(SESSION_REPLACEMENT_SETTLE_TIMEOUT)
            .map_err(|error| {
                AggregateSessionError::new(
                    AggregateSessionFailureCode::BackendUnavailable,
                    generation,
                    None,
                    format!("session resource status did not settle: {error}"),
                )
            })
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
            let closure = reply_rx.recv_timeout(settle_remaining).map_err(|error| {
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
            closure.map_err(|detail| {
                AggregateSessionError::new(
                    AggregateSessionFailureCode::BackendUnavailable,
                    generation,
                    None,
                    detail,
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

    fn release_unadmitted(&self, generation: u64, request_id: u64, approval_ids: &[String]) {
        if let Ok(mut state) = self.state.lock() {
            state.active_request_ids.remove(&request_id);
            if state.generation == generation {
                state.seen_request_ids.remove(&request_id);
            }
            for approval_id in approval_ids {
                state.consumed_approval_ids.remove(approval_id);
            }
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
                generation,
                request_id,
                source,

                timeout,
                approval_grants,
                reply,
            } => {
                let result = executor
                    .as_ref()
                    .ok_or_else(|| HostError::Runtime("session executor is unavailable".into()))
                    .and_then(|executor| {
                        executor.execute_with_context(
                            generation,
                            request_id,
                            &source,
                            timeout,
                            approval_grants,
                        )
                    });
                let _ = reply.send(result);
            }
            SessionCommand::Status { reply } => {
                if let Some(executor) = executor.as_ref() {
                    let _ = reply.send(executor.aggregate_resource_receipt());
                }
            }
            SessionCommand::Replace { generation, reply } => {
                if let Ok(mut slot) = cancellation.lock() {
                    *slot = None;
                }
                let closed = executor
                    .as_ref()
                    .ok_or_else(|| "session executor is unavailable".to_string())
                    .and_then(|executor| {
                        executor
                            .publish_reachability()
                            .map_err(|error| error.to_string())
                    });
                drop(executor.take());
                let result = closed
                    .and_then(|()| start_session_executor(generation, &root))
                    .map(|next| {
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
                let result = executor
                    .as_ref()
                    .ok_or_else(|| "session executor is unavailable".to_string())
                    .and_then(|executor| {
                        executor
                            .publish_reachability()
                            .map_err(|error| error.to_string())
                    });
                drop(executor.take());
                let _ = reply.send(result);
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
    fn session_approval_contract_is_bounded_and_replay_safe() {
        let root = "/tmp/approved-root";
        let now = now_ms();
        let grant = SessionApprovalGrantV1 {
            schema: SESSION_APPROVAL_SCHEMA.into(),
            grant_id: "grant-1".into(),
            engine: EngineIdentity::FsZero,
            root: root.into(),
            generation: 7,
            request_id: 9,
            operation: "fs.write".into(),
            effect: EffectClass::ApprovalRequiredMutation,
            authority_digest: "a".repeat(64),
            policy_digest: "b".repeat(64),
            issued_at_unix_ms: now.saturating_sub(1),
            expires_at_unix_ms: now.saturating_add(1_000),
        };
        let mut state = AggregateSessionState {
            generation: 7,
            accepting: true,
            replacing: false,
            terminating: false,
            shutdown_sent: false,
            worker_stopped: false,
            seen_request_ids: BTreeSet::new(),
            active_request_ids: BTreeSet::new(),
            root: root.into(),
            consumed_approval_ids: BTreeSet::new(),
        };
        let ids = validate_session_approvals(&state, 7, 9, std::slice::from_ref(&grant))
            .expect("valid approval");
        state.consumed_approval_ids.extend(ids);
        assert_eq!(
            validate_session_approvals(&state, 7, 9, std::slice::from_ref(&grant))
                .unwrap_err()
                .code,
            AggregateSessionFailureCode::ApprovalReplay
        );

        let mut fresh_state = state;
        fresh_state.consumed_approval_ids.clear();
        let mut wrong_root = grant.clone();
        wrong_root.root = "/tmp/other-root".into();
        assert_eq!(
            validate_session_approvals(&fresh_state, 7, 9, &[wrong_root])
                .unwrap_err()
                .code,
            AggregateSessionFailureCode::InvalidApproval
        );
        let mut wrong_effect = grant.clone();
        wrong_effect.effect = EffectClass::ReadOnly;
        assert_eq!(
            validate_session_approvals(&fresh_state, 7, 9, &[wrong_effect])
                .unwrap_err()
                .code,
            AggregateSessionFailureCode::InvalidApproval
        );
        let mut expired = grant.clone();
        expired.issued_at_unix_ms = now.saturating_sub(2);
        expired.expires_at_unix_ms = now.saturating_sub(1);
        assert_eq!(
            validate_session_approvals(&fresh_state, 7, 9, &[expired])
                .unwrap_err()
                .code,
            AggregateSessionFailureCode::InvalidApproval
        );
        assert_eq!(
            validate_session_approvals(&fresh_state, 7, 9, &[grant.clone(), grant.clone()])
                .unwrap_err()
                .code,
            AggregateSessionFailureCode::ApprovalReplay
        );
        assert_eq!(
            validate_session_approvals(
                &fresh_state,
                7,
                9,
                &vec![grant.clone(); MAX_SESSION_APPROVAL_GRANTS + 1],
            )
            .unwrap_err()
            .code,
            AggregateSessionFailureCode::InvalidApproval
        );

        let request: SessionRequest = serde_json::from_value(serde_json::json!({
            "type": "execute",
            "id": 1,
            "generation": 7,
            "root": root,
            "source": "return null"
        }))
        .unwrap();
        assert!(matches!(
            request,
            SessionRequest::Execute { approval_grants, .. } if approval_grants.is_empty()
        ));
    }

    #[test]
    fn unqueued_request_releases_request_and_approval_reservations() {
        let state = Arc::new(Mutex::new(AggregateSessionState {
            generation: 7,
            accepting: true,
            replacing: false,
            terminating: false,
            shutdown_sent: false,
            worker_stopped: false,
            seen_request_ids: BTreeSet::from([9]),
            active_request_ids: BTreeSet::from([9]),
            root: "/tmp/approved-root".into(),
            consumed_approval_ids: BTreeSet::from(["grant-1".into(), "grant-2".into()]),
        }));
        let (commands, _receiver) = mpsc::sync_channel(1);
        let session = AggregateSession {
            state: Arc::clone(&state),
            commands,
            cancellation: Arc::new(Mutex::new(None)),
            worker: Mutex::new(None),
        };

        session.release_unadmitted(7, 9, &["grant-1".into()]);

        let state = state.lock().unwrap();
        assert!(!state.seen_request_ids.contains(&9));
        assert!(!state.active_request_ids.contains(&9));
        assert!(!state.consumed_approval_ids.contains("grant-1"));
        assert!(state.consumed_approval_ids.contains("grant-2"));
    }

    #[cfg(unix)]
    #[test]
    fn graph_contract_probe_uses_typed_capabilities_not_help_scraping() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("graphzero-codemode");
        let digest = "a".repeat(64);
        let script = format!(
            "#!/bin/sh\nif [ \"$#\" -eq 2 ] && [ \"$1\" = capabilities ] && [ \"$2\" = --json ]; then\n  printf '%s\n' '{{\"package\":{{\"abi_digest\":\"{digest}\"}}}}'\n  exit 0\nfi\necho forbidden probe arguments >&2\nexit 9\n"
        );
        std::fs::write(&binary, script).unwrap();
        let mut permissions = std::fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&binary, permissions).unwrap();

        assert_eq!(
            probe_contract(EngineIdentity::GraphZero, &binary, false).unwrap(),
            digest
        );

        std::fs::write(
            &binary,
            "#!/bin/sh\nprintf '%s\n' '{\"package\":{\"abi_digest\":\"short\"}}'\n",
        )
        .unwrap();
        let error = probe_contract(EngineIdentity::GraphZero, &binary, false)
            .expect_err("invalid Graph digest must fail closed");
        assert!(
            error.to_string().contains("invalid contract digest"),
            "{error}"
        );
    }

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
    fn token_read_and_shell_options_are_strict_and_forwarded_once() {
        assert!(METHODS.contains(&("token", "read")));
        assert_lower(
            "token",
            "read",
            json!(["fresh-raw.txt", {
                "mode":"exact","start_line":1,"end_line":2,"raw":true,
                "fresh":true,"max_files":1,"max_visible_tokens":512
            }]),
            EngineIdentity::TokenZero,
            "read",
            json!({
                "path":"fresh-raw.txt","mode":"exact","start_line":1,
                "end_line":2,"raw":true,"fresh":true,"max_files":1,
                "max_visible_tokens":512
            }),
        );
        assert_lower(
            "token",
            "read",
            json!(["one.txt", "two.txt"]),
            EngineIdentity::TokenZero,
            "read",
            json!({"path":["one.txt","two.txt"]}),
        );
        assert_lower(
            "token",
            "shell",
            json!([["printf", "ok"], {
                "cwd":".","mode":"exact","rewrite":"off","no_rewrite":true,
                "stdin":"input","timeout_ms":25,"timeout_seconds":1,"background":false
            }]),
            EngineIdentity::TokenZero,
            "shell",
            json!({
                "command":["printf","ok"],"cwd":".","mode":"exact",
                "rewrite":"off","no_rewrite":true,"stdin":"input",
                "timeout_ms":25,"timeout_seconds":1,"background":false
            }),
        );

        for input in [
            json!(["file", {"unknown":true}]),
            json!(["file", {"fresh":"yes"}]),
            json!(["file", {"max_files":0}]),
            json!(["file", {}, "extra"]),
        ] {
            assert!(lower("token", "read", input).is_err());
        }
        let shell_raw = lower(
            "token",
            "shell",
            json!(["printf must-not-run", {"raw":true}]),
        )
        .unwrap_err()
        .to_string();
        assert!(shell_raw.contains("unknown option 'raw'"), "{shell_raw}");
        assert!(shell_raw.contains(r#"mode: "exact""#), "{shell_raw}");
        assert!(lower("token", "shell", json!(["printf ok", {"timeout_ms":0}])).is_err());
        assert!(lower("token", "shell", json!(["printf ok", {}, "extra"])).is_err());
    }

    #[test]
    fn token_job_lowering_uses_the_shared_typed_request() {
        assert!(METHODS.contains(&("token", "job")));
        assert_lower(
            "token",
            "job",
            json!("tzjob-7"),
            EngineIdentity::TokenZero,
            "job",
            json!({"id":"tzjob-7","waitMs":30000,"since":0,"tailBytes":8192}),
        );
        assert_lower(
            "token",
            "job",
            json!(["tzjob-7", {"waitMs":25,"since":9,"tailBytes":64}]),
            EngineIdentity::TokenZero,
            "job",
            json!({"id":"tzjob-7","waitMs":25,"since":9,"tailBytes":64}),
        );
        assert!(lower("token", "job", json!(["tzjob-7", {"extra":true}])).is_err());
        assert!(lower("token", "job", json!(["tzjob-7", {"tailBytes":0}])).is_err());
        assert!(lower("token", "job", json!(["tzjob-7", {}, "extra"])).is_err());
    }

    #[test]
    fn token_job_result_is_revalidated_at_the_aggregate_boundary() {
        let canonical = json!({
            "id":"tzjob-7","status":"running","pid":42,"tail":"ok\n",
            "tailUtf8Lossless":true,"tailBytes":3,"logBytes":3,"cursor":3,
            "version":2,"changed":true,"nextPollMs":20000
        });
        assert_eq!(
            normalize_aggregate_result_value(
                EngineIdentity::TokenZero,
                TOKEN_JOB_OPERATION_V1,
                canonical.clone(),
            )
            .unwrap(),
            canonical
        );

        let mut unknown = canonical.clone();
        unknown["log"] = json!("/private/session.log");
        assert!(
            normalize_aggregate_result_value(
                EngineIdentity::TokenZero,
                TOKEN_JOB_OPERATION_V1,
                unknown,
            )
            .is_err()
        );

        let mut false_exactness = canonical;
        false_exactness["tailBytes"] = json!(2);
        false_exactness["cursor"] = json!(2);
        false_exactness["logBytes"] = json!(2);
        assert!(
            normalize_aggregate_result_value(
                EngineIdentity::TokenZero,
                TOKEN_JOB_OPERATION_V1,
                false_exactness,
            )
            .is_err()
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
    #[test]
    fn dispatch_permit_defaults_and_expand_exception_are_bounded() {
        assert_eq!(dispatch_permit_slots(DispatchPermitClass::Analysis, 1), 1);
        assert_eq!(dispatch_permit_slots(DispatchPermitClass::Analysis, 32), 8);
        assert_eq!(dispatch_permit_slots(DispatchPermitClass::Index, 1), 1);
        assert_eq!(dispatch_permit_slots(DispatchPermitClass::Index, 32), 2);
        assert_eq!(dispatch_permit_slots(DispatchPermitClass::Heavy, 128), 1);
        assert_eq!(
            dispatch_permit_class(EngineIdentity::TokenZero, "expand"),
            None
        );
        assert_eq!(
            dispatch_permit_class(EngineIdentity::FsZero, "fs.search"),
            Some(DispatchPermitClass::Analysis)
        );
        assert_eq!(
            dispatch_permit_class(EngineIdentity::GraphZero, "index"),
            Some(DispatchPermitClass::Index)
        );
    }

    #[test]
    fn execution_context_refs_bind_generation_and_request() {
        let context = AggregateExecutionContext {
            generation: 7,
            request_id: 19,
        };
        assert_eq!(
            execution_session_ref("session-7", context),
            "cm://session/session-7/generation/7"
        );
        assert_eq!(
            execution_cell_ref("session-7", context),
            "cm://cell/session-7/generation/7/request/19"
        );
    }
}
