#![forbid(unsafe_code)]

pub mod checks;
pub mod fake_substrate;
pub mod oracle;
pub mod patterns;
pub mod plan;
pub mod racc;
pub mod raw_worker;
pub mod report;
pub mod schema;
pub mod testkit_bridge;

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

pub const CONTRACT_VERSION: &str = "1.0";
/// Raw-worker v2 conformance contract label, distinct from the plan-level
/// `CONTRACT_VERSION`. A `*-codemode` report carries this so a report cannot
/// overclaim plan-level scope.
pub const RAW_WORKER_CONTRACT_VERSION: &str = "raw-worker-v2";
/// Stable compatibility projection of the authoritative GATE_MAPPINGS table
/// (plan-level G1-G10).
pub const CHECK_IDS: [&str; 10] = ["G1", "G2", "G3", "G4", "G5", "G6", "G7", "G8", "G9", "G10"];
/// Stable projection of the authoritative RAW_GATE_MAPPINGS table
/// (raw-worker v2 RW1-RW10).
pub const RAW_CHECK_IDS: [&str; 10] = [
    "RW1", "RW2", "RW3", "RW4", "RW5", "RW6", "RW7", "RW8", "RW9", "RW10",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ns {
    Fz,
    Tz,
    Gz,
}

impl Ns {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "fz" => Ok(Self::Fz),
            "tz" => Ok(Self::Tz),
            "gz" => Ok(Self::Gz),
            _ => bail!("unsupported namespace {value:?}; expected fz, tz, or gz"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fz => "fz",
            Self::Tz => "tz",
            Self::Gz => "gz",
        }
    }

    pub fn tool_names(self) -> [String; 3] {
        let ns = self.as_str();
        [
            format!("{ns}_execute_code"),
            format!("{ns}_codemode_search"),
            format!("{ns}_codemode_describe"),
        ]
    }

    pub fn ref_regex(self) -> Regex {
        patterns::substrate_ref_re(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityManifest {
    pub contract_version: String,
    pub ns: String,
    pub mutation: String,
    pub plan_forms: Vec<String>,
    #[serde(default)]
    pub limits: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Telemetry {
    pub kind: String,
    pub status: String,
    pub logical_ops: u64,
    pub physical_ops: u64,
    pub batched_ops: u64,
    pub internal_actions: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub store_writes: u64,
    pub wall_ms: u64,
    pub bytes_materialized: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContractError {
    pub kind: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    pub execution_id: String,
    pub ns: String,
    pub status: String,
    pub refs: BTreeMap<String, String>,
    pub telemetry: Telemetry,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ContractError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GateStatus {
    Pass,
    Fail,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompletionStatus {
    Complete,
    Partial,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckResult {
    pub id: String,
    pub name: String,
    pub passed: bool,
    pub status: GateStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    pub details: Vec<String>,
}

impl CheckResult {
    pub fn pass(id: &str, name: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            passed: true,
            status: GateStatus::Pass,
            skip_reason: None,
            details: Vec::new(),
        }
    }
    pub fn fail(id: &str, name: &str, detail: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            passed: false,
            status: GateStatus::Fail,
            skip_reason: None,
            details: vec![detail.into()],
        }
    }
    pub fn skip(id: &str, name: &str, reason: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            passed: false,
            status: GateStatus::Skipped,
            skip_reason: Some(reason.into()),
            details: Vec::new(),
        }
    }
    pub fn with_details(id: &str, name: &str, details: Vec<String>) -> Self {
        if details.is_empty() {
            Self::pass(id, name)
        } else {
            Self {
                id: id.into(),
                name: name.into(),
                passed: false,
                status: GateStatus::Fail,
                skip_reason: None,
                details,
            }
        }
    }
}

pub fn required_gate_ids(surface: Surface) -> Vec<&'static str> {
    match surface {
        // Plan-level G1-G10 require every plan gate.
        Surface::Planner => CHECK_IDS.to_vec(),
        // Raw-worker RW1-RW10 require every raw gate (distinct from G1-G10).
        Surface::Codemode => RAW_CHECK_IDS.to_vec(),
        // MCP only exercises exposure.
        Surface::Mcp => vec!["G1"],
    }
}

/// Full-conformance scope ids for a surface. `Complete` requires every id in
/// this set to be present and non-skipped (in addition to all required ids
/// passing). MCP is capped at Partial because it only exercises exposure:
/// its emitted G2-G10 skips never satisfy the plan-level scope.
fn scope_gate_ids(surface: Surface) -> &'static [&'static str] {
    match surface {
        Surface::Planner => &CHECK_IDS,
        Surface::Codemode => &RAW_CHECK_IDS,
        Surface::Mcp => &CHECK_IDS,
    }
}

/// Report contract_version label per surface so a report cannot overclaim
/// plan-level scope when it only ran raw-worker gates.
fn surface_contract_version(surface: Surface) -> &'static str {
    match surface {
        Surface::Codemode => RAW_WORKER_CONTRACT_VERSION,
        Surface::Planner | Surface::Mcp => CONTRACT_VERSION,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceReport {
    pub ns: String,
    pub bin: String,
    pub contract_version: String,
    pub surface: String,
    pub completion_status: CompletionStatus,
    pub passed: bool,
    pub checks: Vec<CheckResult>,
}

impl ConformanceReport {
    pub fn new(ns: Ns, bin: impl Into<String>, surface: Surface, checks: Vec<CheckResult>) -> Self {
        let required = required_gate_ids(surface);
        let required_failed = checks
            .iter()
            .any(|c| required.contains(&c.id.as_str()) && c.status == GateStatus::Fail);
        let required_passed = required.iter().all(|id| {
            checks
                .iter()
                .any(|c| c.id == *id && c.status == GateStatus::Pass)
        });
        // Full scope is surface-specific: it never assumes the plan-level G
        // mapping for a raw-worker (codemode) report.
        let scope = scope_gate_ids(surface);
        let full_scope = scope.iter().all(|id| {
            checks
                .iter()
                .any(|check| check.id == *id && check.status != GateStatus::Skipped)
        });
        let completion_status = if required_failed {
            CompletionStatus::Failed
        } else if required_passed && full_scope {
            CompletionStatus::Complete
        } else {
            CompletionStatus::Partial
        };
        let passed = completion_status == CompletionStatus::Complete && required_passed;
        Self {
            ns: ns.as_str().into(),
            bin: bin.into(),
            contract_version: surface_contract_version(surface).into(),
            surface: surface.as_str().into(),
            completion_status,
            passed,
            checks,
        }
    }

    pub fn write_to_reports_dir(&self, reports_dir: &Path) -> Result<PathBuf> {
        fs::create_dir_all(reports_dir)
            .with_context(|| format!("creating {}", reports_dir.display()))?;
        let stamp = chrono::Local::now().format("%Y-%m-%d-%H%M%S");
        let path = reports_dir.join(format!("{}-{stamp}.json", self.ns));
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }
}

pub fn schema_paths() -> Vec<PathBuf> {
    [
        "contracts/capability-manifest.schema.json",
        "contracts/telemetry.schema.json",
        "contracts/error.schema.json",
        "contracts/execution-record.schema.json",
        "contracts/limits.schema.json",
        "contracts/raw-worker-v2.schema.json",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

fn push_manifest_error(errors: &mut Vec<String>, invalid: bool, message: String) {
    if invalid {
        errors.push(message);
    }
}

pub fn validate_capability_manifest(ns: Ns, value: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    let manifest: CapabilityManifest = match serde_json::from_value(value.clone()) {
        Ok(manifest) => manifest,
        Err(err) => {
            return vec![format!(
                "capabilities are not shaped like a manifest: {err}"
            )]
        }
    };

    push_manifest_error(
        &mut errors,
        manifest.contract_version != CONTRACT_VERSION,
        format!(
            "contract_version is {:?}, expected {CONTRACT_VERSION:?}",
            manifest.contract_version
        ),
    );
    push_manifest_error(
        &mut errors,
        manifest.ns != ns.as_str(),
        format!("ns is {:?}, expected {:?}", manifest.ns, ns.as_str()),
    );
    if !matches!(
        manifest.mutation.as_str(),
        "allowed" | "denied" | "readonly" | "store_only"
    ) {
        errors.push(format!("invalid mutation {:?}", manifest.mutation));
    }
    for required in ["recipe", "json", "js"] {
        if !manifest.plan_forms.iter().any(|form| form == required) {
            errors.push(format!("plan_forms missing {required:?}"));
        }
    }
    let allowed_limits: BTreeSet<&str> = [
        "max_logical_ops",
        "max_physical_ops",
        "max_wall_ms",
        "hard_max_wall_ms",
        "max_microtasks",
        "max_memory_bytes",
        "max_output_bytes",
        "max_result_ref_bytes",
        "max_refs_emitted",
        "max_parallel_width",
        "max_code_bytes",
    ]
    .into_iter()
    .collect();
    for (name, value) in &manifest.limits {
        if !allowed_limits.contains(name.as_str()) {
            errors.push(format!("unknown echoed limit {name:?}"));
        }
        if *value == 0 {
            errors.push(format!("limit {name:?} must be positive"));
        }
    }
    errors
}

pub fn validate_telemetry(value: &Value) -> Vec<String> {
    let allowed: BTreeSet<&str> = [
        "kind",
        "status",
        "logical_ops",
        "physical_ops",
        "batched_ops",
        "internal_actions",
        "cache_hits",
        "cache_misses",
        "store_writes",
        "wall_ms",
        "bytes_materialized",
        "extra",
    ]
    .into_iter()
    .collect();
    let required: BTreeSet<&str> = allowed
        .iter()
        .copied()
        .filter(|key| *key != "extra")
        .collect();
    validate_object_keys("telemetry", value, &required, &allowed)
}

pub fn validate_error(value: &Value) -> Vec<String> {
    let allowed: BTreeSet<&str> = ["kind", "message", "retryable"].into_iter().collect();
    let required = allowed.clone();
    let mut errors = validate_object_keys("error", value, &required, &allowed);
    match value.get("kind").and_then(Value::as_str) {
        Some("validation" | "sandbox" | "runtime" | "substrate" | "policy") => {}
        Some(other) => errors.push(format!("invalid error kind {other:?}")),
        None => {}
    }
    if value
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(str::is_empty)
    {
        errors.push("error.message must be non-empty".into());
    }
    if value.get("retryable").is_some_and(|v| !v.is_boolean()) {
        errors.push("error.retryable must be boolean".into());
    }
    errors
}

pub fn validate_execution_record(ns: Ns, value: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    let record: ExecutionRecord = match serde_json::from_value(value.clone()) {
        Ok(record) => record,
        Err(err) => return vec![format!("execution record has invalid shape: {err}")],
    };
    if !valid_execution_id(&record.execution_id) {
        errors.push(format!("invalid execution_id {:?}", record.execution_id));
    }
    if record.ns != ns.as_str() {
        errors.push(format!(
            "record ns is {:?}, expected {:?}",
            record.ns,
            ns.as_str()
        ));
    }
    if !matches!(record.status.as_str(), "ok" | "error") {
        errors.push(format!("invalid status {:?}", record.status));
    }
    let ref_re = ns.ref_regex();
    for (part, value) in &record.refs {
        if !ref_re.is_match(value) {
            errors.push(format!("invalid {part} ref {value:?}"));
        }
    }
    match serde_json::to_value(&record.telemetry) {
        Ok(telemetry) => errors.extend(validate_telemetry(&telemetry)),
        Err(err) => errors.push(format!(
            "could not serialize telemetry for validation: {err}"
        )),
    }
    if let Some(error) = record.error {
        match serde_json::to_value(error) {
            Ok(error) => errors.extend(validate_error(&error)),
            Err(err) => errors.push(format!("could not serialize error for validation: {err}")),
        }
    }
    errors
}

pub fn valid_execution_id(value: &str) -> bool {
    patterns::execution_id_re().is_match(value)
}

pub fn valid_ref(ns: Ns, value: &str) -> bool {
    ns.ref_regex().is_match(value)
}

pub fn collect_refs(value: &Value) -> Vec<String> {
    let mut refs = Vec::new();
    collect_refs_inner(value, &mut refs);
    refs
}

fn collect_refs_inner(value: &Value, refs: &mut Vec<String>) {
    match value {
        Value::String(value) => {
            if ["fz://", "gz://", "tz://", "cm://"]
                .iter()
                .any(|prefix| value.starts_with(prefix))
            {
                refs.push(value.clone());
            }
        }
        Value::Array(values) => values
            .iter()
            .for_each(|value| collect_refs_inner(value, refs)),
        Value::Object(map) => map
            .values()
            .for_each(|value| collect_refs_inner(value, refs)),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn validate_object_keys(
    name: &str,
    value: &Value,
    required: &BTreeSet<&str>,
    allowed: &BTreeSet<&str>,
) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(object) = value.as_object() else {
        return vec![format!("{name} must be an object")];
    };
    for key in required {
        if !object.contains_key(*key) {
            errors.push(format!("{name} missing required key {key:?}"));
        }
    }
    for key in object.keys() {
        if !allowed.contains(key.as_str()) {
            errors.push(format!("{name} has unknown top-level key {key:?}"));
        }
        if key == "raw_leak" {
            errors.push(format!("{name} must not contain raw_leak"));
        }
    }
    errors
}

/// The surface an installed artifact serves. Three distinct conformance
/// layers:
/// - `Planner`: a planner host serving `{ns}_execute_code` over JSON-RPC; driven
///   by the plan-level G1-G10 gates (`plan`).
/// - `Codemode`: a planner-free raw-worker v2 binary; driven by the raw-worker
///   RW1-RW10 gates (`raw_worker`).
/// - `Mcp`: an MCP server; only G1 exposure applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Planner,
    Codemode,
    Mcp,
}

impl Surface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planner => "planner",
            Self::Codemode => "codemode",
            Self::Mcp => "mcp",
        }
    }

    /// The surface this artifact must NOT serve.
    pub fn opposite(self) -> Self {
        match self {
            Self::Codemode | Self::Planner => Self::Mcp,
            Self::Mcp => Self::Codemode,
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "codemode" | "code-mode" | "code_mode" => Ok(Self::Codemode),
            "planner" | "plan" => Ok(Self::Planner),
            "mcp" => Ok(Self::Mcp),
            other => bail!("unknown surface {other:?}; expected 'planner', 'codemode', or 'mcp'"),
        }
    }
}

pub struct RunConfig {
    pub ns: Ns,
    /// The single installed artifact under test.
    pub bin: PathBuf,
    /// The surface that artifact is built to serve.
    pub surface: Surface,
    pub reports_dir: PathBuf,
    pub timeout: Duration,
}

impl RunConfig {
    /// One artifact, one surface.
    ///
    /// Surfaces are mutually exclusive by product rule: you install either
    /// `<engine>-codemode` or `<engine>-mcp`, from the same revision and
    /// shared core, and the installer replaces any prior surface. Dual catalog
    /// startup fails closed, and TokenZero makes a dual build a
    /// `compile_error!`. So conformance runs against whichever artifact is
    /// actually installed; asking for both at once describes a system that is
    /// not allowed to exist. See zerostack-hix.
    pub fn new(ns: Ns, bin: PathBuf, surface: Surface, reports_dir: PathBuf) -> Self {
        Self {
            ns,
            bin,
            surface,
            reports_dir,
            timeout: Duration::from_secs(5),
        }
    }
}

pub fn run_conformance(config: &RunConfig) -> ConformanceReport {
    // MCP: JSON-RPC exposure only; G2-G10 are skipped (plan execution required).
    if config.surface == Surface::Mcp {
        let mut checks = Vec::new();
        checks.push(match check_exposure(config) {
            Ok(check) => check,
            Err(err) => CheckResult::fail("G1", "exposure", err.to_string()),
        });
        for (id, name) in [
            ("G2", "refs"),
            ("G3", "telemetry"),
            ("G4", "leak-proof"),
            ("G5", "errors"),
            ("G6", "ctx.step"),
            ("G7", "limits"),
            ("G8", "mutation"),
            ("G9", "coalescing"),
            ("G10", "sandbox-denial"),
        ] {
            checks.push(CheckResult::skip(
                id,
                name,
                "not applicable to the MCP surface; requires planner execution",
            ));
        }
        return ConformanceReport::new(config.ns, substrate_label(config), config.surface, checks);
    }

    // Planner: the canonical plan-level G1-G10 layer (drives execute_code).
    if config.surface == Surface::Planner {
        let checks = crate::plan::run_conformance(config.ns, &config.bin, config.timeout);
        return ConformanceReport::new(config.ns, substrate_label(config), config.surface, checks);
    }

    // Codemode: a planner-free raw-worker v2 binary, driven by RW1-RW10. It is
    // NEVER driven by MCP framing and NEVER by the plan-level G gates.
    let checks = crate::raw_worker::run_conformance(config.ns, &config.bin, config.timeout);
    ConformanceReport::new(config.ns, substrate_label(config), config.surface, checks)
}

/// Surface label for the report. Records basenames, never the authoring
/// machine's absolute paths.
fn substrate_label(config: &RunConfig) -> String {
    config
        .bin
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| config.bin.display().to_string())
}

/// G1 exposure: this artifact serves its own surface and refuses the other.
///
/// Surfaces are mutually exclusive: you install either the CodeMode artifact
/// or the MCP artifact, never both. So exposure is a property of ONE artifact,
/// checked two ways:
///
/// 1. It serves the tool set its surface implies. A CodeMode artifact exposes
///    exactly the CodeMode tools; an MCP artifact exposes none of them.
/// 2. It REFUSES the opposite surface. Asking a single-surface binary for the
///    other surface must fail closed rather than quietly serve a second
///    catalog, which is the invariant that actually protects the separation.
///
/// This previously spawned one binary twice with different `--mode=` flags,
/// then briefly required two artifacts at once. Both describe a dual-surface
/// system that is not allowed to exist.
fn check_exposure(config: &RunConfig) -> Result<CheckResult> {
    let codemode_tools: BTreeSet<String> = config.ns.tool_names().into_iter().collect();

    let mut client = McpClient::spawn(&config.bin, None, config.timeout)?;
    client.initialize()?;
    let served: BTreeSet<String> = client.list_tools()?.into_iter().collect();

    let mut details = check_own_tool_catalog(config.surface, served, &codemode_tools);
    // Opposite spawn Err is a pass: fail-closed refusal of the other surface.
    details.extend(check_opposite_surface_refused(
        &config.bin,
        config.surface,
        config.timeout,
    ));

    Ok(CheckResult::with_details("G1", "exposure", details))
}

/// Probe A: own catalog matches the surface contract (exact set vs leak filter).
fn check_own_tool_catalog(
    surface: Surface,
    served: BTreeSet<String>,
    codemode_tools: &BTreeSet<String>,
) -> Vec<String> {
    let mut details = Vec::new();
    match surface {
        // A planner host serves exactly the three CodeMode tools.
        Surface::Planner | Surface::Codemode => {
            if &served != codemode_tools {
                details.push(format!(
                    "artifact served {served:?}, expected exactly {codemode_tools:?}"
                ));
            }
        }
        Surface::Mcp => {
            let leaked: Vec<String> = served
                .into_iter()
                .filter(|name| name.contains("codemode") || codemode_tools.contains(name))
                .collect();
            if !leaked.is_empty() {
                details.push(format!("mcp artifact exposed codemode tools: {leaked:?}"));
            }
        }
    }
    details
}

/// Probe B: opposite surface must fail closed.
///
/// Spawn `Err` or incomplete serve (init/list not both Ok) is intentional pass.
/// Fully serving the opposite surface is the dual-catalog failure.
fn check_opposite_surface_refused(
    bin: &Path,
    own_surface: Surface,
    timeout: Duration,
) -> Vec<String> {
    let opposite = own_surface.opposite();
    match McpClient::spawn(bin, Some(opposite.as_str()), timeout) {
        Err(_) => Vec::new(),
        Ok(mut wrong) => {
            if wrong.initialize().is_ok() && wrong.list_tools().is_ok() {
                vec![format!(
                    "artifact is built for surface {} but also served {};                      surfaces must be mutually exclusive",
                    own_surface.as_str(),
                    opposite.as_str()
                )]
            } else {
                Vec::new()
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ReceiveDeadlineError {
    Timeout,
    Disconnected,
}

fn recv_until_deadline<T>(
    receiver: &std::sync::mpsc::Receiver<T>,
    deadline: Instant,
) -> std::result::Result<T, ReceiveDeadlineError> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or(ReceiveDeadlineError::Timeout)?;
    receiver
        .recv_timeout(remaining)
        .map_err(|error| match error {
            std::sync::mpsc::RecvTimeoutError::Timeout => ReceiveDeadlineError::Timeout,
            std::sync::mpsc::RecvTimeoutError::Disconnected => ReceiveDeadlineError::Disconnected,
        })
}

pub(crate) struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout_lines: std::sync::mpsc::Receiver<std::io::Result<String>>,
    stdout_reader: Option<std::thread::JoinHandle<()>>,
    next_id: u64,
    timeout: Duration,
}

fn spawn_args(mode: Option<&str>) -> Vec<String> {
    mode.map(|value| format!("--mode={value}"))
        .into_iter()
        .collect()
}

fn notification_message(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

fn write_notification<W: Write>(writer: &mut W, method: &str, params: Value) -> Result<()> {
    let notification = notification_message(method, params);
    writeln!(writer, "{}", serde_json::to_string(&notification)?)?;
    writer.flush()?;
    Ok(())
}

impl McpClient {
    pub(crate) fn spawn(bin: &Path, mode: Option<&str>, timeout: Duration) -> Result<Self> {
        let args = spawn_args(mode);
        let mut command = Command::new(bin);
        command
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().with_context(|| {
            if args.is_empty() {
                format!("spawning {}", bin.display())
            } else {
                format!("spawning {} {}", bin.display(), args.join(" "))
            }
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("missing child stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("missing child stdout"))?;
        let (stdout_sender, stdout_lines) = std::sync::mpsc::channel();
        let stdout_reader = std::thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match stdout.read_line(&mut line) {
                    Ok(0) => {
                        let _ = stdout_sender.send(Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "MCP server stdout closed",
                        )));
                        break;
                    }
                    Ok(_) => {
                        if stdout_sender.send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = stdout_sender.send(Err(error));
                        break;
                    }
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            stdout_lines,
            stdout_reader: Some(stdout_reader),
            next_id: 1,
            timeout,
        })
    }

    pub(crate) fn initialize(&mut self) -> Result<Value> {
        let response = self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "zerostack-codemode-conformance", "version": env!("CARGO_PKG_VERSION") }
            }),
        )?;
        write_notification(&mut self.stdin, "notifications/initialized", json!({}))?;
        Ok(response)
    }

    pub(crate) fn list_tools(&mut self) -> Result<Vec<String>> {
        let response = self.request("tools/list", json!({}))?;
        let tools = response
            .pointer("/result/tools")
            .or_else(|| response.get("tools"))
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("tools/list response missing tools array: {response}"))?;
        Ok(tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
            .collect())
    }

    pub(crate) fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(self.stdin, "{}", serde_json::to_string(&request)?)?;
        self.stdin.flush()?;
        self.wait_for_matching_response(id, method)
    }

    /// Poll stdout lines until a JSON-RPC response with `id` arrives (or timeout/EOF).
    ///
    /// Non-matching ids (notifications / out-of-order) are ignored; empty lines skipped.
    fn wait_for_matching_response(&mut self, id: u64, method: &str) -> Result<Value> {
        let deadline = Instant::now()
            .checked_add(self.timeout)
            .ok_or_else(|| anyhow!("invalid timeout waiting for {method} response"))?;
        loop {
            let received = recv_until_deadline(&self.stdout_lines, deadline);
            let line = match received {
                Ok(Ok(line)) => line,
                Ok(Err(error)) => {
                    self.terminate_child();
                    return Err(error).with_context(|| {
                        format!("reading MCP stdout while waiting for {method} response")
                    });
                }
                Err(ReceiveDeadlineError::Timeout) => {
                    self.terminate_child();
                    bail!("timeout waiting for {method} response");
                }
                Err(ReceiveDeadlineError::Disconnected) => {
                    self.terminate_child();
                    bail!("MCP stdout reader disconnected while waiting for {method} response");
                }
            };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let response: Value =
                serde_json::from_str(line).with_context(|| format!("parsing MCP line {line:?}"))?;
            if response.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = response.get("error") {
                    bail!("MCP error for {method}: {error}");
                }
                return Ok(response);
            }
        }
    }

    fn terminate_child(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.terminate_child();
        if let Some(reader) = self.stdout_reader.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_args_omit_mode_for_standalone_artifacts() {
        assert!(spawn_args(None).is_empty());
        assert_eq!(spawn_args(Some("mcp")), vec!["--mode=mcp"]);
    }

    #[test]
    fn notification_message_has_no_id_and_expected_params() {
        assert_eq!(
            notification_message("notifications/initialized", json!({})),
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
                "params": {}
            })
        );
    }

    #[test]
    fn write_notification_serializes_and_flushes() {
        let mut output = FlushCapture::default();
        write_notification(&mut output, "notifications/initialized", json!({})).unwrap();
        assert_eq!(output.flushes, 1);
        let line: Value = serde_json::from_slice(&output.bytes).unwrap();
        assert_eq!(line["jsonrpc"], "2.0");
        assert_eq!(line["method"], "notifications/initialized");
        assert!(line.get("id").is_none());
        assert_eq!(line["params"], json!({}));
    }

    #[derive(Default)]
    struct FlushCapture {
        bytes: Vec<u8>,
        flushes: usize,
    }

    impl Write for FlushCapture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    #[test]
    fn receive_deadline_reports_expired_deadline_without_waiting() {
        let (_sender, receiver) = std::sync::mpsc::channel::<()>();
        assert_eq!(
            recv_until_deadline(&receiver, Instant::now() - Duration::from_secs(1)),
            Err(ReceiveDeadlineError::Timeout)
        );
    }

    #[test]
    fn receive_deadline_preserves_disconnect_evidence() {
        let (sender, receiver) = std::sync::mpsc::channel::<()>();
        drop(sender);
        assert_eq!(
            recv_until_deadline(&receiver, Instant::now() + Duration::from_secs(1)),
            Err(ReceiveDeadlineError::Disconnected)
        );
    }

    fn sample_telemetry() -> Value {
        json!({
            "kind": "codemode.execute",
            "status": "ok",
            "logical_ops": 100,
            "physical_ops": 4,
            "batched_ops": 1,
            "internal_actions": 101,
            "cache_hits": 2,
            "cache_misses": 3,
            "store_writes": 4,
            "wall_ms": 5,
            "bytes_materialized": 6
        })
    }

    #[test]
    fn ref_and_execution_id_regexes_accept_contract_shapes() {
        assert!(valid_execution_id("cm://exec/1782920000000-012345abcdef"));
        assert!(!valid_execution_id("1782920000000-012345abcdef"));
        assert!(valid_ref(
            Ns::Gz,
            "gz://blob/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(valid_ref(
            Ns::Gz,
            "gz://codemode/execution/1782920000000-012345abcdef/result"
        ));
        assert!(!valid_ref(
            Ns::Gz,
            "codemode/execution/1782920000000-012345abcdef/result"
        ));
    }

    #[test]
    fn telemetry_validator_rejects_raw_leak_and_unknown_fields() {
        let mut good = sample_telemetry();
        assert!(validate_telemetry(&good).is_empty());
        good["raw_leak"] = json!(false);
        let errors = validate_telemetry(&good);
        assert!(
            errors.iter().any(|error| error.contains("raw_leak")),
            "{errors:?}"
        );
    }

    #[test]
    fn capability_manifest_requires_contract_ns_and_plan_forms() {
        let good = json!({
            "contract_version": "1.0",
            "ns": "fz",
            "mutation": "allowed",
            "plan_forms": ["recipe", "json", "js"],
            "limits": { "max_output_bytes": 65536 }
        });
        assert!(validate_capability_manifest(Ns::Fz, &good).is_empty());
        let bad = json!({
            "contract_version": "0.9",
            "ns": "tz",
            "mutation": "maybe",
            "plan_forms": ["json"],
            "limits": { "dead_limit": 1 }
        });
        let errors = validate_capability_manifest(Ns::Fz, &bad);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("contract_version")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|error| error.contains("plan_forms")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|error| error.contains("dead_limit")),
            "{errors:?}"
        );
    }

    #[test]
    fn execution_record_validator_checks_nested_refs() {
        let record = json!({
            "execution_id": "cm://exec/1782920000000-012345abcdef",
            "ns": "gz",
            "status": "ok",
            "refs": {
                "code": "gz://codemode/execution/1782920000000-012345abcdef/code",
                "steps": "gz://codemode/execution/1782920000000-012345abcdef/steps",
                "telemetry": "gz://codemode/execution/1782920000000-012345abcdef/telemetry",
                "result": "gz://codemode/execution/1782920000000-012345abcdef/result"
            },
            "telemetry": sample_telemetry()
        });
        assert!(validate_execution_record(Ns::Gz, &record).is_empty());
        let mut bad = record;
        bad["refs"]["result"] = json!("codemode/execution/1782920000000-012345abcdef/result");
        let errors = validate_execution_record(Ns::Gz, &bad);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("invalid result ref")),
            "{errors:?}"
        );
    }

    #[test]
    fn report_serialization_tracks_overall_pass_state() {
        let report = ConformanceReport::new(
            Ns::Tz,
            "/tmp/tokenzero",
            Surface::Planner,
            vec![
                CheckResult::pass("G1", "exposure"),
                CheckResult::fail("G2", "refs", "bad ref"),
            ],
        );
        assert!(!report.passed);
        let json = serde_json::to_value(report).unwrap();
        assert_eq!(json["contract_version"], CONTRACT_VERSION);
        assert_eq!(json["checks"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn conformance_regression_collect_refs_rejects_unrelated_urls() {
        let value = json!({
            "valid": ["fz://blob/abc", "gz://x", "tz://y", "cm://exec/1-abcdefabcdef"],
            "unrelated": ["https://example.com", "custom://value", "prefix fz://not-a-ref"]
        });
        assert_eq!(
            collect_refs(&value),
            vec![
                "fz://blob/abc",
                "gz://x",
                "tz://y",
                "cm://exec/1-abcdefabcdef"
            ]
        );
    }

    #[test]
    fn conformance_regression_ref_validation_uses_canonical_pattern() {
        let valid = format!("fz://blob/{}", "a".repeat(64));
        assert!(valid_ref(Ns::Fz, &valid));
        assert!(!valid_ref(Ns::Fz, "fz://blob/not-hex"));
        assert!(valid_execution_id("cm://exec/123-abcdefabcdef"));
    }
}
