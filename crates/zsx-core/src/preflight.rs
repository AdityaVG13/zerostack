//! K0 capability broker: parse / resolve / normalize / inject / validate
//! (zerostack-pvwg).
//!
//! The broker is the preflight boundary of the Wave 10 supervisor. Before a
//! plan executes it:
//!
//! - **parse**s the cell text with a string- and comment-aware call scan
//!   (capability call sites, positional strings, object keys and values,
//!   positional arrays — never evaluated);
//! - **resolves** every capability mention against the existing V6
//!   registration ([`crate::lower::METHODS`] plus the interpreter's
//!   intrinsic `zero.decision.require`);
//! - **normalizes** approved aliases / shorthand / defaults / units /
//!   handle forms from the single lowering authority
//!   ([`crate::lower`]'s published tables: compound operation names, search
//!   and lookup alias keys, shell timeout units) and records each approved
//!   fold as receipt evidence;
//! - **injects** the unambiguous operational context — project, workspace,
//!   request, session and manifest roots, ABI version, budget deadline and
//!   the V6 authority digest — into the receipt (`k0: injected …`);
//! - **validates** the plan against a rooted capability manifest when the
//!   caller injected one, and proves every pointed-at external read path is
//!   preserved through an explicit read grant minted in the same plan.
//!
//! # Fail-closed laws (this bead)
//!
//! - The broker **never rewrites the program**: an already-correct plan
//!   runs byte-for-byte as the interpreter sees it, and the native direct
//!   path (`zsx exec`, session `execute`, MCP `zero_execute`) is untouched
//!   (W10-T13: a trivial already-correct call pays one linear scan and is
//!   never routed through anything heavier).
//! - The broker **never selects meaning**: an unregistered surface, method
//!   or compound operation with close candidates resolves through the
//!   decision API against an empty contingent policy into a typed
//!   [`DecisionRequired`] — the model answers, the kernel never guesses.
//! - Deterministic structural refusals fail before any execution and before
//!   any one-shot child spawn: approval-required `fs.write` (the kernel
//!   installs no approval grants), the non-V6 `z.*` guest surface,
//!   unqualified capability calls, conflicting timeout spellings, unknown
//!   names without close candidates, ungranted pointed-at external reads,
//!   and capability usage outside a rooted manifest.
//! - The broker adds **no capability and no authority**: every resolved
//!   name is checked against the existing registration, a manifest can only
//!   shrink the allowed set, and the only file the broker reads is the
//!   caller-supplied capability manifest (bounded).
//!
//! # Honest scan boundary
//!
//! The scanner is conservative. Constructs it cannot analyze (computed
//! capability member access, template literals with substitutions,
//! unterminated strings/comments) are reported as scan-opaque and never
//! silently pass a check. With a capability manifest present an opaque scan
//! fails closed (compliance cannot be certified); without one the plan
//! proceeds under the runtime's own enforcement and the receipt records the
//! gap. The runtime grant/approval/journal gates stay authoritative for
//! anything the text scan cannot prove.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::{Value, json};
use zero_abi::contract_digest_hex;
use zero_abi::decision::{
    ContingentPolicy, DecisionRequired, ObservationClass, PolicyResolution, SemanticDecisionPoint,
};
use zero_abi::zerokernel::{ZEROKERNEL_ABI_VERSION, ZerokernelExecuteRequest};

use crate::lower::{
    COMPOUND_OPS, LOOKUP_LIMIT_ALIAS_KEYS, LOOKUP_QUERY_ALIAS_KEYS, LOOKUP_ROOT_ALIAS_KEYS,
    METHODS, SEARCH_QUERY_ALIAS_KEYS,
};
use crate::read_grant::MAX_SESSION_READ_GRANTS;

/// Schema of the capability manifest at `capability_manifest_root`
/// (validated, never executed).
pub const CAPABILITY_MANIFEST_SCHEMA: &str = "zerostack.k0.capability_manifest.v1";
/// The only accepted manifest version; unknown versions fail closed.
pub const CAPABILITY_MANIFEST_VERSION: u64 = 1;
/// Manifest byte bound: preflight reads one small caller-owned file.
pub const CAPABILITY_MANIFEST_MAX_BYTES: usize = 64 * 1024;
/// Manifest capability entry bound.
pub const CAPABILITY_MANIFEST_MAX_ENTRIES: usize = 256;

/// Stable observation class for capability-resolution decisions.
pub const OBSERVATION_CLASS_CAPABILITY_RESOLVE: &str = "k0.capability.resolve";
/// Decision ids for the three resolution shapes.
pub const DECISION_ID_SURFACE: &str = "k0.resolve.surface";
pub const DECISION_ID_METHOD: &str = "k0.resolve.method";
pub const DECISION_ID_COMPOUND_OP: &str = "k0.resolve.compound_op";

/// The interpreter's intrinsic decision surface (`zero.decision.require`):
/// part of the V6 callable surface even though it is not in `METHODS`.
pub const DECISION_SURFACE_NAME: &str = "decision";
pub const DECISION_REQUIRE_METHOD_NAME: &str = "require";

const SURFACES: &[&str] = &["fs", "graph", "token", "help", DECISION_SURFACE_NAME];
const MAX_MENTIONS: usize = 256;
const MAX_ARGS_BYTES: usize = 4096;
const MAX_VALUE_STRINGS: usize = 16;
const MAX_ARRAY_STRINGS: usize = 32;
const MAX_STRING_BYTES: usize = 1024;
const MAX_EXTERNAL_PATHS: usize = 32;
const MAX_REPAIR_LINES: usize = 32;
const CLOSEST_CANDIDATES: usize = 5;
const CLOSEST_MAX_DISTANCE: usize = 2;

/// Outcome of the broker for one request. `Proceed` keeps the original
/// program; the other two are terminal protocol responses the supervisor
/// returns without executing anything.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BrokerOutcome {
    /// The plan is structurally sound; run the original program and merge
    /// the receipt into the preflight report.
    Proceed(BrokerReceipt),
    /// Semantic ambiguity: never auto-selected. Typed decision payload.
    DecisionRequired(DecisionRequired),
    /// Deterministic structural refusal; the detail rides the preflight
    /// errors of a `Failed` response.
    Refused(String),
}

/// Receipt of the broker boundary: injected operational context plus the
/// structural-repair evidence merged into the response's preflight report.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BrokerReceipt {
    /// `k0: injected …` context line (roots, version, deadline, authority).
    pub injected: String,
    /// `k0: repair …` structural normalization evidence (bounded, deduped).
    pub repairs: Vec<String>,
    /// Scan-gap warning when the scanner could not fully analyze the plan.
    pub opaque_warning: Option<String>,
    /// Capability mentions resolved to registered callables.
    pub resolved_mentions: usize,
}

