//! Static inventory of every public GraphZero operation.

use serde_json::{Value, json};

use super::types::{
    CancellationSemantics, CapabilityRequirement, CostClass, DomainErrorKind, MigrationStatus,
    Mutability, Operation, OperationArgs, OperationResults, RefOwnership, SurfaceExposure,
};

fn args(properties: Value, required: &[&str]) -> OperationArgs {
    OperationArgs {
        schema: json!({
            "type": "object",
            "properties": properties,
            "required": required,
        }),
    }
}

/// Default normalized domain result envelope (success + typed error union).
fn default_results() -> OperationResults {
    OperationResults {
        schema: json!({
            "oneOf": [
                {
                    "type": "object",
                    "description": "DomainResult success envelope",
                    "properties": {
                        "value": {},
                        "refs": { "type": "array", "items": { "type": "string" } },
                        "op": { "type": "string", "minLength": 1 },
                        "telemetry": { "type": "object" }
                    },
                    "required": ["value", "op"]
                },
                {
                    "type": "object",
                    "description": "DomainError envelope",
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": [
                                "validation", "policy", "sandbox", "runtime", "substrate",
                                "busy", "approval", "cancelled", "deadline_exceeded",
                                "not_found", "unauthorized"
                            ]
                        },
                        "message": { "type": "string" },
                        "retryable": { "type": "boolean" },
                        "op": { "type": "string" },
                        "recovery_ref": { "type": "string" }
                    },
                    "required": ["kind", "message", "retryable"]
                }
            ]
        }),
    }
}

/// Ref-first success shape used by orient/blast/snap-class ops.
fn ref_first_results() -> OperationResults {
    OperationResults {
        schema: json!({
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "value": {
                            "type": "object",
                            "properties": {
                                "ack": { "type": "string" },
                                "surface": { "type": "string" }
                            }
                        },
                        "refs": {
                            "type": "array",
                            "items": { "type": "string", "minLength": 1 },
                            "minItems": 0
                        },
                        "op": { "type": "string", "minLength": 1 },
                        "telemetry": { "type": "object" }
                    },
                    "required": ["value", "op", "refs"]
                },
                {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string" },
                        "message": { "type": "string" },
                        "retryable": { "type": "boolean" },
                        "op": { "type": "string" },
                        "recovery_ref": { "type": "string" }
                    },
                    "required": ["kind", "message", "retryable"]
                }
            ]
        }),
    }
}

fn read_errors() -> &'static [DomainErrorKind] {
    &[
        DomainErrorKind::Validation,
        DomainErrorKind::Substrate,
        DomainErrorKind::NotFound,
        DomainErrorKind::DeadlineExceeded,
        DomainErrorKind::Cancelled,
        DomainErrorKind::Busy,
        DomainErrorKind::Unauthorized,
    ]
}

