//! Static inventory of every public TokenZero operation (tokenzero-irx9.1).

use serde_json::{Value, json};

use super::schemas::{
    args, batch_schema, cache_pack_schema, codemode_describe_schema, codemode_search_schema,
    default_results, edit_schema, execute_code_schema, expand_schema, fetch_schema, glob_schema,
    no_args_schema, read_schema, recall_schema, ref_first_results, report_tool_issue_schema,
    rewrite_schema, search_schema, shell_schema, text_schema, tree_schema,
};
use super::types::{
    CancellationSemantics, CostClass, DomainErrorKind, MigrationStatus, Mutability, Operation,
    OperationResults, RefOwnership, SurfaceExposure,
};

fn read_errors() -> &'static [DomainErrorKind] {
    &[
        DomainErrorKind::Validation,
        DomainErrorKind::Substrate,
        DomainErrorKind::NotFound,
        DomainErrorKind::DeadlineExceeded,
        DomainErrorKind::Cancelled,
        DomainErrorKind::Busy,
        DomainErrorKind::Unauthorized,
        DomainErrorKind::Policy,
    ]
}

fn search_errors() -> &'static [DomainErrorKind] {
    &[
        DomainErrorKind::Validation,
        DomainErrorKind::InvalidPattern,
        DomainErrorKind::Substrate,
        DomainErrorKind::DeadlineExceeded,
        DomainErrorKind::Cancelled,
        DomainErrorKind::Busy,
        DomainErrorKind::Unauthorized,
    ]
}

fn edit_errors() -> &'static [DomainErrorKind] {
    &[
        DomainErrorKind::Validation,
        DomainErrorKind::Policy,
        DomainErrorKind::HunkNotFound,
        DomainErrorKind::AmbiguousHunk,
        DomainErrorKind::NoOpHunk,
        DomainErrorKind::Substrate,
        DomainErrorKind::DeadlineExceeded,
        DomainErrorKind::Cancelled,
        DomainErrorKind::Unauthorized,
    ]
}

fn shell_errors() -> &'static [DomainErrorKind] {
    &[
        DomainErrorKind::Validation,
        DomainErrorKind::Policy,
        DomainErrorKind::Approval,
        DomainErrorKind::Sandbox,
        DomainErrorKind::Runtime,
        DomainErrorKind::DeadlineExceeded,
        DomainErrorKind::Cancelled,
        DomainErrorKind::Busy,
        DomainErrorKind::Unauthorized,
    ]
}

#[allow(dead_code)]
fn job_errors() -> &'static [DomainErrorKind] {
    &[
        DomainErrorKind::Validation,
        DomainErrorKind::NotFound,
        DomainErrorKind::Runtime,
    ]
}

fn expand_errors() -> &'static [DomainErrorKind] {
    &[
        DomainErrorKind::Validation,
        DomainErrorKind::InvalidRef,
        DomainErrorKind::NotFound,
        DomainErrorKind::Substrate,
        DomainErrorKind::DeadlineExceeded,
        DomainErrorKind::Cancelled,
    ]
}

fn fetch_errors() -> &'static [DomainErrorKind] {
    &[
        DomainErrorKind::Validation,
        DomainErrorKind::InvalidUrl,
        DomainErrorKind::Runtime,
        DomainErrorKind::Policy,
        DomainErrorKind::DeadlineExceeded,
        DomainErrorKind::Cancelled,
    ]
}

/// Field bundles for the registry helpers (tokenzero-gnt-clippy-core-abi-8peh):
/// named struct fields replace helper signatures with more than seven
/// positional parameters (clippy::too_many_arguments).
struct ClassicSpec {
    name: &'static str,
    description: &'static str,
    aliases: &'static [&'static str],
    mutability: Mutability,
    cost_class: CostClass,
    ref_ownership: RefOwnership,
    cancellation: CancellationSemantics,
    binding: &'static str,
    capabilities: &'static [&'static str],
    cluster: &'static str,
    schema: Value,
    error_kinds: &'static [DomainErrorKind],
}