impl BrokerReceipt {
    /// The receipt lines merged into `PreflightReport.warnings`.
    pub fn warning_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if !self.injected.is_empty() {
            lines.push(self.injected.clone());
        }
        if let Some(warning) = &self.opaque_warning {
            lines.push(format!(
                "k0: scan opaque: {warning}; runtime capability policy still enforced"
            ));
        }
        lines.extend(self.repairs.iter().take(MAX_REPAIR_LINES).cloned());
        lines.push(format!(
            "k0: resolved {} capability mention(s)",
            self.resolved_mentions
        ));
        lines
    }
}

/// One object member of an analyzed call argument, with the string literals
/// directly in its value: the scalar for `{path: "/x"}`, the elements for
/// `{paths: ["/a", "/b"]}`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ObjectKey {
    pub key: String,
    /// The value is exactly one string literal (no concatenation, no nested
    /// object, array, call, or template).
    pub single: Option<String>,
    /// String literals directly in the value: the scalar itself or the
    /// direct elements of a string array.
    pub strings: Vec<String>,
}

/// One capability-shaped call site found by the plan scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityMention {
    /// `zero.<surface>.<method>(...)` fully qualified form.
    pub qualified: bool,
    /// `z.invoke("…", …)` form (no V6 `z` namespace).
    pub invoke: bool,
    /// `z.<member>(…)` form (no V6 `z` namespace).
    pub z_member: bool,
    /// Surface text (`fs`, `graph`, `token`, `help`, `decision`, or any
    /// typed name for unregistered mentions).
    pub surface: String,
    /// Method text.
    pub method: String,
    /// First positional string argument (compound operation name,
    /// positional read path).
    pub first_arg: Option<String>,
    /// Outermost argument-object members.
    pub object_keys: Vec<ObjectKey>,
    /// Strings directly inside a first-level positional array argument.
    pub array_strings: Vec<String>,
    /// The scanner could not fully analyze the argument region (template
    /// with substitution, truncated region, unterminated literal).
    pub opaque_args: bool,
}

/// Result of the string- and comment-aware plan scan.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlanScan {
    pub mentions: Vec<CapabilityMention>,
    /// The scanner met a construct it cannot fully analyze (computed
    /// capability member access, unterminated comments/templates, template
    /// substitutions). With a capability manifest present the broker fails
    /// closed; otherwise it proceeds under runtime enforcement.
    pub opaque: bool,
    pub opaque_reason: Option<String>,
}

