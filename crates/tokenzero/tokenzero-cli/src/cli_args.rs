use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

macro_rules! define_args {
    ($( $(#[$struct_attr:meta])* @ $name:ident($( $(#[$field_attr:meta])* $field:ident: $ty:ty;)*) )*) => {
        $(
            #[derive(Debug, Args)]
            $(#[$struct_attr])*
            pub(crate) struct $name {$($(#[$field_attr])* pub(crate) $field: $ty,)*}
        )*
    };
}

macro_rules! define_subcommands {
    ($(
        @ $name:ident {
            $($(#[$variant_doc:meta])* $variant:ident $(($arg:ty))? $(=> $variant_attr:meta)*),* $(,)?
        }
    )*) => {$(
        #[derive(Debug, Subcommand)]
        pub(crate) enum $name {$($(#[$variant_doc])* $(#[$variant_attr])* $variant $(($arg))?,)*}
    )*};
}

macro_rules! artifact_args {
    ($($name:ident => $default:literal),* $(,)?) => {
        $(
            #[derive(Debug, Args)]
            pub(crate) struct $name {
                #[arg(long, default_value = $default)]
                pub(crate) output_json: PathBuf,
                #[arg(long)]
                pub(crate) output_md: Option<PathBuf>,
                #[arg(long)]
                pub(crate) json: bool,
            }
        )*
    };
}

#[derive(Debug, Parser)]
#[command(
    name = "tokenzero",
    version,
    about = "Rust TokenZero RACC runtime",
    after_help = "Agent surfaces:\n  tokenzero capabilities --json   Print the machine-readable CLI contract\n  tokenzero robot-docs guide      Print a paste-ready guide for agents\n  tokenzero run --json -- <cmd>   Run commands with status-truth telemetry\n  tokenzero read <path> --json=full  Restore the full forensic ToolResponse envelope\n  tokenzero --robot-triage        One-shot health + findings + next command (doctor)"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,
}

#[derive(Debug, Clone, Subcommand)]
pub(crate) enum DoctorCommand {
    #[command(about = "Run all doctor checks. Read-only. Default when omitted.")]
    Diagnose,
    #[command(about = "Apply supported doctor fixers with backups and actions.jsonl")]
    Fix,
    #[command(about = "Undo a prior doctor fixer run")]
    Undo {
        #[arg(value_name = "RUN_ID")]
        run_id: String,
    },
    #[command(name = "ls", about = "List local doctor run artifacts")]
    Ls,
    #[command(about = "Expand a current or known doctor finding")]
    Explain {
        #[arg(value_name = "FINDING_ID")]
        finding_id: String,
    },
    #[command(about = "Print machine-readable doctor contract")]
    Capabilities,
    #[command(
        about = "Print cheap liveness summary",
        alias = "status",
        alias = "statuz"
    )]
    Health,
    #[command(
        name = "robot-docs",
        alias = "robotdocs",
        about = "Print paste-ready doctor handbook for agents"
    )]
    RobotDocs,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SessionLedgerCommand {
    #[command(
        about = "Print per-session token-turn cost breakdown; headline metric is DPMT (decisions per million visible token-turns)"
    )]
    Stats,
    #[command(about = "Export ledger as JSON (includes visible/raw token-turns and DPMT)")]
    Export,
    #[command(about = "Print the stable schema for the session ledger (session-ledger-v3)")]
    Schema,
    #[command(
        about = "Inspect default-off usage telemetry records ({execution_path, raw_tokens, spent_tokens}); sends nothing"
    )]
    Inspect(TelemetryInspectArgs),
    #[command(about = "Query the response ledger")]
    Query {
        #[command(subcommand)]
        query: LedgerQueryCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum LedgerQueryCommand {
    #[command(about = "Aggregate visible token cost for one repo over a time window")]
    Repo {
        #[arg(long)]
        repo: PathBuf,
        #[arg(long, default_value_t = 30)]
        days: u64,
    },
    #[command(
        name = "version-delta",
        about = "Compare visible token cost between crate versions"
    )]
    VersionDelta {
        #[arg(long)]
        baseline: String,
        #[arg(long)]
        candidate: String,
        #[arg(long, default_value_t = 30)]
        days: u64,
    },
    #[command(
        name = "agent-spend",
        about = "Aggregate visible token cost by agent identity"
    )]
    AgentSpend {
        #[arg(long, default_value_t = 30)]
        days: u64,
    },
    #[command(
        name = "task-cost",
        about = "Group response ledger v2 by task/session and write CSV plus JSON"
    )]
    TaskCost {
        #[arg(long, value_name = "PATH")]
        json_out: PathBuf,
        #[arg(long, value_name = "PATH")]
        csv_out: PathBuf,
    },
}