/// Classic knobs plus extended results and argument aliases.
struct ClassicExSpec {
    name: &'static str,
    description: &'static str,
    aliases: &'static [&'static str],
    mutability: Mutability,
    cost_class: CostClass,
    ref_ownership: RefOwnership,
    cancellation: CancellationSemantics,
    binding: &'static str,
    capabilities: &'static [&'static str],
    cluster: &'static str,
    schema: Value,
    results: OperationResults,
    error_kinds: &'static [DomainErrorKind],
    arg_aliases: Value,
}

/// CodeMode binding-only knobs (the op name doubles as the binding path).
struct BindingSpec {
    name: &'static str,
    description: &'static str,
    mutability: Mutability,
    cost_class: CostClass,
    ref_ownership: RefOwnership,
    cancellation: CancellationSemantics,
    capabilities: &'static [&'static str],
    schema: Value,
    results: OperationResults,
    error_kinds: &'static [DomainErrorKind],
}

/// Binding knobs plus canonical-name aliases.
struct BindingExSpec {
    name: &'static str,
    description: &'static str,
    aliases: &'static [&'static str],
    mutability: Mutability,
    cost_class: CostClass,
    ref_ownership: RefOwnership,
    cancellation: CancellationSemantics,
    capabilities: &'static [&'static str],
    schema: Value,
    results: OperationResults,
    error_kinds: &'static [DomainErrorKind],
}

/// Terminal constructor bundle for the shared op constructor.
struct OpParts {
    name: &'static str,
    description: &'static str,
    aliases: &'static [&'static str],
    mutability: Mutability,
    cost_class: CostClass,
    ref_ownership: RefOwnership,
    cancellation: CancellationSemantics,
    migration: MigrationStatus,
    exposure: SurfaceExposure,
    capabilities: &'static [&'static str],
    cluster: &'static str,
    schema: Value,
    results: OperationResults,
    error_kinds: &'static [DomainErrorKind],
    arg_aliases: Value,
}

/// Shared constructor: every op is Public; surfaces/results/aliases vary by helper.
fn op(parts: OpParts) -> Operation {
    Operation {
        name: parts.name,
        description: parts.description,
        aliases: parts.aliases,
        mutability: parts.mutability,
        capability: super::types::CapabilityRequirement::Public,
        cost_class: parts.cost_class,
        ref_ownership: parts.ref_ownership,
        cancellation: parts.cancellation,
        migration: parts.migration,
        exposure: parts.exposure,
        capabilities: parts.capabilities,
        cluster: parts.cluster,
        args: args(parts.schema),
        results: parts.results,
        error_kinds: parts.error_kinds,
        arg_aliases: parts.arg_aliases,
    }
}

fn classic_surface(binding: Option<&'static str>, codemode_mcp: bool) -> SurfaceExposure {
    SurfaceExposure {
        fastmcp_tool: true,
        codemode_mcp_tool: codemode_mcp,
        codemode_binding: binding,
        resource_uri: None,
    }
}

/// Classic FastMCP + CodeMode domain tool (canonical, ref-first, empty arg aliases).
fn classic(spec: ClassicSpec) -> Operation {
    classic_ex(ClassicExSpec {
        name: spec.name,
        description: spec.description,
        aliases: spec.aliases,
        mutability: spec.mutability,
        cost_class: spec.cost_class,
        ref_ownership: spec.ref_ownership,
        cancellation: spec.cancellation,
        binding: spec.binding,
        capabilities: spec.capabilities,
        cluster: spec.cluster,
        schema: spec.schema,
        results: ref_first_results(),
        error_kinds: spec.error_kinds,
        arg_aliases: json!({}),
    })
}