/// Run the K0 capability broker over one execute request.
///
/// `project_root` must be the supervisor's canonicalized project/workspace
/// root (the read-grant "session root" the connector enforces), and
/// `session_id` the supervisor's bound session identity.
pub fn broker(
    request: &ZerokernelExecuteRequest,
    project_root: &Path,
    session_id: &str,
) -> BrokerOutcome {
    let scan = scan_plan(&request.program);
    let mut receipt = BrokerReceipt {
        injected: injected_line(request, session_id),
        ..BrokerReceipt::default()
    };
    let mut mints: Vec<String> = Vec::new();
    let mut external_reads: Vec<String> = Vec::new();
    let mut resolved_callables: Vec<String> = Vec::new();
    let mut repair_set: BTreeSet<String> = BTreeSet::new();
    let root_text = project_root.to_string_lossy().into_owned();

    for mention in scan.mentions.iter().take(MAX_MENTIONS) {
        // Shape refusals, deterministic and in plan order.
        if mention.z_member {
            return BrokerOutcome::Refused(format!(
                "z.{} is not part of the V6 surface; call zero.<surface>.<method>(...) directly",
                mention.method
            ));
        }
        if mention.invoke {
            return BrokerOutcome::Refused(
                "z.invoke is not part of the V6 surface; call zero.<surface>.<method>(...) directly"
                    .to_owned(),
            );
        }
        if !mention.qualified {
            return BrokerOutcome::Refused(format!(
                "unqualified capability call {}.{}(...): qualify as zero.{}.{}(...)",
                mention.surface, mention.method, mention.surface, mention.method
            ));
        }
        if !is_surface_name(&mention.surface) {
            let candidates = closest_candidates(SURFACES.iter().copied(), &mention.surface);
            if candidates.is_empty() {
                return BrokerOutcome::Refused(format!(
                    "unknown capability surface '{}' on zero; registered surfaces: {}",
                    mention.surface,
                    SURFACES.join(", ")
                ));
            }
            let point = SemanticDecisionPoint::new(
                DECISION_ID_SURFACE,
                ObservationClass::new(OBSERVATION_CLASS_CAPABILITY_RESOLVE)
                    .expect("observation class is valid"),
                format!(
                    "capability surface '{}' is not registered; which surface did you mean?",
                    mention.surface
                ),
                candidates,
                Vec::new(),
            )
            .expect("decision point is valid");
            return BrokerOutcome::DecisionRequired(decision(&point, &mention.surface));
        }

        // Compound operation resolution against the single approved table.
        if mention.surface == "fs" && mention.method == "compound" {
            let op = mention.first_arg.clone().or_else(|| {
                mention
                    .object_keys
                    .iter()
                    .find(|key| key.key == "name")
                    .and_then(|key| key.single.clone())
            });
            let Some(op) = op else {
                return BrokerOutcome::Refused(
                    "fs.compound requires an operation name as its first argument".to_owned(),
                );
            };
            let canonical = COMPOUND_OPS
                .iter()
                .find(|(alias, _)| *alias == op)
                .map(|(_, target)| *target);
            let Some(canonical) = canonical else {
                let candidates =
                    closest_candidates(COMPOUND_OPS.iter().map(|(alias, _)| *alias), &op);
                if candidates.is_empty() {
                    let registered = COMPOUND_OPS
                        .iter()
                        .map(|(alias, _)| *alias)
                        .collect::<Vec<_>>()
                        .join(", ");
                    return BrokerOutcome::Refused(format!(
                        "unknown fs.compound operation '{op}'; registered operations: {registered}"
                    ));
                }
                let point = SemanticDecisionPoint::new(
                    DECISION_ID_COMPOUND_OP,
                    ObservationClass::new(OBSERVATION_CLASS_CAPABILITY_RESOLVE)
                        .expect("observation class is valid"),
                    format!(
                        "fs.compound operation '{op}' is not registered; which operation did you mean?"
                    ),
                    candidates,
                    Vec::new(),
                )
                .expect("decision point is valid");
                return BrokerOutcome::DecisionRequired(decision(&point, &op));
            };
            if canonical == "fs.write" {
                return BrokerOutcome::Refused(write_refusal());
            }
            receipt.resolved_mentions += 1;
            resolved_callables.push("fs.compound".to_owned());
            repair_set.insert(format!(
                "k0: repair fs.compound('{op}') resolves to {canonical} (approved alias)"
            ));
            if matches!(op.as_str(), "search" | "find" | "grep") {
                for key in &mention.object_keys {
                    if SEARCH_QUERY_ALIAS_KEYS.contains(&key.key.as_str()) {
                        repair_set.insert(format!(
                            "k0: repair fs.compound search alias key '{}' folds to query",
                            key.key
                        ));
                    }
                }
            }
            if canonical == "fs.read" || canonical == "fs.multiRead" {
                collect_read_paths(mention, &mut external_reads);
            }
            if canonical == "fs.readGrant" {
                if let Err(detail) = collect_mint(mention, &root_text, &mut mints) {
                    return BrokerOutcome::Refused(detail);
                }
            }
            continue;
        }

        // Registered direct method?
        if is_registered_capability(&mention.surface, &mention.method) {
            if mention.method == "write" && mention.surface == "fs" {
                return BrokerOutcome::Refused(write_refusal());
            }
            receipt.resolved_mentions += 1;
            resolved_callables.push(format!("{}.{}", mention.surface, mention.method));
            match (mention.surface.as_str(), mention.method.as_str()) {
                ("fs", "read") => {
                    if mention.first_arg.is_some() {
                        repair_set.insert("k0: repair fs.read positional path shorthand".into());
                    }
                    collect_read_paths(mention, &mut external_reads);
                }
                ("fs", "multi_read") => collect_read_paths(mention, &mut external_reads),
                ("fs", "read_grant") => {
                    if let Err(detail) = collect_mint(mention, &root_text, &mut mints) {
                        return BrokerOutcome::Refused(detail);
                    }
                }
                ("token", "shell") => {
                    let has_ms = mention.object_keys.iter().any(|key| key.key == "timeout_ms");
                    let has_camel = mention.object_keys.iter().any(|key| key.key == "timeoutMs");
                    if has_camel && has_ms {
                        return BrokerOutcome::Refused(
                            "token.shell options must not include both 'timeoutMs' and 'timeout_ms'"
                                .to_owned(),
                        );
                    }
                    if mention
                        .object_keys
                        .iter()
                        .any(|key| key.key == "timeout_seconds")
                    {
                        repair_set.insert(
                            "k0: repair token.shell timeout_seconds lowered to milliseconds at dispatch"
                                .into(),
                        );
                    }
                    if has_camel {
                        repair_set
                            .insert("k0: repair token.shell timeoutMs alias -> timeout_ms".into());
                    }
                }
                ("fs", "lookup") => {
                    for key in &mention.object_keys {
                        if LOOKUP_ROOT_ALIAS_KEYS.contains(&key.key.as_str()) {
                            repair_set.insert(format!(
                                "k0: repair fs.lookup alias key '{}' folds to root",
                                key.key
                            ));
                        } else if LOOKUP_QUERY_ALIAS_KEYS.contains(&key.key.as_str()) {
                            repair_set.insert(format!(
                                "k0: repair fs.lookup alias key '{}' folds to query",
                                key.key
                            ));
                        } else if LOOKUP_LIMIT_ALIAS_KEYS.contains(&key.key.as_str()) {
                            repair_set.insert(format!(
                                "k0: repair fs.lookup alias key '{}' folds to limit",
                                key.key
                            ));
                        }
                    }
                }
                _ => {}
            }
            continue;
        }

        // Unknown method on a registered surface: never auto-select.
        let candidates = closest_candidates(methods_of(&mention.surface), &mention.method);
        if candidates.is_empty() {
            return BrokerOutcome::Refused(format!(
                "unknown capability method '{}' on zero.{}; no registered method is close",
                mention.method, mention.surface
            ));
        }
        let point = SemanticDecisionPoint::new(
            DECISION_ID_METHOD,
            ObservationClass::new(OBSERVATION_CLASS_CAPABILITY_RESOLVE)
                .expect("observation class is valid"),
            format!(
                "capability method '{}' on surface '{}' is not registered; which method did you mean?",
                mention.method, mention.surface
            ),
            candidates
                .iter()
                .map(|candidate| format!("zero.{}.{}", mention.surface, candidate))
                .collect(),
            Vec::new(),
        )
        .expect("decision point is valid");
        return BrokerOutcome::DecisionRequired(decision(&point, &mention.method));
    }

    // Rooted capability manifest validation: the caller injected a
    // restriction, so compliance must be provable.
    if let Some(manifest_root) = &request.roots.capability_manifest_root {
        if scan.opaque {
            return BrokerOutcome::Refused(format!(
                "plan contains constructs the preflight scanner cannot fully analyze ({}) so capability-manifest compliance cannot be certified",
                scan.opaque_reason.as_deref().unwrap_or("unknown construct")
            ));
        }
        let manifest = match load_manifest(manifest_root) {
            Ok(manifest) => manifest,
            Err(detail) => {
                return BrokerOutcome::Refused(format!(
                    "capability manifest at {manifest_root} is not usable: {detail}"
                ));
            }
        };
        for entry in &manifest.capabilities {
            let Some((surface, method)) = entry.split_once('.') else {
                return BrokerOutcome::Refused(format!(
                    "capability manifest entry '{entry}' is not a surface.method capability name"
                ));
            };
            if !is_registered_capability(surface, method) {
                return BrokerOutcome::Refused(format!(
                    "capability manifest names '{entry}' which is not a registered V6 capability"
                ));
            }
        }
        for callable in &resolved_callables {
            if !manifest.capabilities.contains(callable) {
                return BrokerOutcome::Refused(format!(
                    "capability '{callable}' is not granted by the capability manifest at {manifest_root}"
                ));
            }
        }
    }

    // Pointed-at external read paths are preserved through explicit grants
    // only. The runtime grant gate stays authoritative; this is the earlier,
    // deterministic check. When the plan names too many distinct paths to
    // enumerate at preflight, runtime enforcement covers the overflow.
    if external_reads.len() <= MAX_EXTERNAL_PATHS {
        for path in &external_reads {
            if !mints.contains(path) {
                return BrokerOutcome::Refused(format!(
                    "external read of {path} requires an explicit grant: mint zero.fs.read_grant({{path: {path}}}) (or zero.fs.compound('readGrant', {{path: {path}}})) for exactly that path first"
                ));
            }
        }
    }

    if scan.opaque {
        receipt.opaque_warning = Some(
            scan.opaque_reason
                .clone()
                .unwrap_or_else(|| "unknown construct".to_owned()),
        );
    }
    receipt.repairs = repair_set.into_iter().take(MAX_REPAIR_LINES).collect();
    BrokerOutcome::Proceed(receipt)
}

