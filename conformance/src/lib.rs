pub mod checks;
pub mod fake_substrate;
pub mod patterns;
pub mod report;
pub mod schema;

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

pub const CONTRACT_VERSION: &str = "1.0";
pub const CHECK_IDS: [&str; 10] = ["G1", "G2", "G3", "G4", "G5", "G6", "G7", "G8", "G9", "G10"];

static EXECUTION_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^cm://exec/\d+-[0-9a-f]{12}$").expect("valid execution-id regex")
});

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
        let ns = regex::escape(self.as_str());
        Regex::new(&format!(
            r"^{ns}://(blob/[0-9a-f]{{64}}|codemode/execution/[^/]+/(code|steps|telemetry|result|error))$"
        ))
        .expect("valid namespace ref regex")
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub id: String,
    pub name: String,
    pub passed: bool,
    pub details: Vec<String>,
}

impl CheckResult {
    pub fn pass(id: &str, name: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            passed: true,
            details: Vec::new(),
        }
    }

    pub fn fail(id: &str, name: &str, detail: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            passed: false,
            details: vec![detail.into()],
        }
    }

    pub fn with_details(id: &str, name: &str, details: Vec<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            passed: details.is_empty(),
            details,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConformanceReport {
    pub ns: String,
    pub bin: String,
    pub contract_version: String,
    pub passed: bool,
    pub checks: Vec<CheckResult>,
}

impl ConformanceReport {
    pub fn new(ns: Ns, bin: impl Into<String>, checks: Vec<CheckResult>) -> Self {
        let passed = checks.iter().all(|check| check.passed);
        Self {
            ns: ns.as_str().into(),
            bin: bin.into(),
            contract_version: CONTRACT_VERSION.into(),
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
        "schemas/capability-manifest.schema.json",
        "schemas/telemetry.schema.json",
        "schemas/error.schema.json",
        "schemas/execution-record.schema.json",
        "schemas/limits.schema.json",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
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

    if manifest.contract_version != CONTRACT_VERSION {
        errors.push(format!(
            "contract_version is {:?}, expected {CONTRACT_VERSION:?}",
            manifest.contract_version
        ));
    }
    if manifest.ns != ns.as_str() {
        errors.push(format!(
            "ns is {:?}, expected {:?}",
            manifest.ns,
            ns.as_str()
        ));
    }
    if !matches!(
        manifest.mutation.as_str(),
        "allowed" | "denied" | "readonly"
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
    errors.extend(validate_telemetry(
        &serde_json::to_value(&record.telemetry).expect("telemetry serializes"),
    ));
    if let Some(error) = record.error {
        errors.extend(validate_error(
            &serde_json::to_value(error).expect("error serializes"),
        ));
    }
    errors
}

pub fn valid_execution_id(value: &str) -> bool {
    EXECUTION_ID_RE.is_match(value)
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
            if value.contains("://") || value.contains("codemode/execution/") {
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

/// The one surface an installed artifact serves.
///
/// Not a mode you pass at runtime. You install EITHER the CodeMode artifact
/// OR the MCP artifact, never both, and the choice is baked into the binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Codemode,
    Mcp,
}

impl Surface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codemode => "codemode",
            Self::Mcp => "mcp",
        }
    }

    /// The surface this artifact must NOT serve.
    pub fn opposite(self) -> Self {
        match self {
            Self::Codemode => Self::Mcp,
            Self::Mcp => Self::Codemode,
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "codemode" | "code-mode" | "code_mode" => Ok(Self::Codemode),
            "mcp" => Ok(Self::Mcp),
            other => bail!("unknown surface {other:?}; expected 'codemode' or 'mcp'"),
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
    let mut checks = Vec::new();
    checks.push(match check_exposure(config) {
        Ok(check) => check,
        Err(err) => CheckResult::fail("G1", "exposure", err.to_string()),
    });

    // G2-G10 exercise CodeMode execution semantics, so they only apply to a
    // CodeMode artifact. Against an MCP artifact there is no execute_code to
    // drive, and reporting them as failures would be a category error.
    if config.surface == Surface::Mcp {
        return ConformanceReport::new(config.ns, substrate_label(config), checks);
    }

    let codemode = match McpClient::spawn(&config.bin, "codemode", config.timeout) {
        Ok(mut client) => {
            let init = client.initialize();
            if let Err(err) = init {
                checks.push(CheckResult::fail(
                    "G2",
                    "refs",
                    format!("codemode initialize failed: {err}"),
                ));
                None
            } else {
                Some(client)
            }
        }
        Err(err) => {
            checks.push(CheckResult::fail(
                "G2",
                "refs",
                format!("could not spawn codemode server: {err}"),
            ));
            None
        }
    };

    if let Some(mut client) = codemode {
        let mut more = run_live_checks(config.ns, &mut client);
        checks.append(&mut more);
    } else {
        for (id, name) in [
            ("G3", "telemetry"),
            ("G4", "leak-proof"),
            ("G5", "errors"),
            ("G6", "ctx.step"),
            ("G7", "limits"),
            ("G8", "mutation"),
            ("G9", "coalescing"),
            ("G10", "sandbox-denial"),
        ] {
            checks.push(CheckResult::fail(
                id,
                name,
                "skipped because codemode server did not initialize",
            ));
        }
    }

    // Basename only: an absolute path from the authoring machine is a local
    // layout leak, not evidence. See conformance/reports/ATTESTATION.md.
    ConformanceReport::new(config.ns, substrate_label(config), checks)
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
    let mut details = Vec::new();
    let codemode_tools: BTreeSet<String> = config.ns.tool_names().into_iter().collect();

    let mut client = McpClient::spawn(&config.bin, config.surface.as_str(), config.timeout)?;
    client.initialize()?;
    let served: BTreeSet<String> = client.list_tools()?.into_iter().collect();

    match config.surface {
        Surface::Codemode => {
            if served != codemode_tools {
                details.push(format!(
                    "codemode artifact served {served:?}, expected exactly {codemode_tools:?}"
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

    // The other surface must fail closed. A single-surface artifact that also
    // answers as its opposite is exactly the dual catalog the product rule
    // forbids, so serving it is the failure, not refusing it.
    let opposite = config.surface.opposite();
    match McpClient::spawn(&config.bin, opposite.as_str(), config.timeout) {
        Err(_) => {}
        Ok(mut wrong) => {
            if wrong.initialize().is_ok() && wrong.list_tools().is_ok() {
                details.push(format!(
                    "artifact is built for surface {} but also served {};                      surfaces must be mutually exclusive",
                    config.surface.as_str(),
                    opposite.as_str()
                ));
            }
        }
    }

    Ok(CheckResult::with_details("G1", "exposure", details))
}

fn run_live_checks(ns: Ns, client: &mut McpClient) -> Vec<CheckResult> {
    let mut checks = Vec::new();
    let describe_tool = format!("{}_codemode_describe", ns.as_str());
    let execute_tool = format!("{}_execute_code", ns.as_str());

    let capabilities = client.call_tool(&describe_tool, json!({ "name": "capabilities" }));
    let manifest = capabilities.as_ref().ok().and_then(extract_json_payload);
    let mut g7_limits = BTreeMap::new();
    match manifest.as_ref() {
        Some(value) => {
            let details = validate_capability_manifest(ns, value);
            if let Some(limits) = value.get("limits").and_then(Value::as_object) {
                for (name, value) in limits {
                    if let Some(value) = value.as_u64() {
                        g7_limits.insert(name.clone(), value);
                    }
                }
            }
            if !details.is_empty() {
                checks.push(CheckResult::with_details("G7", "limits", details));
            }
        }
        None => checks.push(CheckResult::fail(
            "G7",
            "limits",
            "could not read capabilities manifest",
        )),
    }

    let basic = client.call_tool(
        &execute_tool,
        json!({ "plan": "return { ok: true };", "form": "js" }),
    );
    let basic_value = basic.as_ref().ok().and_then(extract_json_payload);
    checks.push(check_refs(ns, basic_value.as_ref()));
    checks.push(check_telemetry(basic_value.as_ref()));
    checks.push(check_ctx_step(ns, client, &execute_tool));
    checks.push(check_leak_proof(ns, client, &execute_tool));
    checks.push(check_errors(ns, client, &execute_tool));
    if !checks.iter().any(|check| check.id == "G7") {
        checks.push(check_limits(ns, client, &execute_tool, &g7_limits));
    }
    checks.push(check_mutation(ns, client, &execute_tool, manifest.as_ref()));
    checks.push(check_coalescing(client, &execute_tool));
    checks.push(check_sandbox_denial(client, &execute_tool));
    checks
}

fn check_refs(ns: Ns, payload: Option<&Value>) -> CheckResult {
    let Some(payload) = payload else {
        return CheckResult::fail("G2", "refs", "execute_code did not return JSON payload");
    };
    let mut details = Vec::new();
    if let Some(execution_id) = payload.get("execution_id").and_then(Value::as_str) {
        if !valid_execution_id(execution_id) {
            details.push(format!("invalid execution_id {execution_id:?}"));
        }
    } else {
        details.push("missing execution_id".into());
    }
    for value in collect_refs(payload) {
        if !valid_ref(ns, &value) && !valid_execution_id(&value) {
            details.push(format!("invalid CodeMode ref {value:?}"));
        }
    }
    CheckResult::with_details("G2", "refs", details)
}

fn check_telemetry(payload: Option<&Value>) -> CheckResult {
    let Some(payload) = payload else {
        return CheckResult::fail(
            "G3",
            "telemetry",
            "execute_code did not return JSON payload",
        );
    };
    let Some(telemetry) = payload.get("telemetry") else {
        return CheckResult::fail("G3", "telemetry", "missing telemetry object");
    };
    CheckResult::with_details("G3", "telemetry", validate_telemetry(telemetry))
}

fn check_leak_proof(ns: Ns, client: &mut McpClient, execute_tool: &str) -> CheckResult {
    let plan = "return 'x'.repeat(70000);";
    match client.call_tool(execute_tool, json!({ "plan": plan, "form": "js" })) {
        Ok(response) => {
            let visible = response.to_string().len();
            let payload = extract_json_payload(&response).unwrap_or(response);
            let refs = collect_refs(&payload);
            let mut details = Vec::new();
            if visible > 65_536 {
                details.push(format!(
                    "visible response is {visible} bytes, exceeds 64 KiB guard"
                ));
            }
            if !refs.iter().any(|value| valid_ref(ns, value)) {
                details.push("oversize result did not return a valid result/blob ref".into());
            }
            CheckResult::with_details("G4", "leak-proof", details)
        }
        Err(err) => CheckResult::fail("G4", "leak-proof", err.to_string()),
    }
}

fn check_errors(_ns: Ns, client: &mut McpClient, execute_tool: &str) -> CheckResult {
    let cases = [
        (
            "validation",
            json!({ "plan": "{ definitely invalid json", "form": "json" }),
        ),
        (
            "sandbox",
            json!({ "plan": "return fetch('https://example.com');", "form": "js" }),
        ),
        (
            "runtime",
            json!({ "plan": "throw new Error('boom');", "form": "js" }),
        ),
        (
            "substrate",
            json!({ "plan": "return zero.read('__zerostack_missing_target__');", "form": "js" }),
        ),
        (
            "policy",
            json!({ "plan": "return zero.edit('x', 'y');", "form": "js" }),
        ),
    ];
    let mut details = Vec::new();
    for (kind, args) in cases {
        match client.call_tool(execute_tool, args) {
            Ok(response) => {
                let payload = extract_json_payload(&response).unwrap_or(response);
                let error = payload
                    .get("error")
                    .or_else(|| payload.get("content").and_then(|v| v.get("error")));
                match error {
                    Some(error) => {
                        let error_details = validate_error(error);
                        if !error_details.is_empty() {
                            details.push(format!("{kind} case invalid error: {error_details:?}"));
                        }
                        if error.get("kind").and_then(Value::as_str) != Some(kind) {
                            details.push(format!("{kind} case returned wrong kind: {error}"));
                        }
                    }
                    None => details.push(format!(
                        "{kind} case did not return structured error: {payload}"
                    )),
                }
            }
            Err(err) => details.push(format!("{kind} case MCP call failed: {err}")),
        }
    }
    CheckResult::with_details("G5", "errors", details)
}

fn check_ctx_step(ns: Ns, client: &mut McpClient, execute_tool: &str) -> CheckResult {
    let plan = "return ctx.step('x', () => ({value: 42}));";
    match client.call_tool(execute_tool, json!({ "plan": plan, "form": "js" })) {
        Ok(response) => {
            let payload = extract_json_payload(&response).unwrap_or(response);
            let mut details = check_refs(ns, Some(&payload)).details;
            let refs = collect_refs(&payload);
            if !refs.iter().any(|value| value.ends_with("/steps")) {
                details.push("no steps ref returned for ctx.step execution".into());
            }
            CheckResult::with_details("G6", "ctx.step", details)
        }
        Err(err) => CheckResult::fail("G6", "ctx.step", err.to_string()),
    }
}

fn check_limits(
    _ns: Ns,
    client: &mut McpClient,
    execute_tool: &str,
    limits: &BTreeMap<String, u64>,
) -> CheckResult {
    let mut details = Vec::new();
    for name in limits.keys() {
        let plan = match name.as_str() {
            "max_code_bytes" => Some("x".repeat(limits[name] as usize + 1)),
            "max_microtasks" => Some("let p = Promise.resolve(); for (let i=0;i<5000;i++) p = p.then(() => 1); return p;".to_string()),
            "max_output_bytes" => Some("return 'x'.repeat(70000);".to_string()),
            "max_logical_ops" => Some("for (let i=0;i<1005;i++) { ctx.ref(i); } return 1;".to_string()),
            "max_parallel_width" => Some("return zero.queryMany ? zero.queryMany(Array.from({length: 17}, (_, i) => String(i))) : 1;".to_string()),
            "max_wall_ms" | "hard_max_wall_ms" | "max_memory_bytes" | "max_physical_ops" | "max_result_ref_bytes" | "max_refs_emitted" => None,
            _ => None,
        };
        if let Some(plan) = plan {
            match client.call_tool(execute_tool, json!({ "plan": plan, "form": "js" })) {
                Ok(response) => {
                    let payload = extract_json_payload(&response).unwrap_or(response);
                    let enforced = payload.get("ack").and_then(Value::as_str) == Some("X0")
                        || payload.get("error").is_some()
                        || (name == "max_output_bytes"
                            && payload.to_string().len() <= limits[name] as usize);
                    if !enforced {
                        details.push(format!("echoed limit {name} was not observably enforced"));
                    }
                }
                Err(_err) => {}
            }
        } else {
            details.push(format!("echoed limit {name} has no generic violation probe; substrate must add one or omit the limit"));
        }
    }
    CheckResult::with_details("G7", "limits", details)
}

fn check_mutation(
    ns: Ns,
    client: &mut McpClient,
    execute_tool: &str,
    manifest: Option<&Value>,
) -> CheckResult {
    let declared = manifest
        .and_then(|value| value.get("mutation"))
        .and_then(Value::as_str)
        .unwrap_or(match ns {
            Ns::Fz => "allowed",
            Ns::Tz => "denied",
            Ns::Gz => "readonly",
        });
    let mut details = Vec::new();
    if (ns == Ns::Fz && declared != "allowed")
        || (ns == Ns::Tz && declared != "denied")
        || (ns == Ns::Gz && declared != "readonly")
    {
        details.push(format!(
            "declared mutation {declared:?} does not match required namespace default"
        ));
    }
    match client.call_tool(
        execute_tool,
        json!({ "plan": "return zero.edit('x', 'y');", "form": "js" }),
    ) {
        Ok(response) => {
            let payload = extract_json_payload(&response).unwrap_or(response);
            let ack = payload.get("ack").and_then(Value::as_str);
            let error_kind = payload.pointer("/error/kind").and_then(Value::as_str);
            match declared {
                "allowed" => {
                    if ack == Some("X0") && error_kind == Some("policy") {
                        details.push(
                            "allowed mutation capability rejected mutation with policy".into(),
                        );
                    }
                }
                "denied" | "readonly" => {
                    if error_kind != Some("policy") {
                        details.push(format!(
                            "{declared} mutation capability did not reject with policy: {payload}"
                        ));
                    }
                }
                _ => details.push(format!("unknown mutation capability {declared:?}")),
            }
        }
        Err(err) => details.push(format!("mutation probe failed at MCP layer: {err}")),
    }
    CheckResult::with_details("G8", "mutation", details)
}

fn check_coalescing(client: &mut McpClient, execute_tool: &str) -> CheckResult {
    let plan = "return zero.queryMany ? zero.queryMany(Array.from({length: 100}, (_, i) => String(i))) : ctx.step('batch', () => Array.from({length: 100}, (_, i) => ctx.ref(i)));";
    match client.call_tool(execute_tool, json!({ "plan": plan, "form": "js" })) {
        Ok(response) => {
            let payload = extract_json_payload(&response).unwrap_or(response);
            let telemetry = payload.get("telemetry");
            let mut details = Vec::new();
            match telemetry {
                Some(telemetry) => {
                    if telemetry
                        .get("physical_ops")
                        .and_then(Value::as_u64)
                        .unwrap_or(100)
                        >= 100
                    {
                        details.push(format!("physical_ops not coalesced below 100: {telemetry}"));
                    }
                    if telemetry
                        .get("batched_ops")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        < 1
                    {
                        details.push(format!("batched_ops < 1: {telemetry}"));
                    }
                }
                None => details.push("missing telemetry for coalescing probe".into()),
            }
            CheckResult::with_details("G9", "coalescing", details)
        }
        Err(err) => CheckResult::fail("G9", "coalescing", err.to_string()),
    }
}

fn check_sandbox_denial(client: &mut McpClient, execute_tool: &str) -> CheckResult {
    let cases = [
        ("network/fetch", "return fetch('https://example.com');"),
        ("env", "return process.env.HOME;"),
        (
            "process/spawn",
            "return require('child_process').spawn('true');",
        ),
        (
            "raw host FS",
            "return require('fs').readFileSync('/etc/passwd', 'utf8');",
        ),
        (
            "direct DB/store",
            "return globalThis.db || globalThis.store || sqlite;",
        ),
        ("native modules", "return require('node:fs');"),
        ("timers", "return setTimeout(() => 1, 1);"),
    ];
    let mut details = Vec::new();
    for (name, plan) in cases {
        match client.call_tool(execute_tool, json!({ "plan": plan, "form": "js" })) {
            Ok(response) => {
                let payload = extract_json_payload(&response).unwrap_or(response);
                if payload.pointer("/error/kind").and_then(Value::as_str) != Some("sandbox") {
                    details.push(format!(
                        "{name} was not denied with sandbox error: {payload}"
                    ));
                }
            }
            Err(err) => details.push(format!("{name} probe failed at MCP layer: {err}")),
        }
    }
    CheckResult::with_details("G10", "sandbox-denial", details)
}

/// Pull the substrate's JSON payload out of an MCP tool response.
///
/// MCP lets a server return its structured payload in several places, and the
/// engines genuinely use different ones. This missed every GraphZero error
/// envelope until `structuredContent` was handled: GraphZero puts the payload
/// in a `content[]` entry of type `structuredContent` and repeats it at the
/// top level, while `content[0].text` holds a human-readable ack line that is
/// deliberately not JSON. The harness parsed only `json` and `text`, so it
/// reported "did not return structured error" for responses that were
/// correctly structured all along. That is a harness bug, not an engine bug,
/// and it was scoring conformance failures against compliant engines.
fn extract_json_payload(response: &Value) -> Option<Value> {
    // A tool result may carry the payload top-level under structuredContent.
    if let Some(structured) = response.get("structuredContent") {
        if structured.is_object() {
            return Some(structured.clone());
        }
    }
    if let Some(structured) = response
        .get("result")
        .and_then(|r| r.get("structuredContent"))
    {
        if structured.is_object() {
            return Some(structured.clone());
        }
    }
    if response.get("ack").is_some()
        || response.get("contract_version").is_some()
        || response.get("telemetry").is_some()
    {
        return Some(response.clone());
    }
    if let Some(result) = response.get("result") {
        if let Some(payload) = result
            .get("content")
            .and_then(Value::as_array)
            .and_then(|c| payload_from_content(c))
        {
            return Some(payload);
        }
        if result.is_object() {
            return Some(result.clone());
        }
    }
    if let Some(payload) = response
        .get("content")
        .and_then(Value::as_array)
        .and_then(|c| payload_from_content(c))
    {
        return Some(payload);
    }
    None
}

/// Scan one `content[]` array for a structured payload, preferring explicitly
/// structured entries over text that merely happens to parse as JSON.
fn payload_from_content(content: &[Value]) -> Option<Value> {
    for item in content {
        if let Some(structured) = item.get("structuredContent") {
            if structured.is_object() {
                return Some(structured.clone());
            }
        }
        if let Some(json) = item.get("json") {
            return Some(json.clone());
        }
    }
    for item in content {
        if let Some(text) = item.get("text").and_then(Value::as_str) {
            if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                if parsed.is_object() || parsed.is_array() {
                    return Some(parsed);
                }
            }
        }
    }
    None
}

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
    timeout: Duration,
}

impl McpClient {
    fn spawn(bin: &Path, mode: &str, timeout: Duration) -> Result<Self> {
        let mut child = Command::new(bin)
            .arg(format!("--mode={mode}"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawning {} --mode={mode}", bin.display()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("missing child stdin"))?;
        let stdout = BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| anyhow!("missing child stdout"))?,
        );
        Ok(Self {
            child,
            stdin,
            stdout,
            next_id: 1,
            timeout,
        })
    }

    fn initialize(&mut self) -> Result<Value> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "zerostack-codemode-conformance", "version": env!("CARGO_PKG_VERSION") }
            }),
        )
    }

    fn list_tools(&mut self) -> Result<Vec<String>> {
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

    fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
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
        let start = Instant::now();
        loop {
            if start.elapsed() > self.timeout {
                bail!("timeout waiting for {method} response");
            }
            let mut line = String::new();
            let bytes = self.stdout.read_line(&mut line)?;
            if bytes == 0 {
                bail!("server exited while waiting for {method} response");
            }
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
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
