//! Canonical FSZero operation ABI (fszero-ncib.1).
//!
//! One versioned registry is the source of truth for public operation identity:
//! names, surface aliases, mutability, capability requirements, cost class,
//! ref ownership, allowed error classes, and cancellation/deadline semantics.
//!
//! Surface catalogs (MCP tool names, CodeMode `METHODS` paths, CLI opcodes) are
//! validated against this registry. Transport-only framing may differ; domain
//! results must not. The filesystem semantic contract document remains the
//! normative guarantee text; this module is the typed ABI over that contract.

use super::filesystem_contract::{
    FILESYSTEM_CONTRACT_VERSION, filesystem_contract_descriptor,
    filesystem_contract_operation_names, validate_filesystem_contract_document,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// ABI protocol name peers/hubs negotiate on.
pub const OPERATION_ABI_NAME: &str = "fszero-operation-abi";
/// Semver of the typed operation ABI itself (independent of filesystem-contract patch).
/// 1.1.0: full input/output schema ownership via operation-abi-schemas-v1.json.
pub const OPERATION_ABI_VERSION: &str = "1.2.0";
/// Recovery-store key for the published ABI descriptor.
pub const OPERATION_ABI_STORE_KEY: &str = "operation_abi";
/// Digest algorithm advertised with the ABI descriptor.
pub const OPERATION_ABI_DIGEST_ALGORITHM: &str = "sha256";

/// Wire-string enum: `as_str` + `parse` from a single variant→literal table.
macro_rules! str_enum {
    (
        $(#[$enum_meta:meta])*
        $vis:vis enum $name:ident { $( $(#[$vmeta:meta])* $variant:ident => $lit:literal ),+ $(,)? }
    ) => {
        $(#[$enum_meta])*
        $vis enum $name { $( $(#[$vmeta])* $variant, )+ }

        impl $name {
            pub const fn as_str(self) -> &'static str { match self { $(Self::$variant => $lit,)+ } }

            pub fn parse(raw: &str) -> Option<Self> {
                match raw { $($lit => Some(Self::$variant),)+ _ => None, } } } }; }

str_enum! {
/// Whether an operation mutates durable workspace or memory state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mutability { Read => "read", Write => "write", Mixed => "mixed" } }

str_enum! {
/// Relative expected cost class for scheduling and telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CostClass { Cheap => "cheap", Moderate => "moderate", Expensive => "expensive" } }

str_enum! {
/// How an operation relates to recovery refs (`fz://blob/...` and named keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefOwnership { None => "none", Minted => "minted", Consumes => "consumes", Both => "both" } }

str_enum! {
    /// Capability token required before dispatch (authorization / root policy).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum CapabilityRequirement { FilesystemRead => "filesystem.read", FilesystemWrite => "filesystem.write", FilesystemSearch => "filesystem.search", Memory => "memory", World => "world", History => "history", Expand => "expand", Doctor => "doctor", Migrate => "migrate", SurfaceDispatch => "surface.dispatch" }
}

str_enum! {
    /// Cancellation / deadline observation contract for one operation.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum CancellationSemantics { NotCancellable => "not_cancellable", BeforePublish => "before_publish", ReadOnlyStop => "read_only_stop" }
}

/// Canonical operation identity in the registry.
///
/// Complete input/output schema *structure* lives in
/// `contracts/operation-abi-schemas-v1.json` under `domain_operations[id]`
/// (and surface bindings under `mcp_tools` / `codemode_methods`). This struct
/// carries identity, policy, and alias ownership; schema documents are
/// deterministically digested with the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Operation {
    pub id: &'static str,
    pub mutability: Mutability,
    pub cost_class: CostClass,
    pub ref_ownership: RefOwnership,
    pub capability: CapabilityRequirement,
    pub cancellation: CancellationSemantics,
    // input_keys / required_input_keys derived from domain schema at validate/digest time.
    /// MCP tool names that alias this op (excluding surface-only dispatch tools).
    pub mcp_aliases: &'static [&'static str],
    /// CodeMode method paths that alias this op.
    pub codemode_aliases: &'static [&'static str],
    /// CLI single-char opcodes.
    pub cli_opcodes: &'static [char],
}

impl Operation {
    /// Compact registry constructor (const table rows).
    pub const fn new(
        id: &'static str,
        mutability: Mutability,
        cost_class: CostClass,
        ref_ownership: RefOwnership,
        capability: CapabilityRequirement,
        cancellation: CancellationSemantics,
        mcp_aliases: &'static [&'static str],
        codemode_aliases: &'static [&'static str],
        cli_opcodes: &'static [char],
    ) -> Self {
        Self {
            id,
            mutability,
            cost_class,
            ref_ownership,
            capability,
            cancellation,
            mcp_aliases,
            codemode_aliases,
            cli_opcodes,
        }
    }

    /// Domain input JSON Schema from the canonical schema document.
    pub fn domain_input_schema(&self) -> Option<&'static Value> {
        super::operation_schemas::domain_operation_schemas(self.id).and_then(|s| s.get("input"))
    }

    /// Domain output JSON Schema from the canonical schema document.
    pub fn domain_output_schema(&self) -> Option<&'static Value> {
        super::operation_schemas::domain_operation_schemas(self.id).and_then(|s| s.get("output"))
    }

    /// Property names on the domain input schema (stable sorted for digests).
    pub fn input_keys_derived(&self) -> Vec<&'static str> {
        self.domain_input_schema()
            .and_then(|d| d.get("properties"))
            .and_then(Value::as_object)
            .map(|props| sorted_static_strs(props.keys().map(|k| k.as_str())))
            .unwrap_or_default()
    }

    /// Required property names on the domain input schema (stable sorted).
    pub fn required_input_keys_derived(&self) -> Vec<&'static str> {
        self.domain_input_schema()
            .and_then(|d| d.get("required"))
            .and_then(Value::as_array)
            .map(|a| sorted_static_strs(a.iter().filter_map(Value::as_str)))
            .unwrap_or_default()
    }
}

fn sorted_static_strs<'a>(iter: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut keys: Vec<&str> = iter.collect();
    keys.sort_unstable();
    keys
}

/// Structured arguments for a canonical operation (typed envelope over JSON).
#[derive(Debug, Clone, PartialEq)]
pub struct OperationArgs {
    pub operation: String,
    pub fields: Map<String, Value>,
}

impl OperationArgs {
    pub fn new(operation: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
            fields: Map::new(),
        }
    }

    pub fn with_field(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }

    pub fn from_value(operation: impl Into<String>, value: Value) -> Result<Self, DomainError> {
        let fields = match value {
            Value::Object(map) => map,
            Value::Null => Map::new(),
            other => {
                return Err(DomainError::invalid_argument(format!(
                    "operation args must be object, got {other}"
                )));
            }
        };
        Ok(Self {
            operation: operation.into(),
            fields,
        })
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.fields.get(key).and_then(Value::as_str)
    }

    pub fn to_value(&self) -> Value {
        Value::Object(self.fields.clone())
    }
}

/// Typed domain error shared across FastMCP, CodeMode, and the private raw worker.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DomainError {
    pub class: String,
    pub message: String,
    pub retryable: bool,
}