/// The injected-context receipt line: every operational root the kernel
/// binds, the ABI version, the budget deadline, and the V6 authority
/// digest (a stable hash over the registered capability catalog).
fn injected_line(request: &ZerokernelExecuteRequest, session_id: &str) -> String {
    let mut parts = Vec::new();
    parts.push(format!("project={}", request.roots.project_root));
    if let Some(workspace) = &request.roots.workspace_root {
        parts.push(format!("workspace={workspace}"));
    }
    parts.push(format!("session={session_id}"));
    if let Some(request_root) = &request.roots.request_root {
        parts.push(format!("request={request_root}"));
    }
    if let Some(manifest) = &request.roots.capability_manifest_root {
        parts.push(format!("manifest={manifest}"));
    }
    parts.push(format!("version={ZEROKERNEL_ABI_VERSION}"));
    parts.push(format!("deadline={}ms", request.budget.wall_ms));
    parts.push(format!("authority={}", AUTHORITY_DIGEST.as_str()));
    format!("k0: injected {}", parts.join(" "))
}

/// Stable digest over the registered V6 capability catalog. Computed once
/// per process; the receipt line proves the kernel's authority catalog is
/// exactly the existing V6 surface.
static AUTHORITY_DIGEST: LazyLock<String> = LazyLock::new(|| {
    let catalog = Value::Array(
        METHODS
            .iter()
            .map(|(surface, method)| json!([surface, method]))
            .collect(),
    );
    contract_digest_hex(&catalog)
});

fn write_refusal() -> String {
    "fs.write is approval-required and the kernel installs no approval grants; use a harness-owned session (zsx exec / MCP zero_execute) or drop the write".to_owned()
}

fn is_surface_name(name: &str) -> bool {
    matches!(name, "fs" | "graph" | "token" | "help" | DECISION_SURFACE_NAME)
}

fn is_registered_capability(surface: &str, method: &str) -> bool {
    (surface == DECISION_SURFACE_NAME && method == DECISION_REQUIRE_METHOD_NAME)
        || METHODS
            .iter()
            .any(|(s, m)| *s == surface && *m == method)
}

fn methods_of(surface: &str) -> impl Iterator<Item = &'static str> + '_ {
    METHODS
        .iter()
        .filter(move |(s, _)| *s == surface)
        .map(|(_, m)| *m)
}

/// Resolve an ambiguous typed name against the decision API with an empty
/// contingent policy: no rule can cover it, so the resolution is always
/// `Uncovered` and the typed `DecisionRequired` payload is returned.
/// The broker never supplies a policy, so it can never select.
fn decision(point: &SemanticDecisionPoint, observed: &str) -> DecisionRequired {
    let policy = ContingentPolicy::new(Vec::new()).expect("an empty policy is always valid");
    match policy.resolve(point, observed) {
        PolicyResolution::Uncovered { decision_required } => decision_required,
        PolicyResolution::Selected { .. } | PolicyResolution::PolicyError(_) => {
            unreachable!("an empty contingent policy never selects")
        }
    }
}

/// Close candidates within [`CLOSEST_MAX_DISTANCE`] Levenshtein distance of
/// the typed name, nearest first, bounded. Exact matches are excluded (an
/// exact match would have been registered).
fn closest_candidates<'a>(
    candidates: impl Iterator<Item = &'a str>,
    typed: &str,
) -> Vec<String> {
    let mut scored: Vec<(usize, String)> = candidates
        .filter(|candidate| *candidate != typed)
        .map(|candidate| (levenshtein(candidate, typed), candidate.to_owned()))
        .filter(|(distance, _)| *distance <= CLOSEST_MAX_DISTANCE)
        .collect();
    scored.sort();
    scored.truncate(CLOSEST_CANDIDATES);
    scored.into_iter().map(|(_, candidate)| candidate).collect()
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            current[j + 1] = (current[j] + 1)
                .min(previous[j + 1] + 1)
                .min(previous[j] + usize::from(ca != cb));
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// The typed capability manifest carried by `capability_manifest_root`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct CapabilityManifest {
    schema: String,
    version: u64,
    #[serde(default)]
    capabilities: Vec<String>,
}

fn load_manifest(root: &str) -> Result<CapabilityManifest, String> {
    let bytes = std::fs::read(Path::new(root))
        .map_err(|error| format!("cannot read {root}: {error}"))?;
    if bytes.len() > CAPABILITY_MANIFEST_MAX_BYTES {
        return Err(format!(
            "manifest exceeds the {CAPABILITY_MANIFEST_MAX_BYTES} byte bound"
        ));
    }
    let manifest: CapabilityManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid manifest: {error}"))?;
    if manifest.schema != CAPABILITY_MANIFEST_SCHEMA {
        return Err(format!(
            "schema must be {CAPABILITY_MANIFEST_SCHEMA}, got {}",
            manifest.schema
        ));
    }
    if manifest.version != CAPABILITY_MANIFEST_VERSION {
        return Err(format!(
            "version must be {CAPABILITY_MANIFEST_VERSION}, got {}",
            manifest.version
        ));
    }
    if manifest.capabilities.is_empty()
        || manifest.capabilities.len() > CAPABILITY_MANIFEST_MAX_ENTRIES
    {
        return Err(format!(
            "capabilities must list 1..={CAPABILITY_MANIFEST_MAX_ENTRIES} entries"
        ));
    }
    Ok(manifest)
}

fn is_absolute_path(path: &str) -> bool {
    path.starts_with('/')
}