fn mutate_errors() -> &'static [DomainErrorKind] {
    &[
        DomainErrorKind::Validation,
        DomainErrorKind::Policy,
        DomainErrorKind::Substrate,
        DomainErrorKind::DeadlineExceeded,
        DomainErrorKind::Cancelled,
        DomainErrorKind::Busy,
        DomainErrorKind::Unauthorized,
    ]
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
    let mut ops = vec![
        Operation {
            name: "orient",
            description: "One-shot codebase orientation: symbol/callers/deps/outline/context/hot/changes/word/search/locate/delta/recall/callpath/reading_set via surface param; returns ref-first JSON under budget. Agent hint: run orient before touching a symbol so you know the callers/deps/snapshots you'll edit.",
            aliases: &[],
            mutability: Mutability::ReadOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Medium,
            ref_ownership: RefOwnership::Query,
            cancellation: CancellationSemantics::Deadline,
            migration: MigrationStatus::Canonical,
            exposure: SurfaceExposure {
                fastmcp_tool: true,
                codemode_binding: Some("graph.orient"),
                codemode_meta: false,
            },
            args: args(
                json!({
                    "surface": { "type": "string", "description": "symbol|callers|deps|outline|context|hot|changes|word|search|locate|delta|recall|callpath|reading_set (MCP orient + CLI graphzero orient)", "default": "context" },
                    "query": { "type": "string", "description": "Symbol name, path, or natural-language task" },
                    "name": { "type": "string", "description": "Alias for symbol/callers/deps when surface needs a name" },
                    "path": { "type": "string", "description": "File path when surface=outline" },
                    "budget": { "type": "integer", "description": "Visible token budget (1 = ref-only capsule)", "default": 1 },
                    "repo": { "type": "string", "default": "." },
                    "session": { "type": "string", "description": "Dedup session id" },
                }),
                &["query"],
            ),
            results: ref_first_results(),

            error_kinds: read_errors(),
            is_orient_router: true,
        },
        Operation {
            name: "search",
            description: "Search the indexed snapshot (trigram + symbol), or exact worktree text when no index exists; returns gz:// evidence refs, expand for bytes. Agent hint: run search whenever you need quick text or symbol matches before reading files. Large result sets page via cursor (gz://query/<id>), not truncate-or-flood.",
            aliases: &[],
            mutability: Mutability::ReadOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Medium,
            ref_ownership: RefOwnership::Multi,
            cancellation: CancellationSemantics::Deadline,
            migration: MigrationStatus::Canonical,
            exposure: SurfaceExposure {
                fastmcp_tool: true,
                // CodeMode reaches search via graph.query('search', …) / orient.
                codemode_binding: None,
                codemode_meta: false,
            },
            args: args(
                json!({
                    "query": { "type": "string", "description": "Search string" },
                    "budget": { "type": "integer", "default": 1 },
                    "repo": { "type": "string", "default": "." },
                }),
                &["query"],
            ),
            results: ref_first_results(),

            error_kinds: read_errors(),
            is_orient_router: false,
        },
        Operation {
            name: "snap",
            description: "Edit-ready symbol snap: returns path/line/byte_span/definition_kind/confidence/alternates (tcx3) plus budgeted gz:// evidence. Prefer zero.graph.snap(symbol) over grep-then-read. At budget=1 capsule may be a single ref; edit fields stay inline. --export_path for atomic snap-to-file (minimal/capsule/md/zst).",
            aliases: &[],
            mutability: Mutability::ReadOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Medium,
            ref_ownership: RefOwnership::Query,
            cancellation: CancellationSemantics::Deadline,
            migration: MigrationStatus::Canonical,
            exposure: SurfaceExposure {
                fastmcp_tool: true,
                codemode_binding: Some("graph.snap"),
                codemode_meta: false,
            },
            args: args(
                json!({
                    "query": { "type": "string", "description": "Symbol or search text" },
                    "symbol": { "type": "string", "description": "Alias for query" },
                    "budget": { "type": "integer", "default": 1 },
                    "repo": { "type": "string", "default": "." },
                    "session": { "type": "string" },
                    "export_path": { "type": "string", "description": "Optional path for atomic export (snap --to-file)" },
                    "export": { "type": "string", "description": "Alias for export_path" },
                    "to_file": { "type": "string", "description": "Alias" },
                    "format": { "type": "string", "enum": ["minimal", "capsule", "md", "zst"], "default": "minimal" },
                }),
                &[],
            ),
            results: ref_first_results(),

            error_kinds: read_errors(),
            is_orient_router: false,
        },
        Operation {
            name: "remember",
            description: "Persist a short decision-memory fact; returns gz://mem/<id> (expand for exact JSON).",
            aliases: &[],
            mutability: Mutability::StoreOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Cheap,
            ref_ownership: RefOwnership::Mem,
            cancellation: CancellationSemantics::Cooperative,
            migration: MigrationStatus::Canonical,
            exposure: SurfaceExposure {
                fastmcp_tool: true,
                codemode_binding: Some("graph.remember"),
                codemode_meta: false,
            },
            args: args(
                json!({
                    "text": { "type": "string" },
                    "anchors": { "type": "array", "items": { "type": "string" }, "description": "Paths/symbols this fact anchors; alias: singular `anchor` string or array" },
                    "anchor": { "type": "string", "description": "Alias for a single anchors[] entry" },
                    "kind": { "type": "string", "description": "decision|invariant|gotcha|note" },
                    "supersedes": { "type": "array", "items": { "type": "string" }, "description": "memory ids or gz://mem/<id> refs this fact supersedes" },
                    "repo": { "type": "string", "default": "." },
                }),
                &["text"],
            ),
            results: default_results(),

            error_kinds: mutate_errors(),
            is_orient_router: false,
        },
        Operation {
            name: "recall",
            description: "Recall memory facts for a path or symbol; budget=1 returns compact one-liners.",
            aliases: &[],
            mutability: Mutability::ReadOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Cheap,
            ref_ownership: RefOwnership::Query,
            cancellation: CancellationSemantics::Deadline,
            migration: MigrationStatus::Canonical,
            exposure: SurfaceExposure {
                fastmcp_tool: true,
                codemode_binding: Some("graph.recall"),
                codemode_meta: false,
            },
            args: args(
                json!({
                    "target": { "type": "string" },
                    "budget": { "type": "integer", "default": 1 },
                    "repo": { "type": "string", "default": "." },
                }),
                &["target"],
            ),
            results: ref_first_results(),

            error_kinds: read_errors(),
            is_orient_router: false,
        },
        Operation {
            name: "expand",
            description: "Expand g:, q:, or gz:// ref to exact bytes (RACC recovery path). Agent hint: expand the ref returned by orient/locate/search/snap/blast to see the actual content.",
            aliases: &[],
            mutability: Mutability::ReadOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Cheap,
            ref_ownership: RefOwnership::Multi,
            cancellation: CancellationSemantics::Deadline,
            migration: MigrationStatus::Canonical,
            exposure: SurfaceExposure {
                fastmcp_tool: true,
                codemode_binding: Some("graph.expand"),
                codemode_meta: false,
            },
            args: args(
                json!({
                    "reference": { "type": "string", "description": "Single gz:// ref" },
                    "references": { "type": "array", "items": { "type": "string" }, "description": "Batch gz:// refs" },
                    "repo": { "type": "string", "default": "." },
                }),
                &["reference"],
            ),
            results: ref_first_results(),

            error_kinds: read_errors(),
            is_orient_router: false,
        },
        Operation {
            name: "index",
            description: "Index repository into .graphzero/. Agent hint: re-index on a fresh clone or after major repo changes so downstream tools have current data.",
            aliases: &[],
            mutability: Mutability::StoreOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Heavy,
            ref_ownership: RefOwnership::None,
            cancellation: CancellationSemantics::Cooperative,
            migration: MigrationStatus::Canonical,
            exposure: SurfaceExposure {
                fastmcp_tool: true,
                codemode_binding: Some("graph.index"),
                codemode_meta: false,
            },
            args: args(json!({ "path": { "type": "string", "default": "." } }), &[]),
            results: default_results(),

            error_kinds: mutate_errors(),
            is_orient_router: false,
        },
        Operation {
            name: "blast",
            description: "Intent blast radius; budget=1 returns q:<id> compact ref. Canonical MCP/CLI name is blast. blast_intent is a documented alias (same handler), not a lean FastMCP catalog tool; removal requires a major version after clients migrate. Agent hint: run blast when you need to scope intent and gather candidate queries before editing.",
            aliases: &["blast_intent"],
            mutability: Mutability::ReadOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Heavy,
            ref_ownership: RefOwnership::Query,
            cancellation: CancellationSemantics::Deadline,
            migration: MigrationStatus::Canonical,
            exposure: SurfaceExposure {
                fastmcp_tool: true,
                codemode_binding: Some("graph.blast"),
                codemode_meta: false,
            },
            args: args(
                json!({
                    "intent": { "type": "string" },
                    "budget": { "type": "integer", "default": 1 },
                    "depth": { "type": "integer", "default": 4, "description": "Maximum reverse-dependency hops for blast traversal" },
                    "repo": { "type": "string", "default": "." },
                }),
                &["intent"],
            ),
            results: ref_first_results(),

            error_kinds: read_errors(),
            is_orient_router: false,
        },
        Operation {
            name: "reserve",
            description: "Agent edit reservation: action declare|check|release|list. Agent hint: declare before editing, check/reserve while working, and release when you finish.",
            aliases: &[],
            mutability: Mutability::StoreOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Cheap,
            ref_ownership: RefOwnership::None,
            cancellation: CancellationSemantics::Cooperative,
            migration: MigrationStatus::Canonical,
            exposure: SurfaceExposure {
                fastmcp_tool: true,
                codemode_binding: Some("graph.reserve"),
                codemode_meta: false,
            },
            args: args(
                json!({
                    "action": { "type": "string", "description": "declare|check|release|list" },
                    "agent_id": { "type": "string", "description": "Reserving agent id; alias: `agent`" },
                    "agent": { "type": "string", "description": "Alias for agent_id" },
                    "intent_ops": { "type": "array" },
                    "reservation_id": { "type": "string" },
                    "acquire": { "type": "boolean" },
                    "ttl_seconds": { "type": "integer" },
                    "repo": { "type": "string", "default": "." },
                }),
                &["action"],
            ),
            results: default_results(),

            error_kinds: mutate_errors(),
            is_orient_router: false,
        },
        Operation {
            name: "verify",
            description: "Verify a post-edit graph claim (no_remaining_callers, no_outgoing_calls, no_remaining_references, no_remaining_dependencies, symbol_removed). Agent hint: verify after edits that touch the graph to confirm the claim holds before reporting success.",
            aliases: &["verify_claim"],
            mutability: Mutability::ReadOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Medium,
            ref_ownership: RefOwnership::Query,
            cancellation: CancellationSemantics::Deadline,
            migration: MigrationStatus::Canonical,
            exposure: SurfaceExposure {
                fastmcp_tool: true,
                codemode_binding: Some("graph.verify"),
                codemode_meta: false,
            },
            args: args(
                json!({
                    "claim": {
                        "type": "string",
                        "description": "no_remaining_callers | no_outgoing_calls | no_remaining_references | no_remaining_dependencies | symbol_removed",
                        "default": "no_remaining_callers"
                    },
                    "target": { "type": "string" },
                    "repo": { "type": "string", "default": "." },
                }),
                &["target"],
            ),
            results: ref_first_results(),

            error_kinds: read_errors(),
            is_orient_router: false,
        },
        // ── CodeMode-primary domain ops (not separate FastMCP tools) ──
        Operation {
            name: "query",
            description: "Run a QuerySurface request and store the full response behind a gz://query ref.",
            aliases: &["graph.query"],
            mutability: Mutability::ReadOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Medium,
            ref_ownership: RefOwnership::Query,
            cancellation: CancellationSemantics::Deadline,
            migration: MigrationStatus::Canonical,
            exposure: SurfaceExposure {
                fastmcp_tool: false,
                codemode_binding: Some("graph.query"),
                codemode_meta: false,
            },
            args: args(
                json!({
                    "surface": { "type": "string" },
                    "target": { "type": "string" },
                    "query": { "type": "string" },
                    "budget": { "type": "integer", "default": 1 },
                }),
                &["surface"],
            ),
            results: ref_first_results(),

            error_kinds: read_errors(),
            is_orient_router: false,
        },
        Operation {
            name: "multi_query",
            description: "Batch read-only QuerySurface requests; one physical op for many logical requests.",
            aliases: &["graph.multiQuery"],
            mutability: Mutability::ReadOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Medium,
            ref_ownership: RefOwnership::Query,
            cancellation: CancellationSemantics::Deadline,
            migration: MigrationStatus::Canonical,
            exposure: SurfaceExposure {
                fastmcp_tool: false,
                codemode_binding: Some("graph.multiQuery"),
                codemode_meta: false,
            },
            args: args(
                json!({
                    "surface": { "type": "string" },
                    "targets": { "type": "array", "items": { "type": "string" } },
                }),
                &["surface", "targets"],
            ),
            results: ref_first_results(),

            error_kinds: read_errors(),
            is_orient_router: false,
        },
        Operation {
            name: "defs",
            description: "Alias for query(surface=symbol). Prefer orient/query with surface=symbol in new code.",
            aliases: &["graph.defs"],
            mutability: Mutability::ReadOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Medium,
            ref_ownership: RefOwnership::Query,
            cancellation: CancellationSemantics::Deadline,
            migration: MigrationStatus::LegacyAlias,
            exposure: SurfaceExposure {
                fastmcp_tool: false,
                codemode_binding: Some("graph.defs"),
                codemode_meta: false,
            },
            args: args(json!({ "target": { "type": "string" } }), &["target"]),
            results: default_results(),

            error_kinds: read_errors(),
            is_orient_router: false,
        },
        Operation {
            name: "callers",
            description: "Alias for query(surface=callers). Prefer orient/query with surface=callers in new code.",
            aliases: &["graph.callers"],
            mutability: Mutability::ReadOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Medium,
            ref_ownership: RefOwnership::Query,
            cancellation: CancellationSemantics::Deadline,
            migration: MigrationStatus::LegacyAlias,
            exposure: SurfaceExposure {
                fastmcp_tool: false,
                codemode_binding: Some("graph.callers"),
                codemode_meta: false,
            },
            args: args(json!({ "target": { "type": "string" } }), &["target"]),
            results: default_results(),

            error_kinds: read_errors(),
            is_orient_router: false,
        },
        Operation {
            name: "ctx_ref",
            description: "Store arbitrary JSON bytes in GraphZero's blob spine and return gz://blob/<sha256>.",
            aliases: &["ctx.ref", "ref"],
            mutability: Mutability::StoreOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Cheap,
            ref_ownership: RefOwnership::Blob,
            cancellation: CancellationSemantics::None,
            migration: MigrationStatus::Canonical,
            exposure: SurfaceExposure {
                fastmcp_tool: false,
                codemode_binding: Some("ctx.ref"),
                codemode_meta: false,
            },
            args: args(json!({ "value": {} }), &["value"]),
            results: default_results(),

            error_kinds: mutate_errors(),
            is_orient_router: false,
        },
        Operation {
            name: "ctx_step",
            description: "Named plan step helper for CodeMode telemetry; not a graph engine call.",
            aliases: &["ctx.step"],
            mutability: Mutability::ReadOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Cheap,
            ref_ownership: RefOwnership::None,
            cancellation: CancellationSemantics::None,
            migration: MigrationStatus::Canonical,
            exposure: SurfaceExposure {
                fastmcp_tool: false,
                codemode_binding: Some("ctx.step"),
                codemode_meta: false,
            },
            args: args(
                json!({
                    "name": { "type": "string" },
                    "fn": { "type": "string", "description": "guest callback (JS only)" },
                }),
                &["name", "fn"],
            ),
            results: default_results(),

            error_kinds: &[DomainErrorKind::Validation, DomainErrorKind::Runtime],
            is_orient_router: false,
        },
        // ── CodeMode meta surface (mutually exclusive with lean FastMCP) ──
        Operation {
            name: "execute_code",
            description: "Execute a native GraphZero recipe or JSON-DAG plan; JavaScript plans require the aggregate zerostack-codemode-host or zsx and return bounded JSON plus durable refs.",
            aliases: &["gz_execute_code"],
            mutability: Mutability::StoreOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Heavy,
            ref_ownership: RefOwnership::Execution,
            cancellation: CancellationSemantics::Deadline,
            migration: MigrationStatus::Canonical,
            exposure: SurfaceExposure {
                fastmcp_tool: false,
                codemode_binding: None,
                codemode_meta: true,
            },
            args: args(
                json!({
                    "plan": { "type": "string", "maxLength": 65536 },
                    "form": { "type": "string", "enum": ["recipe", "json", "js", "auto"], "default": "auto" },
                    "limits": { "type": "object", "additionalProperties": { "type": "integer", "minimum": 0 } },
                    "repo": { "type": "string", "default": "." }
                }),
                &["plan"],
            ),
            results: default_results(),

            error_kinds: &[
                DomainErrorKind::Validation,
                DomainErrorKind::Policy,
                DomainErrorKind::Sandbox,
                DomainErrorKind::Runtime,
                DomainErrorKind::Substrate,
                DomainErrorKind::Busy,
                DomainErrorKind::DeadlineExceeded,
                DomainErrorKind::Cancelled,
            ],
            is_orient_router: false,
        },
        Operation {
            name: "codemode_search",
            description: "Discover GraphZero CodeMode methods, recipes, examples, safety metadata, and limits.",
            aliases: &["gz_codemode_search"],
            mutability: Mutability::ReadOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Cheap,
            ref_ownership: RefOwnership::None,
            cancellation: CancellationSemantics::None,
            migration: MigrationStatus::Canonical,
            exposure: SurfaceExposure {
                fastmcp_tool: false,
                codemode_binding: None,
                codemode_meta: true,
            },
            args: args(
                json!({
                    "query": { "type": "string", "minLength": 1 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                }),
                &["query"],
            ),
            results: default_results(),

            error_kinds: &[DomainErrorKind::Validation],
            is_orient_router: false,
        },
        Operation {
            name: "codemode_describe",
            description: "Describe a GraphZero CodeMode method, recipe, binding, limits, or capabilities manifest.",
            aliases: &["gz_codemode_describe"],
            mutability: Mutability::ReadOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Cheap,
            ref_ownership: RefOwnership::None,
            cancellation: CancellationSemantics::None,
            migration: MigrationStatus::Canonical,
            exposure: SurfaceExposure {
                fastmcp_tool: false,
                codemode_binding: None,
                codemode_meta: true,
            },
            args: args(
                json!({ "name": { "type": "string", "minLength": 1 } }),
                &["name"],
            ),
            results: default_results(),

            error_kinds: &[DomainErrorKind::Validation, DomainErrorKind::NotFound],
            is_orient_router: false,
        },
    ];

    // Orient sub-surfaces: exact SURFACE_NAMES tokens under `orient.<surface>`
    // so inventory stays unique while mapping is explicit (search/recall also
    // exist as top-level FastMCP tools).
    for surface in crate::SURFACE_NAMES {
        let name_static: &'static str = Box::leak(format!("orient.{surface}").into_boxed_str());
        let desc_static: &'static str = Box::leak(
            format!("Orient sub-surface `{surface}` routed through orient/query.").into_boxed_str(),
        );
        let surface_static: &'static str = *surface;
        let aliases: &'static [&'static str] = if *surface == "reading_set" {
            &["reading-set", "readingset"]
        } else {
            &[]
        };
        ops.push(Operation {
            name: name_static,
            description: desc_static,
            aliases,
            mutability: Mutability::ReadOnly,
            capability: CapabilityRequirement::Public,
            cost_class: CostClass::Medium,
            ref_ownership: RefOwnership::Query,
            cancellation: CancellationSemantics::Deadline,
            migration: MigrationStatus::OrientSubSurface,
            exposure: SurfaceExposure {
                fastmcp_tool: false,
                codemode_binding: None,
                codemode_meta: false,
            },
            args: args(
                json!({
                    "surface": { "type": "string", "const": surface_static },
                    "query": { "type": "string" },
                    "target": { "type": "string" },
                    "budget": { "type": "integer", "default": 1 },
                }),
                &[],
            ),
            results: default_results(),

            error_kinds: read_errors(),
            is_orient_router: false,
        });
    }

    ops.sort_by(|a, b| a.name.cmp(b.name));
    ops
}