macro_rules! domain_error_ctors {
    ($($name:ident => $class:literal),+ $(,)?) => {
        $(
            #[inline] pub fn $name(message: impl Into<String>) -> Self { Self::typed($class, message) }
        )+ }; }

impl DomainError {
    pub fn typed(class: impl Into<String>, message: impl Into<String>) -> Self {
        let class = class.into();
        let retryable = error_class_retryable(&class);
        Self {
            class,
            message: message.into(),
            retryable,
        }
    }

    domain_error_ctors! {
        invalid_argument => "invalid_argument",
        invalid_path => "invalid_path",
        outside_root => "outside_root",
        not_found => "not_found",
        already_exists => "already_exists",
        not_file => "not_file",
        not_directory => "not_directory",
        unsupported_file_type => "unsupported_file_type",
        permission_denied => "permission_denied",
        stale_preimage => "stale_preimage",
        conflict => "conflict",
        budget_exceeded => "budget_exceeded",
        cancelled => "cancelled",
        deadline_exceeded => "deadline_exceeded",
        durability_unavailable => "durability_unavailable",
        store_unavailable => "store_unavailable",
        corrupt_state => "corrupt_state",
        incompatible_contract => "incompatible_contract",
        io_error => "io_error",
        internal => "internal",
    }

    pub fn to_json(&self) -> Value {
        json!({"class": self.class, "message": self.message, "retryable": self.retryable})
    }

    /// Map a wire/detail string into a domain error using the contract taxonomy.
    pub fn from_detail(detail: &str) -> Self {
        Self::typed(classify_detail_to_error_class(detail), detail)
    }
}

impl std::fmt::Display for DomainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.class, self.message)
    }
}

impl std::error::Error for DomainError {}

/// Normalized domain result: value, typed error, refs, mutation flag.
///
/// Surfaces may wrap this in transport-specific JSON-RPC; parity compares this
/// shape (and recoverable bytes behind refs), not envelope fields.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DomainResult {
    pub operation: String,
    pub ok: bool,
    /// One-token surface ack when applicable (`R1`, `E0`, `C`, …).
    pub ack: Option<String>,
    pub value: Option<Value>,
    /// Recoverable refs / recovery keys minted or referenced by the op.
    pub refs: Vec<String>,
    /// True when the operation published a durable mutation.
    pub mutated: bool,
    pub error: Option<DomainError>,
}