/// Textual in-root test: `path` is inside the canonical project root
/// (`== root` or `root + "/"` prefix). The runtime re-verifies canonically;
/// this only decides whether preflight demands a grant.
fn in_root_path(path: &str, root_text: &str) -> bool {
    if root_text == "/" {
        return path.starts_with('/');
    }
    path == root_text
        || path
            .strip_prefix(root_text)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Collect the pointed-at absolute read paths of one read-shaped mention
/// (direct `fs.read`/`fs.multi_read`, or compound read ops). Relative and
/// in-root paths are confined by the session root and need no grant.
fn collect_read_paths(mention: &CapabilityMention, external: &mut Vec<String>) {
    let mut paths: Vec<String> = Vec::new();
    match (mention.surface.as_str(), mention.method.as_str()) {
        ("fs", "read") => {
            if let Some(path) = &mention.first_arg {
                paths.push(path.clone());
            }
        }
        ("fs", "multi_read") => {
            paths.extend(mention.array_strings.iter().take(MAX_ARRAY_STRINGS).cloned());
        }
        _ => {}
    }
    for key in &mention.object_keys {
        match key.key.as_str() {
            "path" | "arg" => {
                if let Some(path) = &key.single {
                    paths.push(path.clone());
                }
            }
            "paths" => {
                paths.extend(key.strings.iter().take(MAX_ARRAY_STRINGS).cloned());
            }
            _ => {}
        }
    }
    for path in paths {
        if !is_absolute_path(&path) {
            continue;
        }
        if !external.contains(&path) && external.len() < MAX_EXTERNAL_PATHS {
            external.push(path);
        }
    }
}

/// Collect one explicit grant mint, refusing deterministically when the
/// mint can never succeed under the read-grant law: the path must be
/// absolute and outside the session root (the runtime enforces the same
/// rules canonically at mint time).
fn collect_mint(
    mention: &CapabilityMention,
    root_text: &str,
    mints: &mut Vec<String>,
) -> Result<(), String> {
    let path = mention
        .first_arg
        .clone()
        .or_else(|| {
            mention
                .object_keys
                .iter()
                .find(|key| key.key == "path")
                .and_then(|key| key.single.clone())
        })
        .or_else(|| {
            mention
                .object_keys
                .iter()
                .find(|key| key.key == "arg")
                .and_then(|key| key.single.clone())
        });
    let Some(path) = path else {
        return Err("fs.read_grant requires a path string".to_owned());
    };
    if !is_absolute_path(&path) {
        return Err(format!("fs.read_grant path must be absolute, got '{path}'"));
    }
    if in_root_path(&path, root_text) {
        return Err(format!(
            "fs.read_grant path {path} is inside the session root; in-root files need no grant"
        ));
    }
    if !mints.contains(&path) && mints.len() < MAX_SESSION_READ_GRANTS {
        mints.push(path);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Plan scan
// ---------------------------------------------------------------------------

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$'
}

fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Names that shadow capability surfaces when bound locally. The scanner
/// tracks only these: a locally bound `fs`/`zero`/`z` must not turn a
/// subsequent identifier use into a (false) capability mention.
#[derive(Clone, Copy, Debug, Default)]
struct Shadowed {
    zero: bool,
    fs: bool,
    graph: bool,
    token: bool,
    help: bool,
    z: bool,
}

impl Shadowed {
    fn mark(&mut self, name: &str) {
        match name {
            "zero" => self.zero = true,
            "fs" => self.fs = true,
            "graph" => self.graph = true,
            "token" => self.token = true,
            "help" => self.help = true,
            "z" => self.z = true,
            _ => {}
        }
    }
    fn surface_shadowed(&self, surface: &str) -> bool {
        match surface {
            "fs" => self.fs,
            "graph" => self.graph,
            "token" => self.token,
            "help" => self.help,
            _ => false,
        }
    }
}

/// Byte-positioned, string- and comment-aware scanner over one region of
/// the plan source.
struct RegionScanner<'s> {
    source: &'s str,
    bytes: &'s [u8],
    pos: usize,
    end: usize,
}

impl<'s> RegionScanner<'s> {
    fn new(source: &'s str, start: usize, end: usize) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            pos: start,
            end: end.min(source.len()),
        }
    }
    fn byte_at(&self, index: usize) -> Option<u8> {
        if index >= self.end {
            None
        } else {
            self.bytes.get(index).copied()
        }
    }
    fn char_len_at(&self, index: usize) -> usize {
        self.source
            .get(index..)
            .and_then(|rest| rest.chars().next())
            .map_or(1, char::len_utf8)
    }
    fn advance_char(&mut self) {
        if self.pos < self.end {
            self.pos = (self.pos + self.char_len_at(self.pos)).min(self.end);
        }
    }
    /// Index of the next significant byte (whitespace and comments
    /// skipped), or `None` at the region end or an unterminated block
    /// comment.
    fn skip_trivia_from(&self, mut index: usize) -> Option<usize> {
        loop {
            let byte = self.byte_at(index)?;
            match byte {
                b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c => index += 1,
                b'/' if self.byte_at(index + 1) == Some(b'/') => {
                    index += 2;
                    while let Some(c) = self.byte_at(index) {
                        if c == b'\n' {
                            break;
                        }
                        index += 1;
                    }
                }
                b'/' if self.byte_at(index + 1) == Some(b'*') => {
                    index += 2;
                    loop {
                        if self.byte_at(index) == Some(b'*')
                            && self.byte_at(index + 1) == Some(b'/')
                        {
                            return self.skip_trivia_from(index + 2);
                        }
                        if self.byte_at(index).is_none() {
                            return None;
                        }
                        index += 1;
                    }
                }
                _ => return Some(index),
            }
        }
    }
    /// Scan one single- or double-quoted string starting at `index`.
    /// Returns the decoded text and the index just past the closing quote.
    fn scan_string(&self, index: usize) -> Option<(String, usize)> {
        let quote = self.byte_at(index)?;
        if quote != b'\'' && quote != b'"' {
            return None;
        }
        let mut i = index + 1;
        let mut out = String::new();
        loop {
            let byte = self.byte_at(i)?;
            if byte == quote {
                return Some((out, i + 1));
            }
            if byte == b'\\' {
                let escaped = self.byte_at(i + 1)?;
                i += 2;
                match escaped {
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'r' => out.push('\r'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000c}'),
                    b'v' => out.push('\u{000b}'),
                    b'0' => out.push('\0'),
                    b'x' => {
                        let hi = self.byte_at(i).and_then(hex_digit);
                        let lo = self.byte_at(i + 1).and_then(hex_digit);
                        if let (Some(hi), Some(lo)) = (hi, lo) {
                            out.push((hi << 4 | lo) as char);
                            i += 2;
                        } else {
                            out.push('x');
                        }
                    }
                    b'u' => {
                        let digits: Vec<u8> = (0..4)
                            .filter_map(|offset| self.byte_at(i + offset).and_then(hex_digit))
                            .collect();
                        if digits.len() == 4 {
                            let code = digits
                                .iter()
                                .fold(0u32, |acc, digit| acc << 4 | u32::from(*digit));
                            if let Some(c) = char::from_u32(code) {
                                out.push(c);
                            }
                            i += 4;
                        } else {
                            out.push('u');
                        }
                    }
                    b'\r' => {
                        if self.byte_at(i) == Some(b'\n') {
                            i += 1;
                        }
                    }
                    b'\n' => {}
                    other => out.push(other as char),
                }
                continue;
            }
            let len = self.char_len_at(i);
            out.push_str(&self.source[i..i + len]);
            i += len;
        }
    }
    /// Scan one template literal starting at `index`, conservatively: a
    /// `${...}` substitution marks the template and scanning resumes at the
    /// next unescaped backtick. Returns `(end after closing backtick,
    /// contains substitution)`.
    fn scan_template(&self, index: usize) -> Option<(usize, bool)> {
        if self.byte_at(index) != Some(b'`') {
            return None;
        }
        let mut i = index + 1;
        let mut substitution = false;
        loop {
            let byte = self.byte_at(i)?;
            match byte {
                b'\\' => {
                    i = (i + 2).min(self.end);
                }
                b'`' => return Some((i + 1, substitution)),
                b'$' if self.byte_at(i + 1) == Some(b'{') => {
                    substitution = true;
                    i += 2;
                }
                _ => i += self.char_len_at(i),
            }
        }
    }
    /// Index of the delimiter matching the opening bracket at `open`
    /// (`(`/`)`, `{`/`}`, `[`/`]`), or `None`.
    fn matching_close(&self, open: usize) -> Option<usize> {
        let opening = self.byte_at(open)?;
        let (close_byte, mut depth) = match opening {
            b'(' => (b')', 0usize),
            b'{' => (b'}', 0usize),
            b'[' => (b']', 0usize),
            _ => return None,
        };
        let mut i = open;
        loop {
            let byte = self.byte_at(i)?;
            match byte {
                b'\'' | b'"' => i = self.scan_string(i)?.1,
                b'`' => i = self.scan_template(i)?.0,
                b'(' | b'{' | b'[' => depth += 1,
                b')' | b'}' | b']' => {
                    depth = depth.checked_sub(1)?;
                    if byte == close_byte && depth == 0 {
                        return Some(i);
                    }
                }
                _ => i += self.char_len_at(i),
            }
        }
    }
    /// Read an identifier at the current position and advance past it.
    fn read_ident(&mut self) -> &'s str {
        let start = self.pos;
        while self.pos < self.end && is_ident_continue(self.byte_at(self.pos).unwrap_or(0)) {
            self.pos += 1;
        }
        &self.source[start..self.pos]
    }
}