fn classic_ex(spec: ClassicExSpec) -> Operation {
    let _retired_v6_binding = spec.binding;
    op(OpParts {
        name: spec.name,
        description: spec.description,
        aliases: spec.aliases,
        mutability: spec.mutability,
        cost_class: spec.cost_class,
        ref_ownership: spec.ref_ownership,
        cancellation: spec.cancellation,
        migration: MigrationStatus::Canonical,
        exposure: classic_surface(None, false),
        capabilities: spec.capabilities,
        cluster: spec.cluster,
        schema: spec.schema,
        results: spec.results,
        error_kinds: spec.error_kinds,
        arg_aliases: spec.arg_aliases,
    })
}

/// CodeMode binding-only helper (the op name is also the binding path).
fn binding(spec: BindingSpec) -> Operation {
    binding_ex(BindingExSpec {
        name: spec.name,
        description: spec.description,
        aliases: &[],
        mutability: spec.mutability,
        cost_class: spec.cost_class,
        ref_ownership: spec.ref_ownership,
        cancellation: spec.cancellation,
        capabilities: spec.capabilities,
        schema: spec.schema,
        results: spec.results,
        error_kinds: spec.error_kinds,
    })
}

fn binding_ex(spec: BindingExSpec) -> Operation {
    op(OpParts {
        name: spec.name,
        description: spec.description,
        aliases: spec.aliases,
        mutability: spec.mutability,
        cost_class: spec.cost_class,
        ref_ownership: spec.ref_ownership,
        cancellation: spec.cancellation,
        migration: MigrationStatus::CodemodeControl,
        exposure: SurfaceExposure {
            fastmcp_tool: false,
            codemode_mcp_tool: false,
            codemode_binding: Some(spec.name),
            resource_uri: None,
        },
        capabilities: spec.capabilities,
        cluster: "codemode",
        schema: spec.schema,
        results: spec.results,
        error_kinds: spec.error_kinds,
        arg_aliases: json!({}),
    })
}
fn resource(
    name: &'static str,
    description: &'static str,
    uri: &'static str,
    capabilities: &'static [&'static str],
    ref_ownership: RefOwnership,
) -> Operation {
    op(OpParts {
        name,
        description,
        aliases: &[],
        mutability: Mutability::ReadOnly,
        cost_class: CostClass::Cheap,
        ref_ownership,
        cancellation: CancellationSemantics::None,
        migration: MigrationStatus::Resource,
        exposure: SurfaceExposure {
            fastmcp_tool: false,
            codemode_mcp_tool: false,
            codemode_binding: None,
            resource_uri: Some(uri),
        },
        capabilities,
        cluster: "resource",
        schema: no_args_schema(),
        results: default_results(),
        error_kinds: read_errors(),
        arg_aliases: json!({}),
    })
}

/// Runtime builder: `json!` schemas are owned Values, so the registry is
/// materialized once via `OnceLock` and sorted by canonical name.
pub fn all_operations() -> &'static [Operation] {
    use std::sync::OnceLock;
    static REG: OnceLock<Vec<Operation>> = OnceLock::new();
    REG.get_or_init(build_registry).as_slice()
}

pub fn operation_by_name(name: &str) -> Option<&'static Operation> {
    all_operations().iter().find(|op| op.name == name)
}