impl DomainResult {
    pub fn success(
        operation: impl Into<String>,
        ack: Option<String>,
        value: Option<Value>,
        refs: Vec<String>,
        mutated: bool,
    ) -> Self {
        Self {
            operation: operation.into(),
            ok: true,
            ack,
            value,
            refs,
            mutated,
            error: None,
        }
    }

    pub fn failure(operation: impl Into<String>, error: DomainError) -> Self {
        Self {
            operation: operation.into(),
            ok: false,
            ack: None,
            value: None,
            refs: Vec::new(),
            mutated: false,
            error: Some(error),
        }
    }

    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("operation".into(), json!(self.operation));
        map.insert("ok".into(), json!(self.ok));
        map.insert("mutated".into(), json!(self.mutated));
        if let Some(ack) = &self.ack {
            map.insert("ack".into(), json!(ack));
        }
        if let Some(value) = &self.value {
            map.insert("value".into(), value.clone());
        }
        if !self.refs.is_empty() {
            map.insert("refs".into(), json!(self.refs));
        }
        if let Some(err) = &self.error {
            map.insert("error".into(), err.to_json());
        }
        Value::Object(map)
    }
}

/// Full public operation registry. Every entry must appear exactly once as a
/// key under `contracts/filesystem-v1.json` → `operations` (except pure
/// surface-dispatch aliases, which map to `surface_dispatch`).
// Fields: id, mut, cost, ref, cap, cancel, mcp, codemode, cli
pub const OPERATION_REGISTRY: &[Operation] = &[
    Operation::new(
        "fs.ls",
        Mutability::Read,
        CostClass::Cheap,
        RefOwnership::Minted,
        CapabilityRequirement::FilesystemRead,
        CancellationSemantics::ReadOnlyStop,
        &["fszero.ls"],
        &["fs.ls"],
        &['L'],
    ),
    Operation::new(
        "fs.read",
        Mutability::Read,
        CostClass::Moderate,
        RefOwnership::Minted,
        CapabilityRequirement::FilesystemRead,
        CancellationSemantics::ReadOnlyStop,
        &["fszero.read"],
        &["fs.read"],
        &['R'],
    ),
    Operation::new(
        "fs.search",
        Mutability::Read,
        CostClass::Expensive,
        RefOwnership::Minted,
        CapabilityRequirement::FilesystemSearch,
        CancellationSemantics::ReadOnlyStop,
        &["fszero.search"],
        &["fs.search"],
        &['S'],
    ),
    Operation::new(
        "fs.multiRead",
        Mutability::Read,
        CostClass::Moderate,
        RefOwnership::Minted,
        CapabilityRequirement::FilesystemRead,
        CancellationSemantics::ReadOnlyStop,
        &["fszero.multi_read"],
        &["fs.multiRead"],
        &[],
    ),
    Operation::new(
        "fs.multiSearch",
        Mutability::Read,
        CostClass::Expensive,
        RefOwnership::Minted,
        CapabilityRequirement::FilesystemSearch,
        CancellationSemantics::ReadOnlyStop,
        &["fszero.multi_search"],
        &["fs.multiSearch"],
        &[],
    ),
    Operation::new(
        "fs.multiList",
        Mutability::Read,
        CostClass::Moderate,
        RefOwnership::Minted,
        CapabilityRequirement::FilesystemRead,
        CancellationSemantics::ReadOnlyStop,
        &["fszero.multi_list"],
        &["fs.multiList"],
        &[],
    ),
    Operation::new(
        "fs.multiAstSearch",
        Mutability::Read,
        CostClass::Expensive,
        RefOwnership::Minted,
        CapabilityRequirement::FilesystemSearch,
        CancellationSemantics::ReadOnlyStop,
        &[],
        &["fs.multiAstSearch"],
        &[],
    ),
    Operation::new(
        "fs.edit",
        Mutability::Write,
        CostClass::Moderate,
        RefOwnership::Minted,
        CapabilityRequirement::FilesystemWrite,
        CancellationSemantics::BeforePublish,
        &["fszero.edit"],
        &["fs.edit"],
        &['E'],
    ),
    Operation::new(
        "fs.write",
        Mutability::Write,
        CostClass::Moderate,
        RefOwnership::Minted,
        CapabilityRequirement::FilesystemWrite,
        CancellationSemantics::BeforePublish,
        &["fszero.write"],
        &["fs.write"],
        &['P'],
    ),
    Operation::new(
        "fs.transact",
        Mutability::Write,
        CostClass::Expensive,
        RefOwnership::Minted,
        CapabilityRequirement::FilesystemWrite,
        CancellationSemantics::BeforePublish,
        &[],
        &["fs.transact"],
        &[],
    ),
    Operation::new(
        "fs.compound",
        Mutability::Mixed,
        CostClass::Expensive,
        RefOwnership::Both,
        CapabilityRequirement::FilesystemWrite,
        CancellationSemantics::BeforePublish,
        &[],
        &["fs.compound"],
        &['C'],
    ),
    Operation::new(
        "fs.stat",
        Mutability::Read,
        CostClass::Cheap,
        RefOwnership::Minted,
        CapabilityRequirement::FilesystemRead,
        CancellationSemantics::ReadOnlyStop,
        &["fszero.stat"],
        &["fs.stat"],
        &['T'],
    ),
    Operation::new(
        "fs.multiStat",
        Mutability::Read,
        CostClass::Cheap,
        RefOwnership::Minted,
        CapabilityRequirement::FilesystemRead,
        CancellationSemantics::ReadOnlyStop,
        &["fszero.multi_stat"],
        &["fs.multiStat"],
        &[],
    ),
    Operation::new(
        "fs.world",
        Mutability::Mixed,
        CostClass::Expensive,
        RefOwnership::Both,
        CapabilityRequirement::World,
        CancellationSemantics::BeforePublish,
        &["fszero.world", "fszero.world_query"],
        &["fs.world"],
        &['W'],
    ),
    // CLI opcode 'M' matches OpCode::Memory / operation_for_opcode.
    Operation::new(
        "fs.memory",
        Mutability::Mixed,
        CostClass::Moderate,
        RefOwnership::Minted,
        CapabilityRequirement::Memory,
        CancellationSemantics::BeforePublish,
        &[
            "fszero.memory_put",
            "fszero.memory_get",
            "fszero.memory_ls",
            "fszero.memory_delete",
            "fszero.memory_rename",
        ],
        &[
            "fs.memory.put",
            "fs.memory.get",
            "fs.memory.ls",
            "fs.memory.delete",
            "fs.memory.rename",
        ],
        &['M'],
    ),
    Operation::new(
        "fs.expand",
        Mutability::Read,
        CostClass::Cheap,
        RefOwnership::Consumes,
        CapabilityRequirement::Expand,
        CancellationSemantics::ReadOnlyStop,
        &["fszero.expand"],
        &["fs.expand"],
        &['X'],
    ),
    Operation::new(
        "fs.history",
        Mutability::Read,
        CostClass::Moderate,
        RefOwnership::Minted,
        CapabilityRequirement::History,
        CancellationSemantics::ReadOnlyStop,
        &["fszero.history"],
        &["fs.history"],
        &['H'],
    ),
    Operation::new(
        "fs.undo",
        Mutability::Write,
        CostClass::Moderate,
        RefOwnership::Minted,
        CapabilityRequirement::History,
        CancellationSemantics::BeforePublish,
        &["fszero.undo"],
        &["fs.undo"],
        &['U'],
    ),
    Operation::new(
        "fs.resolve",
        Mutability::Read,
        CostClass::Expensive,
        RefOwnership::Minted,
        CapabilityRequirement::FilesystemSearch,
        CancellationSemantics::ReadOnlyStop,
        &["fszero.resolve"],
        &["fs.resolve"],
        &['V'],
    ),
    Operation::new(
        "doctor",
        Mutability::Read,
        CostClass::Cheap,
        RefOwnership::None,
        CapabilityRequirement::Doctor,
        CancellationSemantics::NotCancellable,
        &[],
        &[],
        &[],
    ),
    Operation::new(
        "migrate-cas",
        Mutability::Mixed,
        CostClass::Expensive,
        RefOwnership::Both,
        CapabilityRequirement::Migrate,
        CancellationSemantics::BeforePublish,
        &[],
        &[],
        &[],
    ),
];