const KEYWORD_BINDERS: &[&str] = &["const", "let", "var", "function", "class"];

/// String- and comment-aware scan for capability-shaped call sites.
pub fn scan_plan(program: &str) -> PlanScan {
    let mut scanner = RegionScanner::new(program, 0, program.len());
    let mut mentions = Vec::new();
    let mut shadowed = Shadowed::default();
    let mut opaque = false;
    let mut opaque_reason = None;
    while scanner.pos < scanner.end {
        let byte = scanner.byte_at(scanner.pos).unwrap_or(0);
        match byte {
            b'/' if scanner.byte_at(scanner.pos + 1) == Some(b'/') => {
                while let Some(c) = scanner.byte_at(scanner.pos) {
                    if c == b'\n' {
                        break;
                    }
                    scanner.pos += 1;
                }
            }
            b'/' if scanner.byte_at(scanner.pos + 1) == Some(b'*') => {
                scanner.pos += 2;
                loop {
                    if scanner.byte_at(scanner.pos) == Some(b'*')
                        && scanner.byte_at(scanner.pos + 1) == Some(b'/')
                    {
                        scanner.pos += 2;
                        break;
                    }
                    if scanner.byte_at(scanner.pos).is_none() {
                        opaque = true;
                        opaque_reason = Some("unterminated block comment".to_owned());
                        scanner.pos = scanner.end;
                        break;
                    }
                    scanner.pos += 1;
                }
            }
            b'\'' | b'"' => match scanner.scan_string(scanner.pos) {
                Some((_, end)) => scanner.pos = end,
                None => {
                    opaque = true;
                    opaque_reason = Some("unterminated string literal".to_owned());
                    scanner.pos = scanner.end;
                }
            },
            b'`' => match scanner.scan_template(scanner.pos) {
                Some((end, substitution)) => {
                    if substitution {
                        opaque = true;
                        opaque_reason = Some("template literal with substitution".to_owned());
                    }
                    scanner.pos = end;
                }
                None => {
                    opaque = true;
                    opaque_reason = Some("unterminated template literal".to_owned());
                    scanner.pos = scanner.end;
                }
            },
            b'(' => {
                // Arrow-function parameter list? `(… ) =>` shadows its
                // names inside the arrow body.
                if let Some(close) = scanner.matching_close(scanner.pos) {
                    let after = scanner.skip_trivia_from(close + 1);
                    let is_arrow = after.is_some_and(|index| {
                        scanner.byte_at(index) == Some(b'=')
                            && scanner.byte_at(index + 1) == Some(b'>')
                    });
                    if is_arrow {
                        shadow_idents_in(&mut scanner, scanner.pos + 1, close, &mut shadowed);
                        scanner.pos = close + 1;
                        continue;
                    }
                }
                scanner.pos += 1;
            }
            _ if is_ident_start(byte) => {
                let ident = scanner.read_ident();
                if KEYWORD_BINDERS.contains(&ident) {
                    scanner.pos = handle_binder(&mut scanner, ident, &mut shadowed);
                    continue;
                }
                if ident == "catch" {
                    scanner.pos = handle_catch(&mut scanner, &mut shadowed);
                    continue;
                }
                let Some(a) = scanner.skip_trivia_from(scanner.pos) else {
                    scanner.pos = scanner.end;
                    break;
                };
                if scanner.byte_at(a) == Some(b'.') {
                    // Member chain: `zero.<surface>.<method>(`, bare
                    // `<surface>.<method>(`, `z.invoke(`, `z.<member>(`.
                    let Some(m1) = scanner.skip_trivia_from(a + 1) else {
                        break;
                    };
                    if !is_ident_start(scanner.byte_at(m1).unwrap_or(0)) {
                        scanner.pos = m1;
                        continue;
                    }
                    scanner.pos = m1;
                    let member1 = scanner.read_ident();
                    let Some(a2) = scanner.skip_trivia_from(scanner.pos) else {
                        break;
                    };
                    if scanner.byte_at(a2) == Some(b'.') {
                        let Some(m2) = scanner.skip_trivia_from(a2 + 1) else {
                            break;
                        };
                        if !is_ident_start(scanner.byte_at(m2).unwrap_or(0)) {
                            scanner.pos = m2;
                            continue;
                        }
                        scanner.pos = m2;
                        let member2 = scanner.read_ident();
                        let Some(a3) = scanner.skip_trivia_from(scanner.pos) else {
                            break;
                        };
                        match scanner.byte_at(a3) {
                            Some(b'(') => {
                                if ident == "zero" && !shadowed.zero {
                                    if let Some(mention) = capture_mention(
                                        &mut scanner,
                                        a3,
                                        mention_args(&member1, &member2, true, false, false),
                                    ) {
                                        mentions.push(mention);
                                    }
                                    continue;
                                }
                                // Any other three-level member call
                                // (JSON.parse, chained helpers).
                                scanner.pos = a3 + 1;
                                continue;
                            }
                            Some(b'[') if ident == "zero" => {
                                opaque = true;
                                opaque_reason =
                                    Some("computed capability member access".to_owned());
                                scanner.pos = a3;
                                continue;
                            }
                            _ => {
                                scanner.pos = a3;
                                continue;
                            }
                        }
                    }
                    if scanner.byte_at(a2) == Some(b'(') {
                        if is_surface_name(&member1)
                            && !shadowed.surface_shadowed(&member1)
                            && !shadowed.zero
                        {
                            // Bare surface call without the zero root.
                            if let Some(mention) = capture_mention(
                                &mut scanner,
                                a2,
                                mention_args(&member1, ident, false, false, false),
                            ) {
                                mentions.push(mention);
                            }
                            continue;
                        }
                        if ident == "z" && !shadowed.z {
                            if member1 == "invoke" {
                                if let Some(mention) = capture_mention(
                                    &mut scanner,
                                    a2,
                                    mention_args("z", "invoke", false, true, false),
                                ) {
                                    mentions.push(mention);
                                }
                                continue;
                            }
                            if let Some(mention) = capture_mention(
                                &mut scanner,
                                a2,
                                mention_args("z", &member1, false, false, true),
                            ) {
                                mentions.push(mention);
                            }
                            continue;
                        }
                        // Plain two-level member call (JSON.parse, …).
                        scanner.pos = a2 + 1;
                        continue;
                    }
                    if scanner.byte_at(a2) == Some(b'[') && ident == "zero" {
                        opaque = true;
                        opaque_reason = Some("computed capability member access".to_owned());
                    }
                    scanner.pos = a2;
                    continue;
                }
                if scanner.byte_at(a) == Some(b'(') {
                    // Plain call (function application); not capability
                    // shaped.
                    scanner.pos = a + 1;
                    continue;
                }
                scanner.pos = a;
            }
            _ => scanner.advance_char(),
        }
    }
    PlanScan {
        mentions,
        opaque,
        opaque_reason,
    }
}

