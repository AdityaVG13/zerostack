#![forbid(unsafe_code)]

pub mod checks;
pub mod fake_substrate;
pub mod oracle;
pub mod patterns;
pub mod racc;
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
use std::time::{Duration, Instant};

pub const CONTRACT_VERSION: &str = "1.0";
/// Stable compatibility projection of the authoritative GATE_MAPPINGS table.
pub const CHECK_IDS: [&str; 10] = ["G1", "G2", "G3", "G4", "G5", "G6", "G7", "G8", "G9", "G10"];

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
        Surface::Codemode => checks::GATE_MAPPINGS
            .iter()
            .map(|mapping| mapping.id.as_str())
            .collect(),
        Surface::Mcp => vec!["G1"],
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
        let full_scope = checks::GATE_MAPPINGS.iter().all(|mapping| {
            checks
                .iter()
                .any(|check| check.id == mapping.id.as_str() && check.status != GateStatus::Skipped)
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
            contract_version: CONTRACT_VERSION.into(),
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
        "schemas/capability-manifest.schema.json",
        "schemas/telemetry.schema.json",
        "schemas/error.schema.json",
        "schemas/execution-record.schema.json",
        "schemas/limits.schema.json",
        "schemas/raw-worker-v2.schema.json",
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
                "not applicable to MCP surface; requires CodeMode execution",
            ));
        }
        return ConformanceReport::new(config.ns, substrate_label(config), config.surface, checks);
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
            checks.push(CheckResult::skip(
                id,
                name,
                "CodeMode server did not initialize",
            ));
        }
    }

    // Basename only: an absolute path from the authoring machine is a local
    // layout leak, not evidence. See conformance/reports/ATTESTATION.md.
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

    let mut client = McpClient::spawn(&config.bin, config.surface.as_str(), config.timeout)?;
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
        Surface::Codemode => {
            if &served != codemode_tools {
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
    match McpClient::spawn(bin, opposite.as_str(), timeout) {
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

fn run_live_checks(ns: Ns, client: &mut McpClient) -> Vec<CheckResult> {
    let mut checks = Vec::new();
    let describe_tool = format!("{}_codemode_describe", ns.as_str());
    let execute_tool = format!("{}_execute_code", ns.as_str());

    let capabilities = client.call_tool(&describe_tool, json!({ "name": "capabilities" }));
    let manifest = match capabilities {
        Ok(response) => match extract_json_payload(&response) {
            Some(manifest) => Some(manifest),
            None => {
                checks.push(CheckResult::fail(
                    "G7",
                    "limits",
                    "capabilities probe returned no JSON payload",
                ));
                None
            }
        },
        Err(err) => {
            checks.push(CheckResult::fail(
                "G7",
                "limits",
                format!("capabilities probe failed at MCP layer: {err}"),
            ));
            None
        }
    };
    let mut g7_limits = BTreeMap::new();
    if let Some(value) = manifest.as_ref() {
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

fn limit_probe_plan(name: &str, limit: u64) -> Option<String> {
    let above = limit.checked_add(1)?;
    match name {
        "max_code_bytes" => usize::try_from(above).ok().map(|size| "x".repeat(size)),
        "max_microtasks" => Some(format!(
            "let p = Promise.resolve(); for (let i=0;i<{above};i++) p = p.then(() => 1); return p;"
        )),
        "max_output_bytes" => Some(format!("return 'x'.repeat({above});")),
        "max_logical_ops" => Some(format!(
            "for (let i=0;i<{above};i++) {{ ctx.ref(i); }} return 1;"
        )),
        "max_parallel_width" => Some(format!(
            "return zero.queryMany ? zero.queryMany(Array.from({{length: {above}}}, (_, i) => String(i))) : 1;"
        )),
        "max_wall_ms" | "hard_max_wall_ms" | "max_memory_bytes" | "max_physical_ops"
        | "max_result_ref_bytes" | "max_refs_emitted" => None,
        _ => None,
    }
}

fn check_limits(
    _ns: Ns,
    client: &mut McpClient,
    execute_tool: &str,
    limits: &BTreeMap<String, u64>,
) -> CheckResult {
    let mut details = Vec::new();
    for (name, limit) in limits {
        if let Some(plan) = limit_probe_plan(name, *limit) {
            match client.call_tool(execute_tool, json!({ "plan": plan, "form": "js" })) {
                Ok(response) => {
                    let payload = extract_json_payload(&response).unwrap_or(response);
                    let enforced = payload.get("ack").and_then(Value::as_str) == Some("X0")
                        || payload.get("error").is_some()
                        || (name == "max_output_bytes"
                            && payload.to_string().len() <= *limit as usize);
                    if !enforced {
                        details.push(format!("echoed limit {name} was not observably enforced"));
                    }
                }
                Err(err) => details.push(format!(
                    "echoed limit {name} probe failed at MCP layer: {err}"
                )),
            }
        } else {
            details.push(format!("echoed limit {name} has no generic violation probe; substrate must add one or omit the limit"));
        }
    }
    CheckResult::with_details("G7", "limits", details)
}

/// Namespace default for G8 mutation capability (lookup table, not match arms in check).
fn expected_mutation(ns: Ns) -> &'static str {
    match ns {
        Ns::Fz => "allowed",
        Ns::Tz => "denied",
        Ns::Gz => "readonly",
    }
}

/// Pure interpret of a mutation probe response: `(declared × ack × error_kind)`.
///
/// Extracted so a unit matrix can pin accept/reject cells without an MCP client.
fn interpret_mutation_probe(
    declared: &str,
    ack: Option<&str>,
    error_kind: Option<&str>,
    payload: &Value,
) -> Vec<String> {
    let mut details = Vec::new();
    match declared {
        "allowed" => {
            if ack == Some("X0") && error_kind == Some("policy") {
                details.push("allowed mutation capability rejected mutation with policy".into());
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
    details
}

fn check_mutation(
    ns: Ns,
    client: &mut McpClient,
    execute_tool: &str,
    manifest: Option<&Value>,
) -> CheckResult {
    let expected = expected_mutation(ns);
    let declared = manifest
        .and_then(|value| value.get("mutation"))
        .and_then(Value::as_str)
        .unwrap_or(expected);
    let mut details = Vec::new();
    if declared != expected {
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
            details.extend(interpret_mutation_probe(
                declared, ack, error_kind, &payload,
            ));
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
///
/// # Payload priority (load-bearing — do not reorder)
///
/// 1. top-level `structuredContent` (object)
/// 2. `result.structuredContent` (object)
/// 3. whole-body markers: `ack` | `contract_version` | `telemetry`
/// 4. `result.content[]` via [`payload_from_content`] (structured/json before text)
/// 5. bare `result` object
/// 6. top-level `content[]` via [`payload_from_content`]
///
/// GraphZero class: structuredContent must win over human-readable text ack.
fn extract_json_payload(response: &Value) -> Option<Value> {
    for extractor in JSON_PAYLOAD_EXTRACTORS {
        if let Some(payload) = extractor(response) {
            return Some(payload);
        }
    }
    None
}

/// Ordered MCP envelope extractors. Index order IS priority (see module docs above).
const JSON_PAYLOAD_EXTRACTORS: &[fn(&Value) -> Option<Value>] = &[
    payload_top_level_structured,
    payload_result_structured,
    payload_whole_body_markers,
    payload_result_content,
    payload_bare_result,
    payload_top_level_content,
];

fn payload_top_level_structured(response: &Value) -> Option<Value> {
    response
        .get("structuredContent")
        .filter(|structured| structured.is_object())
        .cloned()
}

fn payload_result_structured(response: &Value) -> Option<Value> {
    response
        .get("result")
        .and_then(|r| r.get("structuredContent"))
        .filter(|structured| structured.is_object())
        .cloned()
}

fn payload_whole_body_markers(response: &Value) -> Option<Value> {
    if response.get("ack").is_some()
        || response.get("contract_version").is_some()
        || response.get("telemetry").is_some()
    {
        Some(response.clone())
    } else {
        None
    }
}

fn payload_result_content(response: &Value) -> Option<Value> {
    response
        .get("result")
        .and_then(|result| result.get("content"))
        .and_then(Value::as_array)
        .and_then(|c| payload_from_content(c))
}

fn payload_bare_result(response: &Value) -> Option<Value> {
    response
        .get("result")
        .filter(|result| result.is_object())
        .cloned()
}

fn payload_top_level_content(response: &Value) -> Option<Value> {
    response
        .get("content")
        .and_then(Value::as_array)
        .and_then(|c| payload_from_content(c))
}

/// Scan one `content[]` array for a structured payload, preferring explicitly
/// structured entries over text that merely happens to parse as JSON.
fn explicit_content_payload(item: &Value) -> Option<Value> {
    match item.get("structuredContent") {
        Some(structured) if structured.is_object() => Some(structured.clone()),
        _ => item.get("json").cloned(),
    }
}

/// Scan one content array for a structured payload, preferring explicitly
/// structured entries over text that merely happens to parse as JSON.
fn payload_from_content(content: &[Value]) -> Option<Value> {
    for item in content {
        if let Some(payload) = explicit_content_payload(item) {
            return Some(payload);
        }
    }
    for item in content {
        if let Some(text) = item.get("text").and_then(Value::as_str) {
            if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                if matches!(parsed, Value::Object(_) | Value::Array(_)) {
                    return Some(parsed);
                }
            }
        }
    }
    None
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

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout_lines: std::sync::mpsc::Receiver<std::io::Result<String>>,
    stdout_reader: Option<std::thread::JoinHandle<()>>,
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
            Surface::Codemode,
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

    #[test]
    fn conformance_regression_limit_probes_use_echoed_boundary() {
        assert!(limit_probe_plan("max_parallel_width", 3)
            .is_some_and(|plan| plan.contains("length: 4")));
        assert!(limit_probe_plan("max_logical_ops", u64::MAX).is_none());
        assert!(limit_probe_plan("max_wall_ms", 10).is_none());
    }
}