/// Surface-only aliases that intentionally do not map to a single canonical op.
pub const SURFACE_DISPATCH_ALIASES: &[(&str, &str)] = &[
    ("mcp", "fszero.exec"),
    ("embedded", "FSZeroSession.execute"),
];

/// Retryability for stable error classes (domain layer).
pub fn error_class_retryable(class: &str) -> bool {
    matches!(
        class,
        "cancelled"
            | "deadline_exceeded"
            | "store_unavailable"
            | "budget_exceeded"
            | "io_error"
            | "durability_unavailable"
    )
}

/// Best-effort map from free-form detail text to a contract error class.
pub fn classify_detail_to_error_class(detail: &str) -> &'static str {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("outside root") || lower.contains("escapes root") {
        return "outside_root";
    }
    if lower.contains("stale") && lower.contains("preimage") {
        return "stale_preimage";
    }
    if lower.contains("deadline") {
        return "deadline_exceeded";
    }
    if lower.contains("cancel") {
        return "cancelled";
    }
    if lower.contains("budget") {
        return "budget_exceeded";
    }
    if lower.contains("permission") || lower.contains("eacces") || lower.contains("eperm") {
        return "permission_denied";
    }
    if lower.contains("not a file") || lower.contains("not_file") {
        return "not_file";
    }
    if lower.contains("not a directory") || lower.contains("not_directory") {
        return "not_directory";
    }
    if lower.contains("already exists") {
        return "already_exists";
    }
    if lower.contains("conflict") || lower.contains("ambiguous match") {
        return "conflict";
    }
    if lower.contains("corrupt") {
        return "corrupt_state";
    }
    if lower.contains("incompatible_contract") || lower.contains("incompatible contract") {
        return "incompatible_contract";
    }
    if lower.contains("store") && (lower.contains("fail") || lower.contains("unavail")) {
        return "store_unavailable";
    }
    if lower.contains("not found") || lower.contains("unknown ref") {
        return "not_found";
    }
    if lower.contains("invalid path")
        || lower.contains("absolute")
        || lower.contains("parent")
        || lower.contains("bad path")
    {
        return "invalid_path";
    }
    if lower.contains("invalid") || lower.contains("malformed") {
        return "invalid_argument";
    }
    if lower.contains("unsupported") {
        return "unsupported_file_type";
    }
    if lower.contains("durability")
        || lower.contains("fsync")
        || lower.contains("fullsync")
        || lower.contains("fullfsync")
    {
        return "durability_unavailable";
    }
    if lower.contains("io") || lower.contains("errno") {
        return "io_error";
    }
    "internal"
}