fn mention_args(
    surface: &str,
    method: &str,
    qualified: bool,
    invoke: bool,
    z_member: bool,
) -> CapabilityMention {
    CapabilityMention {
        qualified,
        invoke,
        z_member,
        surface: surface.to_owned(),
        method: method.to_owned(),
        first_arg: None,
        object_keys: Vec::new(),
        array_strings: Vec::new(),
        opaque_args: false,
    }
}

/// Capture the argument region of one mention and analyze it. The region is
/// bounded (`MAX_ARGS_BYTES`); a longer region marks the mention opaque.
fn capture_mention(
    scanner: &mut RegionScanner<'_>,
    open: usize,
    mut mention: CapabilityMention,
) -> Option<CapabilityMention> {
    let close = scanner.matching_close(open)?;
    let args_end = close.min(open.saturating_add(1).saturating_add(MAX_ARGS_BYTES));
    let analysis = analyze_args(scanner.source, open + 1, args_end);
    mention.first_arg = analysis.first_arg;
    mention.object_keys = analysis.keys;
    mention.array_strings = analysis.array_strings;
    mention.opaque_args = analysis.opaque || close > args_end;
    scanner.pos = close + 1;
    Some(mention)
}

struct ArgsAnalysis {
    first_arg: Option<String>,
    keys: Vec<ObjectKey>,
    array_strings: Vec<String>,
    opaque: bool,
}

/// Analyze one mention's argument region: the first positional string, the
/// outermost object members (with single-literal or array values), and the
/// strings of a first-level positional array.
fn analyze_args(source: &str, start: usize, end: usize) -> ArgsAnalysis {
    let mut scanner = RegionScanner::new(source, start, end);
    let mut depth = 0i32;
    let mut seen_object0 = false;
    let mut in_positional_array = false;
    let mut first_arg: Option<String> = None;
    let mut array_strings: Vec<String> = Vec::new();
    let mut keys: Vec<ObjectKey> = Vec::new();
    let mut current_key: Option<ObjectKey> = None;
    let mut expect_key = false;
    // Value state: 0 pending, 1 scalar string(s), 2 string array, 3 opaque.
    let mut value_mode: u8 = 0;
    let mut value_collect_depth: i32 = 0;
    let mut value_other = false;
    let mut opaque = false;
    while scanner.pos < scanner.end {
        let byte = scanner.byte_at(scanner.pos).unwrap_or(0);
        match byte {
            b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c => scanner.pos += 1,
            b'/' if scanner.byte_at(scanner.pos + 1) == Some(b'/') => {
                while let Some(c) = scanner.byte_at(scanner.pos) {
                    if c == b'\n' {
                        break;
                    }
                    scanner.pos += 1;
                }
            }
            b'/' if scanner.byte_at(scanner.pos + 1) == Some(b'*') => {
                scanner.pos += 2;
                loop {
                    if scanner.byte_at(scanner.pos) == Some(b'*')
                        && scanner.byte_at(scanner.pos + 1) == Some(b'/')
                    {
                        scanner.pos += 2;
                        break;
                    }
                    if scanner.byte_at(scanner.pos).is_none() {
                        scanner.pos = scanner.end;
                        break;
                    }
                    scanner.pos += 1;
                }
            }
            b'\'' | b'"' => match scanner.scan_string(scanner.pos) {
                Some((text, next)) => {
                    if text.len() <= MAX_STRING_BYTES {
                        if depth == 0 && !seen_object0 && !in_positional_array && first_arg.is_none()
                        {
                            first_arg = Some(text.clone());
                        }
                        if let Some(key) = current_key.as_mut() {
                            match value_mode {
                                0 => {
                                    // The value is exactly one string
                                    // literal so far.
                                    value_mode = 1;
                                    value_collect_depth = depth;
                                    if key.strings.len() < MAX_VALUE_STRINGS {
                                        key.strings.push(text);
                                    }
                                }
                                1 => {
                                    // More than one literal: not single.
                                    if depth == value_collect_depth
                                        && key.strings.len() < MAX_VALUE_STRINGS
                                    {
                                        key.strings.push(text);
                                    }
                                }
                                2 => {
                                    if depth == value_collect_depth
                                        && key.strings.len() < MAX_VALUE_STRINGS
                                    {
                                        key.strings.push(text);
                                    }
                                }
                                _ => {}
                            }
                        }
                        if in_positional_array && depth == 1 && array_strings.len() < MAX_ARRAY_STRINGS
                        {
                            array_strings.push(text);
                        }
                    }
                    scanner.pos = next;
                }
                None => {
                    opaque = true;
                    scanner.pos += 1;
                }
            },
            b'`' => {
                opaque = true;
                match scanner.scan_template(scanner.pos) {
                    Some((next, _)) => scanner.pos = next,
                    None => scanner.pos = scanner.end,
                }
            }
            b'{' => {
                if depth == 0 {
                    seen_object0 = true;
                }
                depth += 1;
                if depth == 1 {
                    expect_key = true;
                }
                if current_key.is_some() && value_mode == 0 {
                    value_mode = 3;
                } else if current_key.is_some() && value_mode != 0 {
                    value_other = true;
                }
                scanner.pos += 1;
            }
            b'}' => {
                depth -= 1;
                if depth <= 1 {
                    if let Some(key) = current_key.take() {
                        keys.push(finish_key(key, value_mode, value_other));
                    }
                    value_mode = 0;
                    value_other = false;
                    if depth == 0 {
                        expect_key = false;
                    }
                }
                scanner.pos += 1;
            }
            b'[' => {
                if depth == 0 && !seen_object0 {
                    in_positional_array = true;
                }
                depth += 1;
                if current_key.is_some() && value_mode == 0 {
                    value_mode = 2;
                    value_collect_depth = depth;
                } else if current_key.is_some() && value_mode != 0 {
                    value_other = true;
                }
                scanner.pos += 1;
            }
            b']' => {
                depth -= 1;
                if depth == 0 {
                    in_positional_array = false;
                }
                if current_key.is_some() && value_mode != 0 {
                    value_other = true;
                }
                scanner.pos += 1;
            }
            b'(' => {
                depth += 1;
                if current_key.is_some() && value_mode == 0 {
                    value_mode = 3;
                } else if current_key.is_some() && value_mode != 0 {
                    value_other = true;
                }
                scanner.pos += 1;
            }
            b')' => {
                depth = (depth - 1).max(0);
                if current_key.is_some() && value_mode != 0 {
                    value_other = true;
                }
                scanner.pos += 1;
            }
            b',' => {
                if depth == 1 {
                    if let Some(key) = current_key.take() {
                        keys.push(finish_key(key, value_mode, value_other));
                    }
                    value_mode = 0;
                    value_other = false;
                    expect_key = true;
                }
                scanner.pos += 1;
            }
            _ if is_ident_start(byte) => {
                let name = scanner.read_ident();
                if depth == 1 && expect_key {
                    let after = scanner.skip_trivia_from(scanner.pos);
                    if after.is_some_and(|index| scanner.byte_at(index) == Some(b':')) {
                        current_key = Some(ObjectKey {
                            key: name.to_owned(),
                            single: None,
                            strings: Vec::new(),
                        });
                        value_mode = 0;
                        value_other = false;
                        scanner.pos = after.expect("checked above") + 1;
                        continue;
                    }
                    // Shorthand member `{path}`: no value analysis.
                    keys.push(ObjectKey {
                        key: name.to_owned(),
                        single: None,
                        strings: Vec::new(),
                    });
                    expect_key = false;
                    continue;
                }
                if current_key.is_some() && value_mode == 0 {
                    value_mode = 3;
                } else if current_key.is_some() && value_mode != 0 {
                    value_other = true;
                }
                scanner.pos = scanner.skip_trivia_from(scanner.pos).unwrap_or(scanner.pos);
            }
            b'0'..=b'9' => {
                if current_key.is_some() && value_mode == 0 {
                    value_mode = 3;
                } else if current_key.is_some() && value_mode != 0 {
                    value_other = true;
                }
                scanner.pos += 1;
            }
            _ => {
                if current_key.is_some() && value_mode == 0 {
                    // Any other value start (`+`, `.`, `-`, …) is opaque.
                    value_mode = 3;
                } else if current_key.is_some() && value_mode == 1 {
                    // A significant non-string token inside a scalar value
                    // (for example `'/a' + '/b'`) breaks the single-literal
                    // claim; array elements in mode 2 keep collecting.
                    value_other = true;
                }
                scanner.advance_char();
            }
        }
    }
    if let Some(key) = current_key.take() {
        keys.push(finish_key(key, value_mode, value_other));
    }
    ArgsAnalysis {
        first_arg,
        keys,
        array_strings,
        opaque,
    }
}