fn build_registry() -> Vec<Operation> {
    use CancellationSemantics as C;
    use CostClass as K;
    use Mutability as M;
    use RefOwnership as R;

    let mut ops = vec![
        // --- Classic domain tools (FastMCP + CodeMode bindings) ---
        classic(ClassicSpec {
            name: "tz_read",
            description: "Read file(s) under allowed roots: compact visible output plus exact tz:// recovery refs.",
            aliases: &["read"],
            mutability: M::ReadOnly,
            cost_class: K::Medium,
            ref_ownership: R::Blob,
            cancellation: C::Deadline,
            binding: "zero.read",
            capabilities: &["read", "exact-refs", "line-range", "shared-cas"],
            cluster: "material",
            schema: read_schema(),
            error_kinds: read_errors(),
        }),
        classic(ClassicSpec {
            name: "tz_find",
            description: "Search file contents for a literal substring and return compact, recoverable matches.",
            aliases: &["find"],
            mutability: M::ReadOnly,
            cost_class: K::Medium,
            ref_ownership: R::Blob,
            cancellation: C::Deadline,
            binding: "zero.find",
            capabilities: &["search", "literal", "exact-refs", "shared-cas"],
            cluster: "material",
            schema: search_schema("Literal substring to search for."),
            error_kinds: search_errors(),
        }),
        classic(ClassicSpec {
            name: "tz_grep",
            description: "Grep-style exact-first content search: regex when ripgrep is active, literal otherwise.",
            aliases: &["grep"],
            mutability: M::ReadOnly,
            cost_class: K::Medium,
            ref_ownership: R::Blob,
            cancellation: C::Deadline,
            binding: "zero.grep",
            capabilities: &["search", "regex", "exact-refs", "shared-cas"],
            cluster: "material",
            schema: search_schema(
                "Search pattern: regex under the ripgrep backend, literal substring under the internal fallback.",
            ),
            error_kinds: search_errors(),
        }),
        classic(ClassicSpec {
            name: "tz_recall",
            description: "Search every payload already stored in the recovery cache.",
            aliases: &["recall"],
            mutability: M::ReadOnly,
            cost_class: K::Cheap,
            ref_ownership: R::Blob,
            cancellation: C::Cooperative,
            binding: "zero.recall",
            capabilities: &["search", "cache", "exact-refs", "shared-cas"],
            cluster: "material",
            schema: recall_schema(),
            error_kinds: read_errors(),
        }),
        classic(ClassicSpec {
            name: "tz_batch",
            description: "Run several TokenZero ops in one call: one combined capsule, per-op sections, unioned refs.",
            aliases: &["batch"],
            mutability: M::WorkspaceMutating,
            cost_class: K::Heavy,
            ref_ownership: R::Multi,
            cancellation: C::Deadline,
            binding: "zero.batch",
            capabilities: &["batch", "exact-refs"],
            cluster: "execution",
            schema: batch_schema(),
            error_kinds: read_errors(),
        }),
        classic(ClassicSpec {
            name: "tz_fetch",
            description: "Fetch an http(s) URL via curl with a TTL cache and exact tz:// refs.",
            aliases: &["fetch"],
            mutability: M::StoreOnly,
            cost_class: K::Heavy,
            ref_ownership: R::Blob,
            cancellation: C::Deadline,
            binding: "zero.fetch",
            capabilities: &["fetch", "web", "cache", "exact-refs"],
            cluster: "web",
            schema: fetch_schema(),
            error_kinds: fetch_errors(),
        }),
        classic_ex(ClassicExSpec {
            name: "tz_glob",
            description: "List file paths matching a glob pattern (no contents).",
            aliases: &["glob"],
            mutability: M::ReadOnly,
            cost_class: K::Cheap,
            ref_ownership: R::Blob,
            cancellation: C::Cooperative,
            binding: "zero.glob",
            capabilities: &["discover", "glob", "shared-cas"],
            cluster: "material",
            schema: glob_schema(),
            results: ref_first_results(),
            error_kinds: read_errors(),
            arg_aliases: json!({ "pattern": ["glob", "query"] }),
        }),
        classic(ClassicSpec {
            name: "tz_tree",
            description: "Inspect a bounded directory tree for orientation.",
            aliases: &["tree"],
            mutability: M::ReadOnly,
            cost_class: K::Cheap,
            ref_ownership: R::Blob,
            cancellation: C::Cooperative,
            binding: "zero.tree",
            capabilities: &["discover", "tree", "shared-cas"],
            cluster: "material",
            schema: tree_schema(),
            error_kinds: read_errors(),
        }),
        classic(ClassicSpec {
            name: "tz_edit",
            description: "Apply multi-hunk find/replace edits to one file atomically with undo via tz:// ref.",
            aliases: &["edit"],
            mutability: M::WorkspaceMutating,
            cost_class: K::Medium,
            ref_ownership: R::Blob,
            cancellation: C::None,
            binding: "zero.edit",
            capabilities: &["write", "atomic", "exact-refs"],
            cluster: "edit",
            schema: edit_schema(),
            error_kinds: edit_errors(),
        }),
        classic_ex(ClassicExSpec {
            name: "tz_shell",
            description: "Run a local command: compact output, exact stream refs, command_success telemetry.",
            aliases: &["shell"],
            mutability: M::WorkspaceMutating,
            cost_class: K::Heavy,
            ref_ownership: R::Blob,
            cancellation: C::Deadline,
            binding: "zero.shell",
            capabilities: &["shell", "exact-refs", "command-success"],
            cluster: "execution",
            schema: shell_schema(),
            results: ref_first_results(),
            error_kinds: shell_errors(),
            arg_aliases: json!({
                "command": ["cmd", "input", "script"],
                "argv": ["args"],
                "timeout_seconds": ["timeout_secs", "timeout", "shell_timeout_seconds"]
            }),
        }),
        classic_ex(ClassicExSpec {
            name: "tz_ingest",
            description: "Store external text behind exact tz:// refs and return a compact capsule.",
            aliases: &["ingest"],
            mutability: M::StoreOnly,
            cost_class: K::Cheap,
            ref_ownership: R::Blob,
            cancellation: C::None,
            binding: "zero.ingest",
            capabilities: &["ingest", "exact-refs"],
            cluster: "execution",
            schema: text_schema("External text payload to store behind exact refs."),
            results: ref_first_results(),
            error_kinds: read_errors(),
            arg_aliases: json!({ "text": ["input"] }),
        }),
        classic(ClassicSpec {
            name: "tz_expand",
            description: "Recover exact bytes from a tz://, fz://, or gz:// ref.",
            aliases: &["expand"],
            mutability: M::ReadOnly,
            cost_class: K::Cheap,
            ref_ownership: R::Blob,
            cancellation: C::Deadline,
            binding: "zero.token.expand",
            capabilities: &[
                "expand",
                "exact-refs",
                "fragment-selectors",
                "symbol-anchors",
                "diff-baseline",
                "shared-cas",
            ],
            cluster: "material",
            schema: expand_schema(),
            error_kinds: expand_errors(),
        }),
        classic_ex(ClassicExSpec {
            name: "tz_mem",
            description: "Inspect local recovery-cache and configuration state.",
            aliases: &["mem"],
            mutability: M::ReadOnly,
            cost_class: K::Cheap,
            ref_ownership: R::None,
            cancellation: C::None,
            binding: "zero.mem",
            capabilities: &["diagnostic", "cache"],
            cluster: "execution",
            schema: no_args_schema(),
            results: default_results(),
            error_kinds: read_errors(),
            arg_aliases: json!({}),
        }),
        classic(ClassicSpec {
            name: "tz_cache_pack",
            description: "Build a daemonless prompt-cache pack with a stable prefix and volatile refs.",
            aliases: &["cache_pack", "cache-pack"],
            mutability: M::StoreOnly,
            cost_class: K::Medium,
            ref_ownership: R::Multi,
            cancellation: C::Cooperative,
            binding: "zero.cache_pack",
            capabilities: &["cache", "prompt-cache"],
            cluster: "execution",
            schema: cache_pack_schema(),
            error_kinds: read_errors(),
        }),
        classic_ex(ClassicExSpec {
            name: "tz_rewrite",
            description: "Plan a conservative TokenZero-safe rewrite of a shell command without executing it.",
            aliases: &["rewrite"],
            mutability: M::ReadOnly,
            cost_class: K::Cheap,
            ref_ownership: R::None,
            cancellation: C::None,
            binding: "zero.rewrite",
            capabilities: &["diagnostic", "rewrite"],
            cluster: "execution",
            schema: rewrite_schema(),
            results: default_results(),
            error_kinds: shell_errors(),
            arg_aliases: json!({ "command": ["cmd", "input", "script"], "argv": ["args"] }),
        }),
        classic_ex(ClassicExSpec {
            name: "tz_discover",
            description: "Report TokenZero filter and runtime readiness metadata.",
            aliases: &["discover"],
            mutability: M::ReadOnly,
            cost_class: K::Cheap,
            ref_ownership: R::None,
            cancellation: C::None,
            binding: "zero.discover",
            capabilities: &["diagnostic", "discovery"],
            cluster: "execution",
            schema: no_args_schema(),
            results: default_results(),
            error_kinds: read_errors(),
            arg_aliases: json!({}),
        }),
        // --- CodeMode MCP control tools ---
        op(OpParts {
            name: "tz_execute_code",
            description: "Execute a TokenZero CodeMode recipe, JSON plan, or JavaScript plan.",
            aliases: &[],
            mutability: M::WorkspaceMutating,
            cost_class: K::Heavy,
            ref_ownership: R::Execution,
            cancellation: C::Deadline,
            migration: MigrationStatus::CodemodeControl,
            exposure: SurfaceExposure {
                fastmcp_tool: false,
                codemode_mcp_tool: true,
                codemode_binding: None,
                resource_uri: None,
            },
            capabilities: &["codemode", "plan-execution", "sandboxed"],
            cluster: "codemode",
            schema: execute_code_schema(),
            results: default_results(),
            error_kinds: shell_errors(),
            arg_aliases: json!({}),
        }),
        op(OpParts {
            name: "tz_codemode_search",
            description: "Search the TokenZero CodeMode method catalog.",
            aliases: &[],
            mutability: M::ReadOnly,
            cost_class: K::Cheap,
            ref_ownership: R::None,
            cancellation: C::None,
            migration: MigrationStatus::CodemodeControl,
            exposure: SurfaceExposure {
                fastmcp_tool: false,
                codemode_mcp_tool: true,
                codemode_binding: Some("codemode.search"),
                resource_uri: None,
            },
            capabilities: &["codemode", "catalog-search", "read-only"],
            cluster: "codemode",
            schema: codemode_search_schema(),
            results: default_results(),
            error_kinds: read_errors(),
            arg_aliases: json!({}),
        }),
        op(OpParts {
            name: "tz_codemode_describe",
            description: "Describe a TokenZero CodeMode method or capabilities manifest.",
            aliases: &[],
            mutability: M::ReadOnly,
            cost_class: K::Cheap,
            ref_ownership: R::None,
            cancellation: C::None,
            migration: MigrationStatus::CodemodeControl,
            exposure: SurfaceExposure {
                fastmcp_tool: false,
                codemode_mcp_tool: true,
                codemode_binding: Some("codemode.describe"),
                resource_uri: None,
            },
            capabilities: &["codemode", "catalog-describe", "read-only"],
            cluster: "codemode",
            schema: codemode_describe_schema(),
            results: default_results(),
            error_kinds: read_errors(),
            arg_aliases: json!({}),
        }),
        op(OpParts {
            name: "tz_report_tool_issue",
            description: "Record a field issue against a CodeMode/TokenZero tool name.",
            aliases: &["report_tool_issue", "report-tool-issue"],
            mutability: M::StoreOnly,
            cost_class: K::Cheap,
            ref_ownership: R::None,
            cancellation: C::None,
            migration: MigrationStatus::Canonical,
            exposure: classic_surface(None, true),
            capabilities: &["diagnostic", "report"],
            cluster: "codemode",
            schema: report_tool_issue_schema(),
            results: default_results(),
            error_kinds: read_errors(),
            arg_aliases: json!({
                "tool": ["name", "tool_name", "surface"],
                "summary": ["message", "title"],
                "detail": ["body", "repro", "context"]
            }),
        }),
        binding(BindingSpec {
            name: "codemode.journalDoctor",
            description: "List unresolved plan journals and safe recovery advice.",
            mutability: M::ReadOnly,
            cost_class: K::Cheap,
            ref_ownership: R::Session,
            cancellation: C::None,
            capabilities: &["codemode", "journal"],
            schema: no_args_schema(),
            results: default_results(),
            error_kinds: read_errors(),
        }),
        binding(BindingSpec {
            name: "codemode.journalInspect",
            description: "Inspect a redacted durable plan journal by execution id.",
            mutability: M::ReadOnly,
            cost_class: K::Cheap,
            ref_ownership: R::Session,
            cancellation: C::None,
            capabilities: &["codemode", "journal"],
            schema: json!({
                "type": "object",
                "properties": { "execution_id": {"type": "string", "minLength": 1} },
                "required": ["execution_id"]
            }),
            results: default_results(),
            error_kinds: read_errors(),
        }),
        binding(BindingSpec {
            name: "codemode.journalResume",
            description: "Validate that an unresolved journal can be safely resumed.",
            mutability: M::ReadOnly,
            cost_class: K::Cheap,
            ref_ownership: R::Session,
            cancellation: C::None,
            capabilities: &["codemode", "journal"],
            schema: json!({
                "type": "object",
                "properties": { "execution_id": {"type": "string", "minLength": 1} },
                "required": ["execution_id"]
            }),
            results: default_results(),
            error_kinds: read_errors(),
        }),
        binding(BindingSpec {
            name: "codemode.journalRollback",
            description: "CAS-verified reverse-order rollback of an unresolved plan journal.",
            mutability: M::WorkspaceMutating,
            cost_class: K::Medium,
            ref_ownership: R::Session,
            cancellation: C::Cooperative,
            capabilities: &["codemode", "journal", "rollback"],
            schema: json!({
                "type": "object",
                "properties": { "execution_id": {"type": "string", "minLength": 1} },
                "required": ["execution_id"]
            }),
            results: default_results(),
            error_kinds: edit_errors(),
        }),
        binding(BindingSpec {
            name: "codemode.limits",
            description: "Return active CodeMode sandbox, output, ref, and operation limits.",
            mutability: M::ReadOnly,
            cost_class: K::Cheap,
            ref_ownership: R::None,
            cancellation: C::None,
            capabilities: &["codemode", "limits"],
            schema: no_args_schema(),
            results: default_results(),
            error_kinds: read_errors(),
        }),
        // --- Resources ---
        resource(
            "resource.capabilities",
            "Discover tool clusters, aliases, protocol versions, and next recommended calls.",
            "resource://tokenzero/capabilities",
            &["resource"],
            R::None,
        ),
        resource(
            "resource.tools",
            "Complete tool catalog with schemas and agent-oriented descriptions.",
            "resource://tokenzero/tools",
            &["resource"],
            R::None,
        ),
        resource(
            "resource.roots",
            "File-system roots that read/find/tree/shell cwd operations may access.",
            "resource://tokenzero/roots",
            &["resource", "policy"],
            R::None,
        ),
        resource(
            "resource.modes",
            "Accepted mode values for compacting, diagnostics, exact recovery, and pass-through.",
            "resource://tokenzero/modes",
            &["resource"],
            R::None,
        ),
        resource(
            "resource.codemode",
            "Full CodeMode method catalog with signatures and discovery prefixes.",
            "resource://tokenzero/codemode",
            &["resource", "codemode"],
            R::None,
        ),
        resource(
            "resource.cache",
            "Local recovery-cache and shell-output retention configuration.",
            "resource://tokenzero/cache",
            &["resource", "cache"],
            R::None,
        ),
        resource(
            "resource.session_boot",
            "Bounded manifest+delta boot capsule and component token attribution.",
            "resource://tokenzero/session-boot",
            &["resource", "session"],
            R::Session,
        ),
        resource(
            "resource.metrics",
            "Per-tool call counts, error counts, slow-call counts, and latency.",
            "resource://tokenzero/metrics",
            &["resource", "telemetry"],
            R::None,
        ),
        resource(
            "resource.shell_contract",
            "Shell transport, command-success, exact-ref, timeout, and retry semantics.",
            "resource://tokenzero/shell-contract",
            &["resource", "shell", "policy"],
            R::None,
        ),
    ];

    ops.sort_by(|a, b| a.name.cmp(b.name));
    ops
}