pub fn operation_by_id(id: &str) -> Option<&'static Operation> {
    OPERATION_REGISTRY.iter().find(|op| op.id == id)
}

pub fn operation_ids() -> BTreeSet<&'static str> {
    OPERATION_REGISTRY.iter().map(|op| op.id).collect()
}

/// Resolve a surface alias (MCP tool, CodeMode path, or CLI opcode char) to a
/// canonical operation id. Returns `None` for pure surface dispatch.
pub fn resolve_alias(surface: &str, alias: &str) -> Option<&'static str> {
    match surface {
        "mcp" => {
            if alias == "fszero.exec" {
                return None;
            }
            OPERATION_REGISTRY
                .iter()
                .find(|op| op.mcp_aliases.contains(&alias))
                .map(|op| op.id)
        }
        "codemode" => OPERATION_REGISTRY
            .iter()
            .find(|op| op.codemode_aliases.contains(&alias))
            .map(|op| op.id),
        "cli" => {
            let ch = alias.chars().next()?;
            OPERATION_REGISTRY
                .iter()
                .find(|op| op.cli_opcodes.contains(&ch))
                .map(|op| op.id)
        }
        _ => None,
    }
}

fn registry_str_set(
    pick: impl Fn(&Operation) -> &'static [&'static str],
) -> BTreeSet<&'static str> {
    let mut set = BTreeSet::new();
    for op in OPERATION_REGISTRY {
        for alias in pick(op) {
            set.insert(*alias);
        }
    }
    set
}

/// Registry-declared MCP alias set (source of truth for catalog equality).
pub fn registry_mcp_aliases() -> BTreeSet<&'static str> {
    let mut set = registry_str_set(|op| op.mcp_aliases);
    set.insert("fszero.exec");
    set
}

/// Registry-declared CodeMode method set.
pub fn registry_codemode_aliases() -> BTreeSet<&'static str> {
    registry_str_set(|op| op.codemode_aliases)
}

/// Registry-declared CLI opcode set.
pub fn registry_cli_opcodes() -> BTreeSet<char> {
    let mut set = BTreeSet::new();
    for op in OPERATION_REGISTRY {
        set.extend(op.cli_opcodes.iter().copied());
    }
    set
}

/// CLI path label for a registry op in the MCP↔CM↔CLI↔opcode crosswalk.
fn name_map_cli_path(op: &Operation) -> Option<&'static str> {
    if op.id.ends_with("Many") {
        return Some("fszero batch");
    }
    match op.id {
        "doctor" => Some("fszero doctor"),
        "migrate-cas" => Some("fszero migrate-cas"),
        _ if !op.codemode_aliases.is_empty() || !op.cli_opcodes.is_empty() => {
            Some("fszero codemode")
        }
        _ => None,
    }
}