/// Compute `single` for one finished value: exactly one string literal and
/// no other significant token.
fn finish_key(mut key: ObjectKey, value_mode: u8, value_other: bool) -> ObjectKey {
    if value_mode == 1 && !value_other && key.strings.len() == 1 {
        key.single = key.strings.first().cloned();
    }
    key
}

/// Shadow any of the tracked names appearing inside a binding region
/// (arrow parameters, catch parameters, destructuring bindings).
fn shadow_idents_in(
    scanner: &RegionScanner<'_>,
    start: usize,
    end: usize,
    shadowed: &mut Shadowed,
) {
    let mut inner = RegionScanner::new(scanner.source, start, end);
    while inner.pos < inner.end {
        let byte = inner.byte_at(inner.pos).unwrap_or(0);
        match byte {
            b'\'' | b'"' => match inner.scan_string(inner.pos) {
                Some((_, next)) => inner.pos = next,
                None => break,
            },
            b'`' => match inner.scan_template(inner.pos) {
                Some((next, _)) => inner.pos = next,
                None => break,
            },
            _ if is_ident_start(byte) => {
                let name = inner.read_ident();
                shadowed.mark(name);
            }
            _ => inner.advance_char(),
        }
    }
}

/// Handle `const`/`let`/`var`/`function`/`class` binders: shadow the bound
/// name (and function parameters) so later identifier uses are not mistaken
/// for capability surfaces.
fn handle_binder(scanner: &mut RegionScanner<'_>, keyword: &str, shadowed: &mut Shadowed) -> usize {
    let Some(mut index) = scanner.skip_trivia_from(scanner.pos) else {
        return scanner.end;
    };
    let byte = scanner.byte_at(index).unwrap_or(0);
    if is_ident_start(byte) {
        scanner.pos = index;
        let name = scanner.read_ident();
        shadowed.mark(name);
        index = scanner.pos;
    }
    if keyword == "function" {
        if let Some(open) = scanner.skip_trivia_from(index)
            && scanner.byte_at(open) == Some(b'(')
            && let Some(close) = scanner.matching_close(open)
        {
            shadow_idents_in(scanner, open + 1, close, shadowed);
            return close + 1;
        }
    }
    let byte = scanner.byte_at(index).unwrap_or(0);
    if byte == b'{' || byte == b'[' {
        // Destructuring binding: shadow any tracked names inside.
        let Some(close) = scanner.matching_close(index) else {
            scanner.pos = scanner.end;
            return scanner.pos;
        };
        shadow_idents_in(scanner, index + 1, close, shadowed);
        return close + 1;
    }
    index
}

/// Handle `catch (name)`: shadow the caught binding.
fn handle_catch(scanner: &mut RegionScanner<'_>, shadowed: &mut Shadowed) -> usize {
    let Some(index) = scanner.skip_trivia_from(scanner.pos) else {
        return scanner.end;
    };
    if scanner.byte_at(index) == Some(b'(')
        && let Some(close) = scanner.matching_close(index)
    {
        shadow_idents_in(scanner, index + 1, close, shadowed);
        return close + 1;
    }
    index
}
