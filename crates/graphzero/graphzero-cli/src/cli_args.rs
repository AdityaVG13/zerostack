use std::ffi::OsStr;
use std::path::PathBuf;

use clap::{ColorChoice, CommandFactory, Parser, Subcommand, ValueEnum};
use serde_json::json;

use crate::pack_cmd;

/// Clap color policy: honor NO_COLOR (and CI). Does not affect GRAPHZERO_JSON.
pub(crate) fn color_choice_from_env() -> ColorChoice {
    if std::env::var_os("NO_COLOR").is_some() {
        return ColorChoice::Never;
    }
    if matches!(std::env::var("CI"), Ok(v) if !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"))
    {
        return ColorChoice::Never;
    }
    ColorChoice::Auto
}

/// Command factory with NO_COLOR / CI color policy applied.
pub(crate) fn cli_command() -> clap::Command {
    Cli::command().color(color_choice_from_env())
}

#[derive(Parser)]
#[command(
    name = "graphzero",
    version,
    disable_version_flag = true,
    about = "AI-agent code graph (ref-first). Run with no args for JSON triage. Humans: use IDE/MCP harness."
)]
pub struct Cli {
    /// Wrap JSON read verbs in {schema_version, data, meta} (agents: stable parsing)
    #[arg(long, global = true)]
    pub(crate) json: bool,
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum ExportFormat {
    #[default]
    /// Tiny ref+meta (g:/q: + coverage) for agents/handoff/perf (default)
    Minimal,
    /// Full QueryCapsule (or Blast) JSON for audit
    Capsule,
    /// Structured MD handoff summary (human + agent readable)
    Md,
    /// zstd-compressed capsule for committed portable artifact (git friendly)
    Zst,
}

impl ExportFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExportFormat::Minimal => "minimal",
            ExportFormat::Capsule => "capsule",
            ExportFormat::Md => "md",
            ExportFormat::Zst => "zst",
        }
    }
}

#[derive(Clone, ValueEnum)]
pub(crate) enum DaemonAction {
    Enable,
    Disable,
    Status,
    #[value(name = "run", hide = true)]
    Run,
}