/// Full MCP ↔ CodeMode ↔ CLI ↔ opcode name_map (R-PAR-REC-006 / fszero-2qdw.5).
///
/// One row per surface alias expansion of a registry op. Multiple MCP tools that
/// share one CodeMode method (e.g. `fszero.world` + `fszero.world_query` →
/// `fs.world`) each get a row. Memory MCP/CM aliases are paired by index.
/// Surface-only `fszero.exec` is appended as the opcode bridge.
///
/// Fields: `id`, `mcp`, `codemode`, `cli`, `opcode` (null when absent).
pub fn operation_name_map() -> Vec<Value> {
    let mut rows = Vec::new();
    for op in OPERATION_REGISTRY {
        let mcp_n = op.mcp_aliases.len();
        let cm_n = op.codemode_aliases.len();
        let n = mcp_n.max(cm_n).max(1);
        let opcode = op
            .cli_opcodes
            .first()
            .map(|c| Value::String(c.to_string()))
            .unwrap_or(Value::Null);
        let cli = name_map_cli_path(op)
            .map(Value::from)
            .unwrap_or(Value::Null);
        for i in 0..n {
            let mcp = if i < mcp_n {
                Value::String(op.mcp_aliases[i].to_string())
            } else {
                Value::Null
            };
            let codemode = if i < cm_n {
                Value::String(op.codemode_aliases[i].to_string())
            } else if cm_n > 0 {
                // Extra MCP aliases share the primary CodeMode method.
                Value::String(op.codemode_aliases[0].to_string())
            } else {
                Value::Null
            };
            rows.push(json!({
                "id": op.id,
                "mcp": mcp,
                "codemode": codemode,
                "cli": cli.clone(),
                "opcode": opcode.clone(),
            }));
        }
    }
    // Surface-dispatch opcode bridge (not a registry domain op).
    rows.push(json!({
        "id": "fszero.exec",
        "mcp": "fszero.exec",
        "codemode": Value::Null,
        "cli": Value::Null,
        "opcode": Value::Null,
    }));
    rows
}

/// MCP tool → exact property names from the canonical surface schema.
pub fn registry_mcp_input_keys(tool: &str) -> Option<BTreeSet<&'static str>> {
    let entry = super::operation_schemas::mcp_tool_schema_entry(tool)?;
    let props = entry.get("input")?.get("properties")?.as_object()?;
    Some(props.keys().map(|k| k.as_str()).collect())
}

/// Shared per-op identity fields for digest and doctor descriptor (order fixed for digest).
fn operation_identity_json(op: &Operation, domain: Value) -> Map<String, Value> {
    let opcodes: Vec<String> = op.cli_opcodes.iter().map(char::to_string).collect();
    [
        ("mutability", json!(op.mutability.as_str())),
        ("cost_class", json!(op.cost_class.as_str())),
        ("ref_ownership", json!(op.ref_ownership.as_str())),
        ("capability", json!(op.capability.as_str())),
        ("cancellation", json!(op.cancellation.as_str())),
        ("input_keys", json!(op.input_keys_derived())),
        (
            "required_input_keys",
            json!(op.required_input_keys_derived()),
        ),
        ("mcp_aliases", json!(op.mcp_aliases)),
        ("codemode_aliases", json!(op.codemode_aliases)),
        ("cli_opcodes", json!(opcodes)),
        ("domain_schemas", domain),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v))
    .collect()
}

fn domain_schema_or_null(op_id: &str) -> Value {
    super::operation_schemas::domain_operation_schemas(op_id)
        .cloned()
        .unwrap_or(Value::Null)
}

/// Identity map for every registry op (optionally attach contract binding).
fn registry_operations_map(with_contract: bool) -> Map<String, Value> {
    let contract_ops = with_contract.then(|| {
        filesystem_contract_descriptor()
            .get("operations")
            .cloned()
            .unwrap_or(Value::Null)
    });
    let mut operations = Map::new();
    for op in OPERATION_REGISTRY {
        let mut entry = operation_identity_json(op, domain_schema_or_null(op.id));
        if let Some(ref cops) = contract_ops {
            entry.insert(
                "contract".into(),
                cops.get(op.id).cloned().unwrap_or(Value::Null),
            );
        }
        operations.insert(op.id.to_string(), Value::Object(entry));
    }
    operations
}

fn registry_digest_payload() -> Value {
    let contract = filesystem_contract_descriptor();
    json!({
        "abi_name": OPERATION_ABI_NAME, "abi_version": OPERATION_ABI_VERSION, "filesystem_contract_version": FILESYSTEM_CONTRACT_VERSION,
        "operations": Value::Object(registry_operations_map(false)), "surface_schemas_digest": super::operation_schemas::operation_abi_schemas_digest(),
        "surface_schemas": super::operation_schemas::operation_abi_schemas_document().clone(), "contract_operations": contract.get("operations").cloned().unwrap_or(Value::Null),
        "contract_aliases": contract.get("aliases").cloned().unwrap_or(Value::Null), "contract_error_classes": contract.get("error_classes").cloned().unwrap_or(Value::Null),
    })
}

/// Deterministic SHA-256 hex digest of the canonical ABI + full schema catalog + contract binding.
pub fn operation_abi_digest() -> String {
    zero_abi::contract_digest_hex(&registry_digest_payload())
}

static ABI_DESCRIPTOR: OnceLock<Value> = OnceLock::new();