define_subcommands! {
    @ Commands { Read(ReadArgs) => command(about = "Read bounded file content with exact recovery refs"), Find(FindArgs) => command( about = "Search local text and return compact matches", visible_alias = "search" ), Grep(FindArgs) => command(about = "Grep-style search: regex under the ripgrep backend, literal otherwise (use find for literal-only)"), Glob(GlobArgs) => command(about = "List matching paths without dumping file contents"), Tree(TreeArgs) => command(about = "Inspect a bounded directory tree"), Edit(EditArgs) => command(about = "Apply multi-hunk find/replace edits to one file with undo refs"), Recall(RecallArgs) => command(about = "Search payloads already stored in the recovery cache"), Fetch(FetchArgs) => command(about = "Fetch an http(s) URL via curl with a TTL cache and exact refs"), Run(RunArgs) => command( alias = "shell", alias = "rn", about = "Run a command with status-truth telemetry" ), Ingest(IngestArgs) => command(about = "Ingest text or a file into a compact TokenZero capsule"), Expand(ExpandArgs) => command(about = "Recover exact bytes from one or more prior TokenZero refs"), SessionOpen(CommonArgs) => command(name = "session-open", about = "Open a bounded manifest+delta session", hide = true), Mem(CommonArgs) => command(about = "Inspect recovery-cache state"), Rewrite(RewriteArgs) => command(about = "Rewrite a shell command with TokenZero-safe routing", hide = true) => command(alias = "rewrite-command"), Hook(HookArgs) => command(about = "Agent-harness hook adapters: stdin JSON in, decision JSON out", hide = true), Discover(CommonArgs) => command(about = "List local TokenZero tool-discovery metadata"), Doctor(DoctorArgs) => command(about = "Check local TokenZero health and next steps"), Stats(StatsArgs) => command(about = "Print local TokenZero usage statistics", hide = true), Pulse(PulseArgs) => command(about = "Inspect or sync local Pulse telemetry"), SessionLedger(SessionLedgerArgs) => command( about = "Session cost ledger: token-turns (mass × turns_remaining); headline DPMT", alias = "ledger" , hide = true), Cache(CacheArgs) => command(about = "Inspect or prune TokenZero recovery-cache state"), Install(InstallArgs) => command(about = "Plan or apply local integration writes with rollback data"), Init(InitArgs) => command(about = "Compatibility alias for install --mcp --agent <name>", hide = true), Clients(ClientsArgs) => command( about = "Inspect AI client TokenZero integration state", alias = "client" , hide = true), ClientStatus(ClientStatusArgs) => command(name = "client-status", about = "Alias for clients detect", hide = true), Capabilities(CapabilitiesArgs) => command( about = "Print the machine-readable CLI contract for agents", alias = "capability", alias = "capabilites" ), RobotDocs(RobotDocsArgs) => command( name = "robot-docs", about = "Print in-tool documentation for agents", alias = "robot-doc", alias = "robotdocs" ), CachePack(CachePackArgs) => command(name = "cache-pack", about = "Build a daemonless prompt-cache pack with stable prefix and volatile refs", hide = true), Bench(BenchArgs) => command(about = "Run benchmark suites (eval artifact; not agent-primary)", hide = true), McpServer(McpServerArgs) => command(name = "mcp-server", about = "Run the explicit classic MCP compatibility server over stdio", hide = true), McpSmoke(McpSmokeArgs) => command(name = "mcp-smoke", about = "Smoke-test the MCP server (eval artifact)", hide = true), McpSoak(McpSoakArgs) => command(name = "mcp-soak", about = "Soak-test the MCP server (eval artifact)", hide = true), ExactRecoveryShell(ExactRecoveryShellArgs) => command(name = "exact-recovery-shell", about = "Audit exact byte recovery over shell corpora (eval artifact)", hide = true), ExactRecoveryAudit(ExactRecoveryAuditArgs) => command(name = "exact-recovery-audit", about = "Audit exact byte recovery guarantees (eval artifact)", hide = true), HarmEval(HarmEvalArgs) => command(name = "harm-eval", about = "Run the harm evaluation harness (eval artifact)", hide = true), ProtectedAnchorAudit(ProtectedAnchorAuditArgs) => command(name = "protected-anchor-audit", about = "Audit protected-anchor preservation (eval artifact)", hide = true), FalseSuccessShell(FalseSuccessShellArgs) => command(name = "false-success-shell", about = "Audit false-success detection on shell output (eval artifact)", hide = true), RepoInventory(RepoInventoryArgs) => command(name = "repo-inventory", about = "Inventory repository surfaces for audits (eval artifact)", hide = true), PromptCachePack(PromptCachePackArgs) => command(name = "prompt-cache-pack", about = "Build a prompt-cache pack artifact (eval)", hide = true), InstallSmoke(InstallSmokeArgs) => command(name = "install-smoke", about = "Plan an install probe in a disposable temporary root; --apply runs apply plus rollback there", hide = true), PackageAudit(PackageAuditArgs) => command(name = "package-audit", about = "Audit release packaging (eval artifact)", hide = true), ShellMatrix(ShellMatrixArgs) => command(name = "shell-matrix", about = "Audit shell compatibility matrix (eval artifact)", hide = true), OsReachAudit(OsReachAuditArgs) => command(name = "os-reach-audit", about = "Audit OS reach/portability claims (eval artifact)", hide = true), OsReleaseArtifact(OsReleaseArtifactArgs) => command(name = "os-release-artifact", about = "Produce OS release artifacts (eval)", hide = true), OneShotEval(OneShotEvalArgs) => command(name = "one-shot-eval", about = "Run one-shot evaluation harness (eval artifact)", hide = true), SourceCurrencyAudit(SourceCurrencyAuditArgs) => command(name = "source-currency-audit", about = "Audit source freshness/currency (eval artifact)", hide = true), AdapterApprovalAudit(AdapterApprovalAuditArgs) => command(name = "adapter-approval-audit", about = "Audit adapter approval state (eval artifact)", hide = true), AdapterApprovalTemplate(AdapterApprovalTemplateArgs) => command(name = "adapter-approval-template", about = "Emit adapter approval template (eval artifact)", hide = true), ClaimAudit(ClaimAuditArgs) => command(name = "claim-audit", about = "Audit marketing/claim evidence against artifacts (eval artifact)", hide = true), CompletionAudit(CompletionAuditArgs) => command(name = "completion-audit", about = "Audit task completion claims (eval artifact)", hide = true), SecurityPrivacyAudit(SecurityPrivacyAuditArgs) => command(name = "security-privacy-audit", about = "Audit security/privacy posture (eval artifact)", hide = true), ArtifactHandoff(ArtifactHandoffArgs) => command(name = "artifact-handoff", about = "Package artifacts for handoff (eval)", hide = true), Reach(ReachArgs) => command(about = "Audit repository reach/coverage (eval artifact)", hide = true), WsSkeleton(WsSkeletonArgs) => command(name = "ws-skeleton", about = "Emit workspace skeleton scaffold (eval)", hide = true), Quote(QuoteArgs) => command(about = "Quote shell arguments safely for the current platform", hide = true), }
    @ HookTarget { ClaudeCode(HookClaudeCodeArgs) => command( name = "claude-code", about = "Claude Code PreToolUse adapter: wraps Bash commands in `tokenzero run` (valid pass-through events exit 0; empty/invalid stdin exits 2)" ), ClaudeCodeSessionStart(HookSessionStartArgs) => command( name = "claude-code-session-start", about = "Claude Code SessionStart adapter: restores a compact session pack after compaction/resume (valid pass-through events exit 0; empty/invalid stdin exits 2)" ), }
    @ PulseCommand { Stats => command( name = "stats", alias = "status", about = "Print local Pulse telemetry report" ), Sync => command(about = "Reconcile the JSONL ledger into the SQLite query cache"), Doctor => command(about = "Check Pulse store markers, integrity, and hot index"), ExportJsonl(PulseExportArgs) => command(name = "export-jsonl", about = "Write an atomic JSONL snapshot from the reconciled SQLite cache"), ImportJsonl(PulseImportArgs) => command(name = "import-jsonl", about = "Validate a snapshot, replace the ledger, and rebuild SQLite"), }
    @ BenchCommand { Competitors(BenchCompetitorsArgs), }
    @ CacheCommand { Status(CommonArgs) => command(alias = "statuz"), Prune(CachePruneArgs), #[doc = "Migrate legacy short refs to full-hash canonical refs (dry-run by default)."] MigrateRefs(CacheMigrateRefsArgs), #[doc = "Verify migration integrity without mutating."] MigrateVerify(CacheMigrateVerifyArgs), #[doc = "Rollback migration aliases and manifest (never CAS/source bytes)."] MigrateRollback(CacheMigrateRollbackArgs), #[doc = "Clean up legacy source payloads after successful verification."] MigrateCleanup(CacheMigrateCleanupArgs), }
    @ ClientsCommand { Detect(ClientStatusArgs) => command(about = "Detect configured TokenZero AI client surfaces"), Scan(ClientStatusArgs) => command(about = "Scan this machine for AI harnesses TokenZero can adapt to"), Plan(ClientsPlanArgs) => command(about = "Plan TokenZero AI client integration writes"), Doctor(ClientStatusArgs) => command(about = "Diagnose TokenZero AI client integration state"), Rollback(ClientsRollbackArgs) => command(about = "Rollback a previous TokenZero client integration write"), }
    @ RobotDocsCommand { Guide => command(alias = "manual"), Commands => command(about = "Print canonical command quick reference for agents"), Examples => command(about = "Print copy-paste examples for common agent tasks"), }
}

define_args! {
    #[derive(Clone)] @ CommonArgs(#[arg(long)] root: Option<PathBuf>; #[arg(long)] cache_path: Option<PathBuf>; #[arg(long)] json: bool;)
    #[derive(Clone)] @ StatsArgs(#[arg(long)] root: Option<PathBuf>; #[arg(long)] cache_path: Option<PathBuf>; #[arg(long)] json: bool; #[arg(long)] cachezero: bool;)
    #[derive(Clone)] @ ToolArgs(#[arg(long, default_value = "auto")] mode: String; #[arg(long)] budget: Option<usize>; #[arg(long)] allowed_root: Vec<PathBuf>; #[arg(long)] cache_path: Option<PathBuf>; #[arg(long, alias = "timeout", alias = "timout", value_name = "SECONDS")] timeout_seconds: Option<u64>; #[arg(long, alias = "jsno", alias = "jason")] json: bool;)
    @ ReadArgs(#[arg(value_name = "PATH", required_unless_present = "paths_from")] path: Vec<PathBuf>; #[arg(long)] paths_from: Option<PathBuf>; #[arg(long, default_value_t = 20)] max_files: usize; #[arg(long, default_value_t = 4000)] max_visible_tokens: usize; #[arg(long)] start_line: Option<usize>; #[arg(long)] end_line: Option<usize>; #[arg(long)] raw: bool; #[command(flatten)] tool: ToolArgs;)
    @ FindArgs(query: String; path: Vec<PathBuf>; #[arg(long, default_value_t = 20)] max_files: usize; #[arg(long, default_value_t = 4000)] max_visible_tokens: usize; #[command(flatten)] tool: ToolArgs;)
    @ RecallArgs(query: String; #[arg(long, default_value_t = 50)] max_hits: usize; #[arg(long, default_value_t = 4000)] max_visible_tokens: usize; #[command(flatten)] tool: ToolArgs;)
    @ FetchArgs(url: String; #[doc = "Serve a cached body younger than this without touching the network."] #[arg(long)] ttl_seconds: Option<usize>; #[doc = "Bypass the TTL cache and re-fetch."] #[arg(long)] fresh: bool; #[arg(long, default_value_t = 4000)] max_visible_tokens: usize; #[command(flatten)] tool: ToolArgs;)
    @ GlobArgs(pattern: String; path: Vec<PathBuf>; #[arg(long, default_value_t = 200)] max_files: usize; #[arg(long, default_value_t = 4000)] max_visible_tokens: usize; #[arg(long)] include_hidden: bool; #[command(flatten)] tool: ToolArgs;)
    @ TreeArgs(path: Vec<PathBuf>; #[arg(long, default_value_t = 2)] depth: usize; #[arg(long, default_value_t = 200)] max_files: usize; #[arg(long, default_value_t = 4000)] max_visible_tokens: usize; #[arg(long)] include_hidden: bool; #[command(flatten)] tool: ToolArgs;)
    @ EditArgs(#[arg(value_name = "PATH")] path: PathBuf; #[doc = "JSON array of {find, replace, replace_all?} hunks."] #[arg(long = "edits-json", value_name = "JSON")] edits_json: Option<String>; #[doc = "Read the edits JSON from stdin instead of --edits-json."] #[arg(long)] stdin: bool; #[doc = "Create a new file: one hunk with empty find; replace is the content."] #[arg(long)] create: bool; #[doc = "Validate and render the hunk diff without writing."] #[arg(long)] dry_run: bool; #[arg(long, default_value_t = 4000)] max_visible_tokens: usize; #[command(flatten)] tool: ToolArgs;)
    @ RunArgs(#[arg(last = true, required_unless_present = "stdin")] command: Vec<String>; #[arg(long)] cwd: Option<PathBuf>; #[arg(long)] rewrite: Option<String>; #[arg(long)] no_rewrite: bool; #[arg(long)] stdin: bool; #[arg(long = "env")] env_overrides: Vec<String>; #[arg(long)] explain_runtime: bool; #[arg(long)] runtime_platform: Option<String>; #[command(flatten)] tool: ToolArgs;)
    @ IngestArgs(input: Option<PathBuf>; #[arg(long)] stdin: bool; #[arg(long, default_value = "auto")] kind: String; #[command(flatten)] tool: ToolArgs;)
    @ ExpandArgs(#[arg(value_name = "REF", required_unless_present = "refs_from")] refs: Vec<String>; #[doc = "Read additional newline-delimited refs; multi-ref JSON output is an ordered array."] #[arg(long)] refs_from: Option<PathBuf>; #[arg(long)] selector: Option<String>; #[arg(long)] raw: bool; #[arg(long)] summary: bool; #[arg(long)] start_line: Option<usize>; #[arg(long)] end_line: Option<usize>; #[arg(long)] line: Option<usize>; #[arg(long)] lines: Option<String>; #[arg(long)] around: Option<String>; #[arg(long)] anchor_kind: Option<String>; #[arg(long)] symbol: Option<String>; #[arg(long)] cache_path: Option<PathBuf>; #[arg(long)] json: bool;)
    @ RewriteArgs(#[doc = "Command string; alternative to trailing -- <command...>."] command: Option<String>; #[doc = "Command after --, matching tokenzero run -- <command...>."] #[arg(last = true)] argv: Vec<String>; #[arg(long, default_value = "safe")] mode: String; #[arg(long)] json: bool;)
    @ HookArgs(#[command(subcommand)] target: HookTarget;)
    @ HookClaudeCodeArgs(#[doc = "rewrite | guide | off. Unknown values pass through (fail-open)."] #[arg(long, default_value = "rewrite")] mode: String;)
    @ HookSessionStartArgs(#[doc = "Token budget for the restored session pack."] #[arg(long, default_value_t = 600)] max_tokens: usize;)
    @ DoctorArgs(#[arg(long, global = true)] root: Option<PathBuf>; #[arg(long, global = true)] cache_path: Option<PathBuf>; #[arg(long, global = true)] runtime: bool; #[arg(long, global = true)] json: bool; #[arg(long = "robot-triage", global = true)] robot_triage: bool; #[arg(long, global = true)] fix: bool; #[arg(long = "dry-run", global = true)] dry_run: bool; #[arg(long, global = true)] explain: Option<String>; #[command(subcommand)] command: Option<DoctorCommand>;)
    @ PulseArgs(#[arg(long, global = true)] root: Option<PathBuf>; #[arg(long, global = true)] json: bool; #[command(subcommand)] command: Option<PulseCommand>;)
    @ PulseExportArgs(#[arg(value_name = "OUTPUT")] output: PathBuf;)
    @ PulseImportArgs(#[arg(value_name = "INPUT")] input: PathBuf;)
    @ SessionLedgerArgs(#[arg(long, global = true)] root: Option<PathBuf>; #[arg(long, global = true)] json: bool; #[command(subcommand)] command: Option<SessionLedgerCommand>;)
    @ TelemetryInspectArgs(#[doc = "Explicitly opt in to usage telemetry inspection/recording (no exporter exists)."] #[arg(long)] telemetry: bool; #[doc = "Explicitly opt out; takes precedence over --telemetry."] #[arg(long)] no_telemetry: bool;)
    @ CacheArgs(#[command(subcommand)] command: CacheCommand;)
    @ CachePackArgs(#[arg(long, default_value = "agent")] scope: String; #[arg(long)] root: Option<PathBuf>; #[arg(long)] cache_path: Option<PathBuf>; #[arg(long)] json: bool;)
    @ BenchArgs(#[command(subcommand)] command: BenchCommand;)
    @ BenchCompetitorsArgs(#[arg(long, default_value = "shell-heavy")] suite: String; #[arg(long)] output_json: Option<PathBuf>; #[arg(long)] adapter_approval_artifact: Option<PathBuf>; #[arg(long)] json: bool;)
    @ CachePruneArgs(#[arg(long)] root: Option<PathBuf>; #[arg(long)] cache_path: Option<PathBuf>; #[arg(long)] apply: bool; #[arg(long)] json: bool;)
    @ CacheMigrateRefsArgs(#[arg(long)] root: Option<PathBuf>; #[arg(long)] cache_path: Option<PathBuf>; #[doc = "Actually write to CAS, store, and manifest. Without this flag, migration is dry-run only."] #[arg(long)] apply: bool; #[arg(long)] json: bool;)
    @ CacheMigrateVerifyArgs(#[arg(long)] root: Option<PathBuf>; #[arg(long)] cache_path: Option<PathBuf>; #[arg(long)] json: bool;)
    @ CacheMigrateRollbackArgs(#[arg(long)] root: Option<PathBuf>; #[arg(long)] cache_path: Option<PathBuf>; #[doc = "Actually remove aliases and manifest. Without this flag, rollback is dry-run only."] #[arg(long)] apply: bool; #[arg(long)] json: bool;)
    @ CacheMigrateCleanupArgs(#[arg(long)] root: Option<PathBuf>; #[arg(long)] cache_path: Option<PathBuf>; #[doc = "Actually remove legacy source payloads. Requires --confirm-cleanup."] #[arg(long, requires = "confirm_cleanup")] apply: bool; #[doc = "Required confirmation flag. Cleanup is irreversible without migration re-run."] #[arg(long, requires = "apply")] confirm_cleanup: bool; #[arg(long)] json: bool;)
    @ InstallArgs(#[arg(long)] root: Option<PathBuf>; #[arg(long)] plan: bool; #[arg(long)] apply: bool; #[arg(long)] rollback: Option<String>; #[arg(long)] global: bool; #[arg(long)] mcp: bool; #[arg(long)] shell: bool; #[arg(long)] instructions: bool; #[arg(long)] cli: bool; #[doc = "Wire the Claude Code PreToolUse hook into .claude/settings.json."] #[arg(long)] hooks: bool; #[doc = "Install the universal PATH shims under .tokenzero/shims/."] #[arg(long)] shims: bool; #[arg(long = "agent", value_name = "AGENT")] agents: Vec<String>; #[arg(long)] grok: bool; #[doc = "MCP tool surface profile (always classic; CodeMode is a separate execution layer)."] #[arg(long, value_name = "SURFACE", default_value = "classic")] surface: String; #[arg(long)] json: bool;)
    @ InitArgs(#[arg(long)] root: Option<PathBuf>; #[arg(long)] global: bool; #[arg(long = "agent", value_name = "AGENT")] agents: Vec<String>; #[arg(long)] mcp: bool; #[arg(long)] shell: bool; #[arg(long)] instructions: bool; #[arg(long)] cli: bool; #[doc = "Wire the Claude Code PreToolUse hook into .claude/settings.json."] #[arg(long)] hooks: bool; #[doc = "Install the universal PATH shims under .tokenzero/shims/."] #[arg(long)] shims: bool; #[arg(long)] apply: bool; #[arg(long)] plan: bool; #[doc = "MCP tool surface profile (always classic; CodeMode is a separate execution layer)."] #[arg(long, value_name = "SURFACE", default_value = "classic")] surface: String; #[arg(long)] json: bool;)
    @ ClientsArgs(#[command(subcommand)] command: ClientsCommand;)
    @ ClientStatusArgs(#[arg(long)] root: Option<PathBuf>; #[arg(long = "agent", value_name = "AGENT")] agents: Vec<String>; #[arg(long)] grok: bool; #[arg(long)] json: bool;)
    @ ClientsPlanArgs(#[arg(long)] root: Option<PathBuf>; #[arg(long, default_value = "standard")] profile: String; #[arg(long = "agent", value_name = "AGENT")] agents: Vec<String>; #[arg(long)] grok: bool; #[arg(long)] json: bool;)
    @ ClientsRollbackArgs(id: String; #[arg(long)] root: Option<PathBuf>; #[arg(long)] json: bool;)
    @ CapabilitiesArgs(#[arg(long, alias = "jsno", alias = "jason")] json: bool;)
    @ RobotDocsArgs(#[command(subcommand)] command: RobotDocsCommand;)
    @ McpServerArgs(#[doc = "Launch the explicit classic MCP compatibility surface. Aggregate CodeMode belongs to ZeroStack."] #[arg(long, default_value = "mcp", value_name = "MODE")] mode: String; #[arg(long)] allowed_root: Vec<PathBuf>; #[arg(long)] cache_path: Option<PathBuf>; #[arg(long, default_value = "auto")] default_mode: String; #[arg(long, alias = "timeout", value_name = "SECONDS")] shell_timeout_seconds: Option<u64>; #[arg(long, value_name = "SECONDS")] idle_timeout_seconds: Option<u64>; #[doc = "Backward-compatible alias for --mode; only mcp is accepted locally."] #[arg(long, value_name = "SURFACE")] tool_surface: Option<String>;)
    @ OsReachAuditArgs(#[arg(long, default_value = "results/current/tokenzero_os_reach_audit.json")] output_json: PathBuf; #[arg(long)] output_md: Option<PathBuf>; #[arg(long, default_value = ".")] root: PathBuf; #[arg(long = "os-artifact")] os_artifact: Vec<PathBuf>; #[arg(long)] release_approval: bool; #[arg(long)] json: bool;)
    @ OsReleaseArtifactArgs(#[arg(long, default_value = "results/current/tokenzero_os_release_artifact.json")] output_json: PathBuf; #[arg(long)] output_md: Option<PathBuf>; #[arg(long, default_value = ".")] root: PathBuf; #[arg(long)] json: bool;)
    @ SourceCurrencyAuditArgs(#[arg(long, default_value = "results/current/tokenzero_source_currency.json")] output_json: PathBuf; #[arg(long)] output_md: Option<PathBuf>; #[arg(long)] refresh_ledger: Option<PathBuf>; #[arg(long)] refresh_git_heads: bool; #[arg(long)] json: bool;)
    @ AdapterApprovalAuditArgs(#[arg(long, default_value = "results/current/tokenzero_adapter_approval_audit.json")] output_json: PathBuf; #[arg(long)] output_md: Option<PathBuf>; #[arg(long)] approval_file: Option<PathBuf>; #[arg(long)] execution_approval: bool; #[arg(long)] json: bool;)
    @ ClaimAuditArgs(#[arg(long, default_value = "results/current/tokenzero_claim_audit.json")] output_json: PathBuf; #[arg(long)] output_md: Option<PathBuf>; #[arg(long)] source_artifact: Option<PathBuf>; #[arg(long)] benchmark_artifact: Option<PathBuf>; #[arg(long)] adapter_approval_artifact: Option<PathBuf>; #[arg(long)] recovery_artifact: Option<PathBuf>; #[arg(long)] task_success_artifact: Option<PathBuf>; #[arg(long)] os_artifact: Option<PathBuf>; #[arg(long)] release_approval: bool; #[arg(long)] json: bool;)
    @ ReachArgs(#[arg(long, default_value = ".")] root: PathBuf; #[arg(long)] output_json: Option<PathBuf>; #[arg(long)] json: bool;)
    @ InstallSmokeArgs(#[doc = "Run apply plus rollback inside the disposable temporary root. Without this flag, only the plan is evaluated."] #[arg(long)] apply: bool; #[doc = "Write the report to this explicit path; no artifact is written by default."] #[arg(long)] output_json: Option<PathBuf>; #[arg(long)] json: bool;)
    @ PackageAuditArgs(#[arg(long, default_value = ".")] dist: PathBuf; #[arg(long)] json: bool;)
    @ QuoteArgs(#[arg(long)] platform: String; #[arg(last = true)] args: Vec<String>; #[arg(long)] json: bool;)

}

artifact_args! {
    McpSmokeArgs => "results/current/rust_mcp_smoke.json",
    McpSoakArgs => "results/current/rust_mcp_soak.json",
    HarmEvalArgs => "results/current/harm_eval.json",
    RepoInventoryArgs => "results/current/repo_inventory.json",
    PromptCachePackArgs => "results/current/prompt_cache_pack.json",
    ShellMatrixArgs => "results/current/tokenzero_shell_matrix.json",
    FalseSuccessShellArgs => "results/current/tokenzero_false_success_shell.json",
    ExactRecoveryShellArgs => "results/current/tokenzero_exact_recovery_shell.json",
    ExactRecoveryAuditArgs => "results/current/tokenzero_exact_recovery_audit.json",
    ProtectedAnchorAuditArgs => "results/current/tokenzero_protected_anchor_audit.json",
    OneShotEvalArgs => "results/current/tokenzero_one_shot_eval.json",
    AdapterApprovalTemplateArgs => "results/current/tokenzero_adapter_approval_file.json",
    CompletionAuditArgs => "results/current/tokenzero_completion_audit.json",
    SecurityPrivacyAuditArgs => "results/current/tokenzero_security_privacy_audit.json",
    ArtifactHandoffArgs => "results/current/tokenzero_artifact_handoff.json",
    WsSkeletonArgs => "results/current/tokenzero_ws_001.json",
}