#[derive(Subcommand)]
pub(crate) enum ZerorefFixtureAction {
    /// Print the ZeroRef v1 capability descriptor JSON
    Descriptor,
    /// Write bytes into the canonical CAS; print fixture JSON on stdout
    Put {
        /// Isolated project store root
        #[arg(long)]
        store_root: PathBuf,
        /// Explicit shared CAS root (three-binary matrix interop)
        #[arg(long)]
        shared_root: Option<PathBuf>,
        /// Input file; reads stdin when absent
        #[arg(long)]
        input: Option<PathBuf>,
        /// Stricter CAS size policy for conformance runs
        #[arg(long)]
        max_object_bytes: Option<u64>,
    },
    /// Expand a ZeroRef v1 ref: exact bytes on stdout (or --out), diagnostics JSON on stderr
    Expand {
        /// Isolated project store root
        #[arg(long)]
        store_root: PathBuf,
        /// Explicit shared CAS root (three-binary matrix interop)
        #[arg(long)]
        shared_root: Option<PathBuf>,
        /// Full ZeroRef v1 blob ref, optionally with #B/#L fragment
        #[arg(long = "ref")]
        reference: String,
        /// Write bytes to this file instead of stdout
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub(crate) enum RobotDocsAction {
    /// Print agent quick guide (stdout)
    Guide,
}

#[derive(Subcommand)]
pub(crate) enum TelemetryCommand {
    /// Dry-run the exact shareable telemetry payload (sends nothing; exporter=none)
    Inspect {
        /// Explicitly opt in to shareable telemetry inspection permission
        #[arg(long)]
        telemetry: bool,
        /// Explicitly opt out; takes precedence over --telemetry and config/env
        #[arg(long)]
        no_telemetry: bool,
        /// Repository root (resolves store: `.graphzero/` or `.zerostack/graphzero/` + optional config.json)
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// ZeroRef v1 conformance fixtures for the three-binary matrix (agents/CI)
    #[command(name = "zeroref-fixture")]
    ZerorefFixture {
        #[command(subcommand)]
        action: ZerorefFixtureAction,
    },
    /// Index a repository into the resolved GraphZero store
    /// (`<repo>/.graphzero`, or `<repo>/.zerostack/graphzero` when a unified root is present)
    Index {
        /// Repository root (defaults to current directory)
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Alias for path (agent-friendly)
        #[arg(long, hide = true)]
        repo: Option<PathBuf>,
    },
    /// Query a symbol; returns a JSON capsule with gz:// evidence refs
    /// Use --export / --to-file for durable portable artifact (atomic, perf minimal).
    Snap {
        symbol: String,
        /// Token budget for the visible part of the capsule
        #[arg(long, default_value_t = 1)]
        budget: usize,
        /// Repository root (defaults to current directory)
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Write export artifact atomically here (supports minimal/capsule/md/zst).
        /// If given, still emits tiny meta on stdout (exported+ref+size).
        #[arg(long = "export", alias = "to-file", value_name = "PATH")]
        export: Option<PathBuf>,
        /// Export format (perf default minimal for agents).
        #[arg(long, value_enum, default_value_t = ExportFormat::Minimal)]
        format: ExportFormat,
    },
    /// Persist a decision-memory fact (gz://mem/<id>)
    #[command(
        long_about = "Persist a decision-memory fact and return gz://mem/<id>.\n\n\
Path anchors: pass repo-relative paths via --anchor (there is no --path flag).\n\
Anchors may also be symbol names. Repeat --anchor or use comma-separated values.\n\n\
Examples:\n  \
graphzero remember --text 'prefer refs' --anchor src/lib.rs\n  \
graphzero remember --text 'prefer refs' --anchor MySymbol --anchor src/main.rs"
    )]
    Remember {
        /// Fact text (max 500 chars)
        #[arg(long)]
        text: String,
        /// Symbol name or repo-relative path (path anchors use --anchor; there is no --path)
        #[arg(
            long = "anchor",
            alias = "anchors",
            value_delimiter = ',',
            value_name = "ANCHOR",
            help = "Symbol name or repo-relative path anchor (no --path; paths go through --anchor)"
        )]
        anchors: Vec<String>,
        #[arg(long)]
        kind: Option<String>,
        /// Prior memory facts superseded by this fact (ids or gz://mem/<id>, comma-separated)
        #[arg(long, value_delimiter = ',')]
        supersedes: Vec<String>,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Recall facts for a symbol, memory ref, or path anchor
    #[command(long_about = "Recall decision-memory facts for TARGET.\n\n\
TARGET forms:\n  \
- symbol name (e.g. MySymbol)\n  \
- gz://mem/<id> memory ref\n  \
- repo-relative path anchor (e.g. src/lib.rs)\n\n\
Examples:\n  \
graphzero recall MySymbol --budget 1\n  \
graphzero recall gz://mem/<id>\n  \
graphzero recall src/lib.rs")]
    Recall {
        /// symbol name | gz://mem/<id> | repo-relative path anchor
        #[arg(value_name = "TARGET")]
        target: String,
        #[arg(long, default_value_t = 1)]
        budget: usize,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Expand a gz:// ref to its exact bytes
    Expand {
        reference: String,
        /// Repository root (defaults to current directory)
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Opt-in warm daemon control
    Daemon {
        #[arg(value_enum)]
        action: DaemonAction,
        /// Repository root for enable/disable/status/run
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Required for `enable`: spawn detached warm stem (stdout/stderr discarded).
        /// Without --yes, enable prints a preview and refuses to mutate.
        #[arg(long)]
        yes: bool,
    },
    #[command(
        long_about = "Serve runs the MCP stdio server. Stdout is reserved for JSON-RPC only and must not be piped into other commands, while stderr carries diagnostics. Do not run it interactively in a shell pipeline that shares stdout."
    )]
    /// MCP server over stdio (Model Context Protocol)
    Serve,
    /// Compact the delta log into a new snapshot
    Compact {
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Index and store statistics
    Stats {
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Legacy multi-surface entry (agents: prefer orient / search)
    #[command(name = "query-surface", hide = true)]
    QuerySurface {
        /// Tool name: symbol, callers, deps, outline, context, hot, changes, word, search, callpath
        surface: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        query: Option<String>,
        #[arg(long)]
        path: Option<String>,
        #[arg(long, default_value_t = 1)]
        budget: usize,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Tier-B SCIP ingest
    Ingest {
        #[command(subcommand)]
        action: IngestCommand,
    },
    /// why-graph passive ingestion diagnostics
    Why {
        #[command(subcommand)]
        action: WhyCommand,
    },
    /// Multi-hop call/import neighborhood for retrieval chains
    Neighborhood {
        /// Seed symbol. Repeat for multiple retrieval roots.
        #[arg(long = "seed", required = true)]
        seed: Vec<String>,
        /// Maximum call/import hops to traverse.
        #[arg(long = "hops", default_value_t = 2)]
        hops: u32,
        /// Maximum edges to return.
        #[arg(long, default_value_t = 50)]
        budget: usize,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Intent-level blast radius (break sites, tests, silent risk)
    Blast {
        #[arg(long)]
        intent: String,
        #[arg(long, default_value_t = 1)]
        budget: usize,
        /// Maximum reverse dependency hops for blast traversal.
        #[arg(long = "depth", default_value_t = 4)]
        depth: u32,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Speculative world identifier for impact-before-edit output.
        #[arg(long = "world-ref")]
        world_ref: Option<String>,
        /// FSZero world-ref v1 enumeration envelope (JSON text). Strictly
        /// validated before graph work; unknown major versions and mismatched
        /// world refs fail loudly.
        #[arg(long = "world-envelope")]
        world_envelope: Option<String>,
        /// Planned edit as path::before=>after. Repeat to describe the speculative world delta.
        #[arg(long = "planned-edit", value_name = "PATH::BEFORE=>AFTER")]
        planned_edit: Vec<String>,
        /// Focus symbol for speculative output. Defaults to the parsed blast target.
        #[arg(long = "focus")]
        focus: Vec<String>,
        /// Write export artifact atomically here (supports minimal/capsule/md/zst). If given, still emits tiny meta on stdout.
        #[arg(long = "export", alias = "to-file", value_name = "PATH")]
        export: Option<PathBuf>,
        /// Export format (perf default minimal for agents).
        #[arg(long, value_enum, default_value_t = ExportFormat::Minimal)]
        format: ExportFormat,
    },
    /// Publish third-party evidence-backed edges
    Publish {
        /// JSON batch file (publish/v1)
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        capability: Option<String>,
        /// DANGEROUS: allow anonymous publish without a capability token.
        /// Explicit opt-in only; never the default. Prefer a signed capability.
        #[arg(long)]
        unsafe_allow_anonymous_publish: bool,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },

    /// Semantic reservations (swarm coordination)
    Reserve {
        #[command(subcommand)]
        action: ReserveCommand,
        /// Repository root
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Dependency shard packs
    Pack {
        #[command(subcommand)]
        action: pack_cmd::PackCommand,
    },

    /// Execute GraphZero CodeMode recipe, JSON DAG, or sandboxed JS plan
    #[command(name = "code-mode", alias = "codemode", alias = "code_mode")]
    CodeMode {
        /// Plan text (recipe, JSON, or JavaScript code)
        plan: String,
        /// Repository root (defaults to current directory)
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Discover GraphZero CodeMode methods, recipes, examples, and limits
    #[command(
        name = "code-mode-search",
        alias = "codemode-search",
        alias = "code_mode_search"
    )]
    CodeModeSearch {
        /// Search query
        #[arg(default_value = "")]
        query: String,
    },
    /// Describe a GraphZero CodeMode method, recipe, or limit
    #[command(
        name = "code-mode-describe",
        alias = "codemode-describe",
        alias = "code_mode_describe"
    )]
    CodeModeDescribe {
        /// Name to describe, e.g. graph.query, graph.multiQuery, ctx.ref, limits
        name: String,
    },
    /// Machine-readable agent contract (JSON on stdout)
    Capabilities,
    /// Agent-oriented documentation
    RobotDocs {
        #[command(subcommand)]
        action: Option<RobotDocsAction>,
    },
    /// One-shot agent orientation (JSON)
    AgentTriage {
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Diagnose CLI + workspace health (JSON)
    Doctor {
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Install one package surface (mutually exclusive mcp|codemode)
    Install {
        /// Package surface: mcp or codemode
        #[arg(long)]
        surface: String,
        /// Install state prefix (default: $GRAPHZERO_INSTALL_PREFIX or ~/.graphzero-install)
        #[arg(long)]
        prefix: Option<PathBuf>,
        /// Path to the surface binary being registered
        #[arg(long)]
        binary: Option<PathBuf>,
        /// Preview planned install writes without mutating the prefix
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove install state, client config, and shim marker
    Uninstall {
        #[arg(long)]
        prefix: Option<PathBuf>,
        /// Preview removals without deleting anything
        #[arg(long)]
        dry_run: bool,
    },
    /// Print SBOM / package identity JSON for a surface
    Sbom {
        #[arg(long)]
        surface: Option<String>,
    },
    /// Shareable telemetry permission (default off; inspect dry-run only; no exporter)
    Telemetry {
        #[command(subcommand)]
        action: TelemetryCommand,
    },
    /// Alias: query-surface (default surface matches MCP/ABI: context)
    Orient {
        #[arg(long, default_value = "context")]
        surface: String,
        #[arg(long)]
        name: Option<String>,
        /// Positional symbol alias (`graphzero orient main`)
        #[arg(value_name = "NAME")]
        positional_name: Option<String>,
        #[arg(long)]
        query: Option<String>,
        #[arg(long)]
        path: Option<String>,
        #[arg(long, default_value_t = 1)]
        budget: usize,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Write export artifact atomically here (supports minimal/capsule/md/zst).
        #[arg(long = "export", alias = "to-file", value_name = "PATH")]
        export: Option<PathBuf>,
        /// Export format (perf default minimal for agents).
        #[arg(long, value_enum, default_value_t = ExportFormat::Minimal)]
        format: ExportFormat,
    },
    /// Symbol surface alias (graphzero symbol; orient remains the multi-surface entrypoint)
    Symbol {
        /// Symbol name to examine (required)
        #[arg(long)]
        name: String,
        /// Token budget for the visible part of the capsule
        #[arg(long, default_value_t = 1)]
        budget: usize,
        /// Repository root (defaults to current directory)
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Write export artifact atomically here (supports minimal/capsule/md/zst).
        #[arg(long = "export", alias = "to-file", value_name = "PATH")]
        export: Option<PathBuf>,
        /// Export format (perf default minimal for agents).
        #[arg(long, value_enum, default_value_t = ExportFormat::Minimal)]
        format: ExportFormat,
    },
    /// Alias: query-surface search (NL query; positional `search GraphStore` accepted)
    Search {
        /// Search needle (`--query` form)
        #[arg(long)]
        query: Option<String>,
        /// Positional query alias (`graphzero search GraphStore`)
        #[arg(value_name = "QUERY")]
        positional_query: Option<String>,
        #[arg(long, default_value_t = 1)]
        budget: usize,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Write export artifact atomically here (supports minimal/capsule/md/zst).
        #[arg(long = "export", alias = "to-file", value_name = "PATH")]
        export: Option<PathBuf>,
        /// Export format (perf default minimal for agents).
        #[arg(long, value_enum, default_value_t = ExportFormat::Minimal)]
        format: ExportFormat,
    },
    /// Verify post-edit agent claim
    #[command(long_about = "Verify a post-edit claim against the indexed graph.\n\n\
Allowed --claim values (ClaimKind):\n  \
no_remaining_callers | no_outgoing_calls | no_remaining_references | \
no_remaining_dependencies | symbol_removed\n\n\
Example:\n  \
graphzero verify --claim no_remaining_callers MySymbol --repo .")]
    Verify {
        /// Symbol (or target) the claim asserts about
        target: Option<String>,
        /// Claim kind (invalid values still error via parse_claim_kind)
        #[arg(
            long,
            required_unless_present = "claims_file",
            value_name = "CLAIM",
            help = "Claim kind: no_remaining_callers | no_outgoing_calls | no_remaining_references | no_remaining_dependencies | symbol_removed (ex: --claim no_remaining_callers MySymbol)"
        )]
        claim: Option<String>,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// PR body / claims file containing GraphZero-Claim: <claim> <target> lines
        #[arg(long = "claims-file", value_name = "PATH")]
        claims_file: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub(crate) enum IngestCommand {
    /// Merge a SCIP index file into the snapshot store
    Scip {
        scip_file: PathBuf,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
}

#[derive(Subcommand)]
pub(crate) enum ReserveCommand {
    /// Declare intent and footprint
    Declare {
        #[arg(long = "agent", alias = "agent-id")]
        agent: String,
        /// Intent JSON file path OR inline intent text (e.g. "change fn foo")
        #[arg(long)]
        intent: String,
        #[arg(long, default_value_t = 3600)]
        ttl: u64,
        #[arg(long)]
        json: bool,
    },
    /// Check overlap (optional acquire)
    Check {
        #[arg(long = "agent", alias = "agent-id")]
        agent: String,
        /// Intent JSON file path OR inline intent text
        #[arg(long)]
        intent: String,
        #[arg(long)]
        acquire: bool,
        #[arg(long)]
        ttl: Option<u64>,
        #[arg(long)]
        json: bool,
    },
    /// Release reservation
    Release {
        #[arg(long = "agent", alias = "agent-id")]
        agent: String,
        #[arg(long)]
        reservation_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Query active reservations
    Query {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum WhyCommand {
    /// Ingest local why fixtures into the resolved GraphZero why store
    /// (`<repo>/.graphzero/why` or under `.zerostack/graphzero/why`)
    Ingest {
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        #[arg(long)]
        fixtures: Option<PathBuf>,
    },
    /// JSON status: counts, UNKNOWN connectors, redactions
    Status {
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Replay golden fixtures twice and report digest stability
    Replay {
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        #[arg(long)]
        fixtures: PathBuf,
    },
    /// Verify every persisted why edge expands
    EvidenceCheck {
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
}

pub(crate) fn version_requested() -> bool {
    let short = OsStr::new("-V");
    let long = OsStr::new("--version");
    std::env::args_os().any(|arg| arg == short || arg == long)
}

/// Map common agent typos to corrective hints (clap already suggests similar subcommands).
pub(crate) fn agent_typo_extra_hint(err: &str) -> Option<&'static str> {
    let lower = err.to_lowercase();
    if lower.contains("serach") {
        return Some("graphzero search --query <text>");
    }
    if lower.contains("orrient") {
        return Some("graphzero orient --surface symbol --name <symbol>");
    }
    None
}

pub fn agent_json_mode() -> bool {
    matches!(std::env::var("GRAPHZERO_JSON"), Ok(v) if v == "1" || v == "true")
        || matches!(std::env::var("GRAPHZERO_AGENT"), Ok(v) if v == "1" || v == "true")
        || std::env::args().any(|a| a == "--json")
}

pub(crate) fn json_version_mode() -> bool {
    matches!(std::env::var("GRAPHZERO_JSON"), Ok(val) if val == "1")
}

pub(crate) fn print_version(json: bool) {
    let cmd = cli_command();
    let name = cmd.get_name();
    let version = cmd.get_version().unwrap_or("0.0.0");
    if json {
        let info = json!({ "name": name, "version": version });
        println!("{info}");
    } else {
        println!("{name} {version}");
    }
}