/// Machine-readable ABI descriptor for doctor/root reports and recovery.
pub fn operation_abi_descriptor() -> &'static Value {
    ABI_DESCRIPTOR.get_or_init(|| {
        json!({
            "abi": {
                "name": OPERATION_ABI_NAME, "version": OPERATION_ABI_VERSION, "filesystem_contract_version": FILESYSTEM_CONTRACT_VERSION,
                "digest_algorithm": OPERATION_ABI_DIGEST_ALGORITHM, "digest": operation_abi_digest(),
                "schemas_digest": super::operation_schemas::operation_abi_schemas_digest(), "schemas_name": super::operation_schemas::OPERATION_ABI_SCHEMAS_NAME,
                "schemas_version": super::operation_schemas::OPERATION_ABI_SCHEMAS_VERSION, "operation_count": OPERATION_REGISTRY.len(),
            },
            "operations": Value::Object(registry_operations_map(true)), "surface_schemas": super::operation_schemas::operation_abi_schemas_document().clone(),
            "surface_dispatch_aliases": SURFACE_DISPATCH_ALIASES.iter().map(|(surface, alias)| json!({"surface": surface, "alias": alias})).collect::<Vec<_>>(), "error_retryability": error_retryability_table(),
        })
    })
}

fn error_retryability_table() -> Value {
    let classes = filesystem_contract_descriptor()
        .get("error_classes")
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    Value::Object(
        classes
            .into_iter()
            .map(|c| (c.clone(), json!(error_class_retryable(&c))))
            .collect(),
    )
}

fn insert_unique<T: Ord + std::fmt::Display + Copy>(
    set: &mut BTreeSet<T>,
    val: T,
    kind: &str,
) -> Result<(), String> {
    if set.insert(val) {
        Ok(())
    } else {
        Err(format!("duplicate {kind} {val}"))
    }
}

/// Validate registry internal uniqueness and binding to the filesystem contract.
pub fn validate_operation_abi() -> Result<(), String> {
    let mut seen = BTreeSet::new();
    let mut mcp_seen = BTreeSet::new();
    let mut cm_seen = BTreeSet::new();
    let mut cli_seen = BTreeSet::new();

    for op in OPERATION_REGISTRY {
        insert_unique(&mut seen, op.id, "operation id")?;
        for alias in op.mcp_aliases {
            insert_unique(&mut mcp_seen, *alias, "mcp alias")?;
        }
        for alias in op.codemode_aliases {
            insert_unique(&mut cm_seen, *alias, "codemode alias")?;
        }
        for ch in op.cli_opcodes {
            insert_unique(&mut cli_seen, *ch, "cli opcode")?;
        }
        // Domain schemas are the sole owner of input property/required sets
        // (fszero LOC: no hand-maintained input_keys index).
        let domain = op
            .domain_input_schema()
            .ok_or_else(|| format!("{} missing domain input schema", op.id))?;
        let props = domain
            .get("properties")
            .and_then(Value::as_object)
            .ok_or_else(|| format!("{} domain input missing properties", op.id))?;
        let schema_required: BTreeSet<&str> = domain
            .get("required")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        for key in &schema_required {
            if !props.contains_key(*key) {
                return Err(format!(
                    "{} required key {key} missing from domain input properties",
                    op.id
                ));
            }
        }

        if op.domain_output_schema().is_none() {
            return Err(format!("{} missing domain output schema", op.id));
        }
    }

    let registry_ops = operation_ids();
    super::operation_schemas::validate_operation_abi_schemas(&registry_ops)?;

    validate_filesystem_contract_document(filesystem_contract_descriptor())?;

    let contract_ops = filesystem_contract_operation_names();
    let registry_ops: BTreeSet<String> = OPERATION_REGISTRY
        .iter()
        .map(|op| op.id.to_string())
        .collect();
    if contract_ops != registry_ops {
        return Err(format!(
            "registry ops {:?} != contract ops {:?}",
            registry_ops
                .symmetric_difference(&contract_ops)
                .collect::<Vec<_>>(),
            contract_ops
                .symmetric_difference(&registry_ops)
                .collect::<Vec<_>>()
        ));
    }

    let contract = filesystem_contract_descriptor();
    let aliases = contract
        .get("aliases")
        .and_then(Value::as_object)
        .ok_or_else(|| "aliases missing".to_string())?;

    // Surface aliases: every contract mapping must match registry resolve_alias.
    for surface in ["mcp", "codemode", "cli"] {
        let map = aliases
            .get(surface)
            .and_then(Value::as_object)
            .ok_or_else(|| format!("aliases.{surface} missing"))?;
        for (alias, target) in map {
            let target = target
                .as_str()
                .ok_or_else(|| format!("aliases.{surface}.{alias} not string"))?;
            if surface == "mcp" && target == "surface_dispatch" {
                continue;
            }
            let resolved = resolve_alias(surface, alias)
                .ok_or_else(|| format!("{surface} alias {alias} missing from registry"))?;
            if resolved != target {
                return Err(format!(
                    "{surface} alias {alias}: registry→{resolved} contract→{target}"
                ));
            }
        }
    }
    let mcp = aliases.get("mcp").and_then(Value::as_object).unwrap();
    for alias in registry_mcp_aliases() {
        if alias != "fszero.exec" && !mcp.contains_key(alias) {
            return Err(format!("registry mcp alias {alias} missing from contract"));
        }
    }

    let codemode = aliases.get("codemode").and_then(Value::as_object).unwrap();
    for alias in registry_codemode_aliases() {
        if !codemode.contains_key(alias) {
            return Err(format!(
                "registry codemode alias {alias} missing from contract"
            ));
        }
    }
    let cli = aliases.get("cli").and_then(Value::as_object).unwrap();
    for ch in registry_cli_opcodes() {
        let key = ch.to_string();
        if !cli.contains_key(&key) {
            return Err(format!("registry cli opcode {ch} missing from contract"));
        }
    }

    // Every contract error class used by ops must exist; every registry op's
    // contract errors must be non-empty.
    let errors = contract
        .get("error_classes")
        .and_then(Value::as_object)
        .ok_or_else(|| "error_classes missing".to_string())?;
    let operations = contract
        .get("operations")
        .and_then(Value::as_object)
        .ok_or_else(|| "operations missing".to_string())?;
    for op in OPERATION_REGISTRY {
        let mapping = operations
            .get(op.id)
            .ok_or_else(|| format!("contract missing op {}", op.id))?;
        let op_errors = mapping
            .get("errors")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{}.errors missing", op.id))?;
        if op_errors.is_empty() {
            return Err(format!("{}.errors must not be empty", op.id));
        }
        for e in op_errors {
            let class = e
                .as_str()
                .ok_or_else(|| format!("{}.errors entry not string", op.id))?;
            if !errors.contains_key(class) {
                return Err(format!("{} references unknown error {class}", op.id));
            }
        }
    }

    // Digest must be stable / recompute to self.
    let d1 = operation_abi_digest();
    let d2 = operation_abi_digest();
    if d1 != d2 || d1.len() != 64 {
        return Err("operation abi digest unstable or wrong length".into());
    }

    Ok(())
}

fn live_set_matches_registry(
    surface: &str,
    expected_static: BTreeSet<&'static str>,
    live: &BTreeSet<String>,
) -> Result<(), String> {
    let expected: BTreeSet<String> = expected_static.into_iter().map(str::to_string).collect();
    if &expected != live {
        return Err(format!(
            "{surface} catalog mismatch missing={:?} extra={:?}",
            expected.difference(live).collect::<Vec<_>>(),
            live.difference(&expected).collect::<Vec<_>>()
        ));
    }
    Ok(())
}

/// Compare live MCP tool names to the registry (+ surface dispatch).
pub fn live_mcp_matches_registry(live: &BTreeSet<String>) -> Result<(), String> {
    live_set_matches_registry("mcp", registry_mcp_aliases(), live)
}

/// Compare live CodeMode method paths to the registry.
pub fn live_codemode_matches_registry(live: &BTreeSet<String>) -> Result<(), String> {
    live_set_matches_registry("codemode", registry_codemode_aliases(), live)
}

/// Exact MCP tool schema parity against the canonical schema document.
///
/// Rejects missing/extra properties, type changes, requiredness drift,
/// constraint drift, and output-shape drift. The `properties` argument is
/// accepted for call-site compatibility but ignored in favor of full-tool
/// validation when `full_tool` is provided via
/// [`super::operation_schemas::validate_live_mcp_tool`].
pub fn validate_mcp_tool_schema(tool: &str, properties: &BTreeSet<String>) -> Result<(), String> {
    let allowed = registry_mcp_input_keys(tool)
        .ok_or_else(|| format!("unknown mcp tool in registry: {tool}"))?;
    let allowed: BTreeSet<String> = allowed.into_iter().map(str::to_string).collect();
    if properties != &allowed {
        return Err(format!(
            "mcp tool {tool} property set mismatch missing={:?} extra={:?}",
            allowed.difference(properties).collect::<Vec<_>>(),
            properties.difference(&allowed).collect::<Vec<_>>()
        ));
    }
    // Full structural validation against the materialized catalog entry.
    let live = super::operation_schemas::materialize_mcp_tools()
        .into_iter()
        .find(|t| t.get("name").and_then(Value::as_str) == Some(tool))
        .ok_or_else(|| format!("cannot materialize mcp tool {tool}"))?;
    super::operation_schemas::validate_live_mcp_tool(&live)
}

#[cfg(test)]
#[path = "../../../../tests/fszero/unit/fszero-core/domain_error_ctors_tests.rs"]
mod domain_error_ctors_tests;
