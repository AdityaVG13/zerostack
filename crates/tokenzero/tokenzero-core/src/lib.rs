#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::fmt;
use std::path::Path;

pub const CLI_SCHEMA_VERSION: &str = "tokenzero.cli.v1";
pub const MCP_SCHEMA_VERSION: &str = "tokenzero.mcp.v1";
pub const INSTALL_SCHEMA_VERSION: &str = "tokenzero.install_plan.v1";
pub const PULSE_SCHEMA_VERSION: &str = "tokenzero.pulse.v1";

macro_rules! string_enum {
    ($(#[$meta:meta])* $vis:vis enum $name:ident, $as_vis:vis as_str {
        $($(#[$variant_meta:meta])* $variant:ident => $text:literal),+ $(,)?
    }) => {
        $(#[$meta])*
        $vis enum $name { $($(#[$variant_meta])* $variant),+ }

        impl $name {
            $as_vis fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum! {
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum Mode, as_str {
        #[default] Auto => "auto",
        Passthrough => "passthrough",
        Diagnostic => "diagnostic",
        Structured => "structured",
        Dedupe => "dedupe",
        DiffAware => "diff-aware",
        Exact => "exact",
        Lossy => "lossy",
        Hybrid => "hybrid",
        Critical => "critical",
        Fidelity => "fidelity",
    }
}

impl std::str::FromStr for Mode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        const MAP: &[(&[&str], Mode)] = &[
            (&["auto", "hybrid"], Mode::Auto),
            (&["passthrough"], Mode::Passthrough),
            (&["diagnostic", "critical"], Mode::Diagnostic),
            (&["structured", "fidelity"], Mode::Structured),
            (&["dedupe"], Mode::Dedupe),
            (&["diff-aware", "diff_aware", "diffaware"], Mode::DiffAware),
            (&["exact"], Mode::Exact),
            (&["lossy"], Mode::Lossy),
        ];
        MAP.iter()
            .find(|(aliases, _)| aliases.contains(&s))
            .map(|(_, m)| *m)
            .ok_or_else(|| format!("unsupported mode: {s}"))
    }
}

impl Mode {
    pub fn effective_policy(self) -> Self {
        match self {
            Self::Hybrid => Self::Auto,
            Self::Critical => Self::Diagnostic,
            Self::Fidelity | Self::Lossy => Self::Structured,
            other => other,
        }
    }
}

string_enum! {
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum McpToolSurface, pub as_str {
        #[default] Classic => "mcp",
        CodeMode => "codemode",
    }
}

impl McpToolSurface {
    pub const ENV: &'static str = "TOKENZERO_MCP_TOOL_SURFACE";
}

impl std::str::FromStr for McpToolSurface {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s
            .trim()
            .to_ascii_lowercase()
            .replace(['_', ' '], "-")
            .as_str()
        {
            "" | "mcp" | "classic" | "aliases" | "full" => Ok(Self::Classic),
            "codemode" | "code-mode" => Ok(Self::CodeMode),
            other => Err(format!(
                "unsupported MCP launch mode '{other}'; use mcp or codemode"
            )),
        }
    }
}

string_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ContentType, as_str {
        Code => "code",
        ShellOutput => "shell_output",
        SearchResult => "search_result",
        Tree => "tree",
        Diff => "diff",
        JsonConfig => "json_config",
        Markdown => "markdown",
        Logs => "logs",
        Unknown => "unknown",
    }
}

/// Returns true if `haystack` starts with any pipe-delimited prefix.
pub(crate) fn starts_with_any(h: &str, p: &str) -> bool {
    p.split('|').any(|n| h.starts_with(n))
}

/// Returns true if `haystack` ends with any pipe-delimited suffix.
pub(crate) fn ends_with_any(h: &str, p: &str) -> bool {
    p.split('|').any(|n| h.ends_with(n))
}

/// Returns true if `haystack` contains any pipe-delimited needle.
pub(crate) fn contains_any(h: &str, p: &str) -> bool {
    p.split('|').any(|n| h.contains(n))
}

/// Returns true if `haystack` contains any whitespace-delimited needle.
pub(crate) fn contains_any_ws(h: &str, n: &str) -> bool {
    n.split_whitespace().any(|w| h.contains(w))
}

pub(crate) fn is_one_of(value: &str, choices: &str) -> bool {
    choices.split_whitespace().any(|choice| value == choice)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Visible {
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefRecord {
    pub kind: String,
    #[serde(rename = "ref")]
    pub ref_id: String,
    pub bytes: usize,
    pub live: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Accounting {
    pub raw_tokens: usize,
    pub visible_tokens: usize,
    pub recovery_tokens: usize,
    /// Output tokens billed at the tool boundary. Defaults preserve older records.
    #[serde(default)]
    pub billed_tokens: usize,
    /// Billed output tokens satisfied by the measured cache source.
    #[serde(default)]
    pub cached_tokens: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_ref_tokens: Option<usize>,
    /// Kernel measure label for these counts: `estimator:` or `tiktoken:`.
    /// Never unlabeled, never an invented ExactTokenizerIdentity.
    #[serde(default = "tokens::count_tokens_tokenizer_id")]
    pub tokenizer_id: String,
    /// True only for a single `provider/model@hex` identity. Estimator and
    /// tiktoken MCP totals never certify as exact.
    #[serde(default)]
    pub certified: bool,
}

impl Default for Accounting {
    fn default() -> Self {
        Self::measured(0, 0, 0, 0, 0, None)
    }
}

impl Accounting {
    /// MCP/engine accounting block stamped with the kernel measure estimator.
    pub fn measured(
        raw_tokens: usize,
        visible_tokens: usize,
        recovery_tokens: usize,
        billed_tokens: usize,
        cached_tokens: usize,
        exact_ref_tokens: Option<usize>,
    ) -> Self {
        let mut accounting = Self {
            raw_tokens,
            visible_tokens,
            recovery_tokens,
            billed_tokens,
            cached_tokens,
            exact_ref_tokens,
            tokenizer_id: tokens::count_tokens_tokenizer_id(),
            certified: false,
        };
        accounting.stamp_tokenizer();
        accounting
    }

    /// spent = visible + recovery. Expand charges belong here.
    pub fn spent_tokens(&self) -> usize {
        self.visible_tokens.saturating_add(self.recovery_tokens)
    }

    /// Recovered mass on this response (recovery_tokens).
    pub fn recovered_tokens(&self) -> usize {
        self.recovery_tokens
    }

    /// Stamp `tokenizer_id` from the kernel measure estimator. Refuses
    /// unlabeled `estimate:` / empty / Q99 by replacing them. Never invents
    /// ExactTokenizerIdentity; estimator and tiktoken stay uncertified.
    pub fn stamp_tokenizer(&mut self) {
        let id = self.tokenizer_id.trim();
        // MCP registry labels are identity collisions, not unlabeled estimates.
        // Keep them so Pulse/preflight refuse instead of relabeling to the kernel
        // estimator (which would look like honest accounting).
        if tokens::is_forbidden_mcp_tokenizer_identity(id) {
            self.certified = false;
            return;
        }
        let labeled = id.starts_with("estimator:")
            || id.starts_with("tiktoken:")
            || (id.contains('/') && id.contains('@'));
        if id.is_empty() || !labeled || tokens::preflight_tokenizer_id(id).is_err() {
            self.tokenizer_id = tokens::count_tokens_tokenizer_id();
        }
        if self.tokenizer_id.starts_with("estimator:") || self.tokenizer_id.starts_with("tiktoken:")
        {
            self.certified = false;
        }
    }

    pub fn visible_savings_ratio(&self) -> f64 {
        savings_ratio(self.raw_tokens, self.visible_tokens)
    }
    /// M_rec used-tokens are `visible + recovery` (saturating).
    ///
    /// Exact-expand payloads that also appear in `visible_tokens` are counted
    /// in both on purpose: the hub zero-ledger receipt treats that overlap as
    /// conservative (understates savings). `used` is "shown or recovered",
    /// not a partition of disjoint masses (tokenzero-73yc). Signed: spent>raw
    /// is a negative ratio, not a clamped 0% save.
    pub fn recovery_adjusted_savings_ratio(&self) -> f64 {
        savings_ratio(self.raw_tokens, self.spent_tokens())
    }
}

impl Serialize for Accounting {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut fields = 9;
        if self.exact_ref_tokens.is_some() {
            fields += 1;
        }
        let mut state = serializer.serialize_struct("Accounting", fields)?;
        state.serialize_field("raw_tokens", &self.raw_tokens)?;
        state.serialize_field("visible_tokens", &self.visible_tokens)?;
        state.serialize_field("recovery_tokens", &self.recovery_tokens)?;
        state.serialize_field("billed_tokens", &self.billed_tokens)?;
        state.serialize_field("cached_tokens", &self.cached_tokens)?;
        if let Some(exact) = self.exact_ref_tokens {
            state.serialize_field("exact_ref_tokens", &exact)?;
        }
        state.serialize_field("tokenizer_id", &self.tokenizer_id)?;
        state.serialize_field("certified", &self.certified)?;
        state.serialize_field("spent_tokens", &self.spent_tokens())?;
        state.serialize_field("recovered_tokens", &self.recovered_tokens())?;
        state.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repair: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repair: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolResponse {
    pub schema_version: String,
    pub status: String,
    pub tool: String,
    /// ACK/2 one-token class atom. Pure mutation success is silent (None).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ack: Option<String>,
    /// Expandable detail ref for the response body when one is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible: Option<Visible>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<RefRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accounting: Option<Accounting>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<Diagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CliError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety: Option<serde_json::Value>,
    /// vz89.11 output channel separation: present only when the harness opted
    /// in (TOKENZERO_CHANNEL_SEPARATION). Absent means byte-identical default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<ChannelSeparation>,
    /// Recovery receipt marking terminal (do-not-recompact) exact-byte
    /// recovery. Present only on expand-family responses that return stored
    /// bytes verbatim; adapters must not re-compact or re-summarize the
    /// visible body of a response carrying this receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<RecoveryReceipt>,
    /// CacheZero would-be outcome. Never implies the body was served from cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_status: Option<String>,
    /// Tokens that a hit would have avoided showing. Zero on forced-miss.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_tokens_estimate: Option<u64>,
    /// Leftover visible-token budget after this result. Distinct from
    /// accounting so agents can adapt near exhaustion without parsing notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_budget_tokens: Option<u64>,
    /// True when a scan or render stopped on a budget. A zero-hit with this
    /// set is not a proven miss -- retry with a larger budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_exhausted: Option<bool>,
}

/// Terminal-recovery marker for adapter compaction pipelines (yevj).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryReceipt {
    /// True when this response IS the recovered content: re-running it
    /// through compaction would destroy the bytes the agent paid to recover.
    pub terminal: bool,
    /// Adapter contract: never re-compact the visible body.
    pub do_not_recompact: bool,
    /// True when the visible body is byte-exact recovered content.
    pub exact_bytes: bool,
}

/// Machine-action channel separated from user-facing prose (hub vz89.11).
/// The harness renders `status_line` deterministically at zero model-output
/// cost; `user_message` stays null between tool calls and may carry one brief
/// final explanation at completion.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChannelSeparation {
    /// Machine-readable action atom (canonical op name, e.g. "read").
    pub action: String,
    /// Deterministic status line derivable from the operation + receipt.
    pub status_line: String,
    /// Nullable by contract: None serializes as an explicit null.
    pub user_message: Option<String>,
}

/// Env var opting a harness into channel-separated responses.
pub const CHANNEL_SEPARATION_ENV: &str = "TOKENZERO_CHANNEL_SEPARATION";

/// How much of the channel contract the harness opted into (vz89.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMode {
    /// No channel block; responses stay byte-identical to the pre-gate contract.
    Off,
    /// Machine action + deterministic status line, `user_message` always null.
    /// The between-tool-calls mode: no model narration is paid for.
    Action,
    /// Action mode plus one brief receipt-derived `user_message` on a terminal
    /// envelope. Still zero model-output cost: the text comes from receipts.
    Terminal,
}

impl ChannelMode {
    pub fn from_env_value(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "on" | "true" | "yes" | "action" => Self::Action,
            "terminal" | "final" => Self::Terminal,
            _ => Self::Off,
        }
    }

    pub fn enabled(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Whether a terminal envelope may carry a receipt-derived user message.
    pub fn emits_user_message(self) -> bool {
        matches!(self, Self::Terminal)
    }
}

/// The channel mode the harness opted into. Default `Off`.
pub fn channel_mode() -> ChannelMode {
    std::env::var(CHANNEL_SEPARATION_ENV)
        .map(|raw| ChannelMode::from_env_value(&raw))
        .unwrap_or(ChannelMode::Off)
}

/// Whether the harness opted into channel separation (vz89.11). Default off:
/// responses are byte-identical to the pre-gate contract.
pub fn channel_separation_enabled() -> bool {
    channel_mode().enabled()
}

impl ToolResponse {
    fn base(status: &str, tool: impl Into<String>) -> Self {
        Self {
            schema_version: CLI_SCHEMA_VERSION.to_string(),
            status: status.to_string(),
            tool: tool.into(),
            ..Self::default()
        }
    }

    pub fn ok(
        tool: impl Into<String>,
        mode: Mode,
        visible: String,
        refs: Vec<RefRecord>,
        mut accounting: Accounting,
    ) -> Self {
        accounting.stamp_tokenizer();
        Self {
            ack: Some(AckClass::Success.atom().to_string()),
            detail_ref: refs.first().map(|record| record.ref_id.clone()),
            mode: Some(mode.to_string()),
            visible: Some(Visible {
                kind: "capsule".to_string(),
                text: visible,
            }),
            refs,
            accounting: Some(accounting),
            ..Self::base("ok", tool)
        }
    }

    pub fn error(
        tool: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
        repair: Option<String>,
    ) -> Self {
        let code = code.into();
        let ack = AckClass::from_error_kind(&code, false).atom().to_string();
        Self {
            ack: Some(ack),
            error: Some(CliError {
                code,
                message: message.into(),
                repair,
            }),
            ..Self::base("error", tool)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LossySpan {
    pub description: String,
    pub reason: String,
    pub recovery_may_be_needed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capsule {
    pub text: String,
    pub raw_tokens: usize,
    pub visible_tokens: usize,
    pub omitted_lines: usize,
    pub mode: Mode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protected_anchors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exact_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lossy_spans: Vec<LossySpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lossy_policy_id: Option<String>,
}

impl Capsule {
    /// Enforce RACC omission rule: transformed bytes require an exact selector,
    /// a visible protected anchor, or an explicit lossy declaration.
    pub fn validate_omission_rule(&self, original: &str) -> Result<(), String> {
        let original = original.trim_end();
        if original.is_empty() || self.text.contains(original) {
            return Ok(());
        }
        if self
            .protected_anchors
            .iter()
            .any(|anchor| !anchor.is_empty() && self.text.contains(&format!("[[anchor:{anchor}]]")))
        {
            return Ok(());
        }
        if self
            .exact_refs
            .iter()
            .any(|reference| exact_ref_has_selector(reference) && self.text.contains(reference))
        {
            return Ok(());
        }
        let lossy_declared = self.mode == Mode::Lossy
            && self
                .lossy_policy_id
                .as_ref()
                .is_some_and(|id| !id.is_empty())
            && !self.lossy_spans.is_empty()
            && self.lossy_spans.iter().all(|span| {
                !span.description.is_empty()
                    && !span.reason.is_empty()
                    && span.recovery_may_be_needed
            })
            && self.text.contains("mode=lossy")
            && self.text.contains("lossy_policy_id=");
        lossy_declared.then_some(()).ok_or_else(|| {
            "capsule omitted bytes without a protected anchor, exact tz:// selector, or explicit lossy declaration".to_string()
        })
    }
}

fn exact_recovery_scheme(reference: &str) -> bool {
    let base = reference
        .split_once('#')
        .map_or(reference, |(base, _)| base);
    // Recovery expand treats fz://blob and gz://blob as same-store aliases of tz://.
    base.starts_with("z://blob/")
        || base.starts_with("tz://")
        || base.starts_with("fz://blob/")
        || base.starts_with("gz://blob/")
}

fn exact_ref_has_selector(reference: &str) -> bool {
    let Some((base, selector)) = reference.split_once('#') else {
        return false;
    };
    if !exact_recovery_scheme(base) || selector.is_empty() {
        return false;
    }
    // Same published grammar expand accepts: `#Bstart-end`, `#Bstart+len`,
    // `#Bn`, `#Lstart-end`, `#Lstart-Lend`, `#Ln`. A recovery cue the
    // expander cannot parse is not a selector.
    if let Some(bytes) = selector.strip_prefix('B') {
        if let Some((start, end)) = bytes.split_once('-') {
            return start.parse::<usize>().is_ok() && end.parse::<usize>().is_ok();
        }
        if let Some((start, len)) = bytes.split_once('+') {
            return start.parse::<usize>().is_ok() && len.parse::<usize>().is_ok();
        }
        return bytes.parse::<usize>().is_ok();
    }
    if let Some(lines) = selector.strip_prefix('L') {
        if let Some((start, end)) = lines.split_once('-') {
            return start.parse::<usize>().is_ok()
                && end.trim_start_matches('L').parse::<usize>().is_ok();
        }
        return lines.parse::<usize>().is_ok();
    }
    selector
        .strip_prefix("symbol=")
        .is_some_and(|symbol| !symbol.is_empty())
}

fn exact_recovery_ref(reference: &str, byte_len: usize) -> Option<String> {
    exact_recovery_scheme(reference).then(|| {
        if reference.contains('#') {
            reference.to_string()
        } else {
            format!("{reference}#B0-{byte_len}")
        }
    })
}

fn validated_capsule(capsule: Capsule, original: &str) -> Result<Capsule, String> {
    capsule.validate_omission_rule(original)?;
    Ok(capsule)
}

fn finalize_capsule_omission(
    mut capsule: Capsule,
    original: &str,
    max_visible_tokens: usize,
    exact_ref: Option<String>,
) -> Result<Capsule, String> {
    let original_trimmed = original.trim_end();
    let omitted = !original_trimmed.is_empty() && !capsule.text.contains(original_trimmed);
    if omitted {
        if let Some(reference) = exact_ref.filter(|value| exact_ref_has_selector(value)) {
            // validate_omission_rule requires the selector to be present in the
            // VISIBLE TEXT, not merely recorded in exact_refs: a ref an agent
            // cannot see is a ref it cannot expand, so recording it alone would
            // satisfy the struct while still stranding the omitted bytes.
            // Without this the branch panicked on any budgeted read whose
            // enforce_token_budget_with_ref marker had already been trimmed.
            if !capsule.text.contains(&reference) {
                capsule.text.push('\n');
                capsule.text.push_str(&format!(
                    "... omitted by visible budget; expand {reference} for the full output ..."
                ));
                capsule.visible_tokens = count_tokens(&capsule.text);
            }
            capsule.exact_refs.push(reference);
        } else {
            let mut declared = capsule.text.clone();
            if !declared.contains("mode=lossy") {
                declared.push('\n');
                declared.push_str(VISIBLE_BUDGET_LOSSY_DECLARATION);
            }
            let declared_tokens = count_tokens(&declared);
            let raw_full_tokens = count_tokens(original_trimmed);
            if declared_tokens >= raw_full_tokens {
                // Inflation guard: a lossy declaration plus summary that costs
                // more than the raw payload is not a compression. Exact mode is
                // not exempt — hiding one token behind a 10-token stub is still
                // worse than the raw payload.
                capsule.text = original_trimmed.to_string();
                capsule.visible_tokens = raw_full_tokens;
                capsule.omitted_lines = 0;
            } else {
                capsule.mode = Mode::Lossy;
                capsule.lossy_policy_id = Some("tokenzero.visible-compression.v1".to_string());
                capsule.lossy_spans.push(LossySpan {
                    description: "bytes omitted from the visible capsule".to_string(),
                    reason: "visible token budget or selected compression policy".to_string(),
                    recovery_may_be_needed: true,
                });
                capsule.text = enforce_token_budget_with_ref(&declared, max_visible_tokens, None);
                capsule.visible_tokens = count_tokens(&capsule.text);
            }
        }
    }
    validated_capsule(apply_never_worse_passthrough(capsule, original), original)
}

/// A capsule whose visible cost exceeds the raw payload is not a save.
/// Emit the payload itself so callers cannot report spent>raw as compression.
fn apply_never_worse_passthrough(mut capsule: Capsule, original: &str) -> Capsule {
    let raw_text = original.trim_end();
    let raw_count = count_tokens(raw_text);
    if raw_count < capsule.visible_tokens {
        capsule.text = raw_text.to_string();
        capsule.visible_tokens = raw_count;
        capsule.omitted_lines = 0;
        capsule.exact_refs.clear();
        capsule.lossy_spans.clear();
        capsule.lossy_policy_id = None;
        capsule.mode = Mode::Passthrough;
    }
    capsule
}

pub fn make_capsule(
    text: &str,
    mode: Mode,
    max_visible_tokens: usize,
    label: Option<&str>,
) -> Result<Capsule, String> {
    let raw_tokens = count_tokens(text);
    make_capsule_with_raw_tokens(text, raw_tokens, mode, max_visible_tokens, label)
}

pub fn make_capsule_with_raw_tokens(
    text: &str,
    raw_tokens: usize,
    mode: Mode,
    max_visible_tokens: usize,
    label: Option<&str>,
) -> Result<Capsule, String> {
    make_capsule_with_recovery_ref(text, raw_tokens, mode, max_visible_tokens, label, None)
}

/// Adds an inline exact-ref recovery cue to a token-budgeted capsule.
pub fn make_capsule_with_recovery_ref(
    text: &str,
    raw_tokens: usize,
    mode: Mode,
    max_tokens: usize,
    label: Option<&str>,
    recovery_ref: Option<&str>,
) -> Result<Capsule, String> {
    let prefix = capsule_prefix(label, max_tokens, raw_tokens);
    let exact_ref = recovery_ref.and_then(|reference| exact_recovery_ref(reference, text.len()));
    let policy = mode.effective_policy();
    let mut visible = match policy {
        Mode::Exact => format!("{prefix}[exact payload stored; use expand for raw bytes]"),
        Mode::Passthrough => format!("{prefix}{}", text.trim_end()),
        Mode::Diagnostic => match error_block(text, 3) {
            b if b.trim().is_empty() => summarize_lines(text, 8, 6, &prefix),
            b => format!("{prefix}{}", b.trim_end()),
        },
        Mode::Structured => summarize_lines(text, 24, 16, &prefix),
        Mode::Dedupe => format!("{prefix}{}", dedupe_lines(text, 8).trim_end()),
        Mode::DiffAware => format!("{prefix}{}", diff_summary(text, 120).trim_end()),
        Mode::Auto if max_tokens == 0 || raw_tokens <= max_tokens => {
            format!("{prefix}{}", text.trim_end())
        }
        Mode::Auto => summarize_lines(text, 18, 12, &prefix),
        _ => unreachable!(),
    };
    if policy != Mode::Passthrough {
        visible = enforce_token_budget_with_ref(&visible, max_tokens, exact_ref.as_deref());
    }
    let mut visible_tokens = count_tokens(&visible);
    let mut mode = mode;
    if visible_tokens > raw_tokens {
        let fallback = text.trim_end().to_string();
        let fallback_tokens = count_tokens(&fallback);
        if fallback_tokens < visible_tokens {
            visible_tokens = fallback_tokens;
            visible = fallback;
            mode = Mode::Passthrough;
        }
    }
    finalize_capsule_omission(
        Capsule {
            visible_tokens,
            raw_tokens,
            omitted_lines: text.lines().count().saturating_sub(visible.lines().count()),
            text: visible,
            mode,
            protected_anchors: Vec::new(),
            exact_refs: Vec::new(),
            lossy_spans: Vec::new(),
            lossy_policy_id: None,
        },
        text,
        max_tokens,
        exact_ref,
    )
}

/// Creates a domain-aware summary with byte-exact recovery via `recovery_ref`.
pub fn make_capsule_content_aware(
    text: &str,
    raw_tokens: usize,
    content_type: ContentType,
    max_visible_tokens: usize,
    label: Option<&str>,
    recovery_ref: Option<&str>,
    aggressive: bool,
) -> Result<Capsule, String> {
    if !aggressive && (max_visible_tokens == 0 || raw_tokens <= max_visible_tokens) {
        return make_capsule_with_recovery_ref(
            text,
            raw_tokens,
            Mode::Auto,
            max_visible_tokens,
            label,
            recovery_ref,
        );
    }
    let prefix = capsule_prefix(label, max_visible_tokens, raw_tokens);
    let exact_ref = recovery_ref.and_then(|reference| exact_recovery_ref(reference, text.len()));
    let budget = if aggressive {
        max_visible_tokens / 3
    } else {
        max_visible_tokens
    };
    let visible = match content_type {
        ContentType::Code => summarize_code(text, budget, &prefix),
        ContentType::Logs | ContentType::ShellOutput => summarize_logs(text, budget, &prefix),
        ContentType::JsonConfig => summarize_json(text, budget, &prefix),
        ContentType::Diff => summarize_lines(text, 12, 8, &prefix),
        ContentType::SearchResult => summarize_lines(text, 20, 5, &prefix),
        _ => summarize_lines(text, 18, 12, &prefix),
    };
    let visible = enforce_token_budget_with_ref(&visible, max_visible_tokens, exact_ref.as_deref());
    let visible_tokens = count_tokens(&visible);
    finalize_capsule_omission(
        Capsule {
            omitted_lines: text.lines().count().saturating_sub(visible.lines().count()),
            text: visible,
            raw_tokens,
            visible_tokens,
            mode: if aggressive { Mode::Exact } else { Mode::Auto },
            protected_anchors: Vec::new(),
            exact_refs: Vec::new(),
            lossy_spans: Vec::new(),
            lossy_policy_id: None,
        },
        text,
        max_visible_tokens,
        exact_ref,
    )
}

/// Summarize code: show first N lines (imports/signatures) + last M lines.
const CODE_SIG_PREFIXES: &str =
    "pub |fn |struct |enum |impl |trait |class |def |function |export |import |use |#[";

fn push_labeled_lines(out: &mut String, label: &str, lines: &[&str], limit: usize) {
    if lines.is_empty() {
        return;
    }
    out.push_str(label);
    for line in lines.iter().take(limit) {
        out.push_str(line);
        out.push('\n');
    }
}

fn summarize_code(text: &str, budget_tokens: usize, prefix: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    if total <= 30 {
        return format!("{prefix}{}", text.trim_end());
    }
    let sigs: Vec<&str> = lines
        .iter()
        .take(total.min(80))
        .filter(|l| starts_with_any(l.trim(), CODE_SIG_PREFIXES))
        .copied()
        .collect();
    let head = 8.min(total);
    let tail = 6.min(total.saturating_sub(head));
    let mut out = format!("{prefix}{}", lines[..head].join("\n"));
    push_labeled_lines(
        &mut out,
        "\n\n# declarations/signatures:\n",
        &sigs,
        budget_tokens / 8,
    );
    out.push_str(&omitted_lines_marker(total.saturating_sub(head + tail)));
    out + &lines[total - tail..].join("\n")
}

/// Summarize logs: prioritize errors/warnings, then head+tail.
const LOG_ERROR_NEEDLES: &str = "error fatal panic failed traceback";

fn summarize_logs(text: &str, budget_tokens: usize, prefix: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let (mut errs, mut warns) = (Vec::new(), Vec::new());
    for l in &lines {
        let low = l.to_ascii_lowercase();
        if contains_any_ws(&low, LOG_ERROR_NEEDLES) {
            errs.push(*l);
        } else if low.contains("warn") {
            warns.push(*l);
        }
    }
    let mut out = prefix.to_string();
    let limit = budget_tokens / 6;
    push_labeled_lines(
        &mut out,
        &format!("# {} error(s):\n", errs.len()),
        &errs,
        limit,
    );
    push_labeled_lines(
        &mut out,
        &format!("# {} warning(s):\n", warns.len()),
        &warns,
        limit / 2,
    );
    if errs.is_empty() && warns.is_empty() {
        let head = 6.min(lines.len());
        let tail = 4.min(lines.len().saturating_sub(head));
        out.push_str(&lines[..head].join("\n"));
        if lines.len() > head + tail {
            out.push_str(&format!(
                "\n... omitted {} lines ...\n",
                lines.len().saturating_sub(head + tail)
            ));
        }
        if tail > 0 {
            out.push_str(&lines[lines.len() - tail..].join("\n"));
        }
    } else {
        out.push_str(&format!(
            "# {} total lines; exact ref available",
            lines.len()
        ));
    }
    out
}

/// Summarize JSON: show schema shape (keys, types, array lengths).
fn summarize_json(text: &str, _budget_tokens: usize, prefix: &str) -> String {
    let mut out = prefix.to_string();
    match serde_json::from_str::<serde_json::Value>(text.trim()) {
        Ok(serde_json::Value::Object(map)) => {
            out.push_str(&format!("json_object: {} keys\n", map.len()));
            for (key, val) in map.iter().take(25) {
                let kind = match val {
                    serde_json::Value::String(s) if s.len() > 100 => "string(long)",
                    serde_json::Value::Array(a) if a.is_empty() => "array(0)",
                    serde_json::Value::Object(o) if o.is_empty() => "object(0)",
                    other => json_kind(other),
                };
                out.push_str(&format!("  {key}: {kind}\n"));
            }
        }
        Ok(serde_json::Value::Array(items)) => {
            out.push_str(&format!("json_array: {} items\n", items.len()));
            if let Some(first) = items.first() {
                let sample: String = serde_json::to_string(first)
                    .unwrap_or_default()
                    .chars()
                    .take(200)
                    .collect();
                out.push_str(&format!("  sample: {sample}\n"));
            }
        }
        _ => return summarize_lines(text, 12, 8, prefix),
    }
    out + "# exact ref available for full content"
}

pub fn summarize_lines(text: &str, head: usize, tail: usize, prefix: &str) -> String {
    let lines: Vec<_> = text.lines().collect();
    // Saturate: `head = usize::MAX` must keep the whole text, not wrap and panic
    // on `lines[..head]`.
    if lines.len() <= head.saturating_add(tail).saturating_add(3) {
        return format!("{prefix}{}", text.trim_end());
    }
    format!(
        "{prefix}{}\n\n... omitted {} lines; exact ref available ...\n\n{}",
        lines[..head].join("\n"),
        lines.len().saturating_sub(head.saturating_add(tail)),
        lines[lines.len() - tail..].join("\n"),
    )
}

fn capsule_prefix(label: Option<&str>, max_visible_tokens: usize, raw_tokens: usize) -> String {
    let Some(label) = label else {
        return String::new();
    };
    let full = format!("# {label}\n");
    if max_visible_tokens == 0 {
        return full;
    }
    let budget = max_visible_tokens.saturating_sub(raw_tokens).max(4);
    if count_tokens(&full) <= budget {
        return full;
    }
    let compact = format!("# {}\n", compact_label(label));
    if count_tokens(&compact) <= budget || count_tokens(&compact) < count_tokens(&full) {
        compact
    } else {
        "# source\n".to_string()
    }
}

fn compact_label(label: &str) -> String {
    if label.contains(['\\', '/'])
        && let Some(name) = Path::new(label).file_name().and_then(|name| name.to_str())
    {
        return format!(".../{name}");
    }
    let mut chars = label.chars();
    let head: String = chars.by_ref().take(48).collect();
    chars
        .next()
        .map_or_else(|| label.to_string(), |_| format!("{head}..."))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandStatus {
    pub transport_status: String,
    pub command_success: bool,
    pub exit_code: Option<i32>,
    pub failed_segment: Option<String>,
    pub pipeline_masking_warning: Option<String>,
    pub pipeline_rerun_command: Option<String>,
    pub shell_syntax_summary: String,
    pub status_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub policy: String,
    pub reason: String,
    pub family: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellRender {
    pub visible: String,
    pub policy: PolicyDecision,
    pub command_status: CommandStatus,
    pub diagnostics: Vec<Diagnostic>,
    pub omitted_lines: usize,
    pub output_strategy: String,
}

#[derive(Debug, Clone)]
pub struct ShellRenderInput<'a> {
    pub command: &'a str,
    pub stdout: &'a str,
    pub stderr: &'a str,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub mode: Mode,
    pub max_visible_tokens: usize,
    pub stdout_ref: Option<&'a str>,
    pub stderr_ref: Option<&'a str>,
    pub combined_ref: Option<&'a str>,
}

fn shell_input_status(input: &ShellRenderInput<'_>) -> CommandStatus {
    classify_command_status(
        input.command,
        input.stdout,
        input.stderr,
        input.exit_code,
        input.timed_out,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellViewCase {
    CompactTiny,
    CompactDiagnostic,
    CompactInventory,
    PolicyBased,
}

struct ShellRenderContext<'a> {
    policy: &'a PolicyDecision,
    status: &'a CommandStatus,
    combined: &'a str,
    combined_tokens: usize,
    max_tokens: usize,
}

/// Token count of the full shell input against which a rendered diagnostic is
/// measured. Stream recovery bytes remain unchanged and separately referenced.
pub fn shell_raw_tokens(
    command: &str,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> usize {
    count_tokens(&shell_policy::shell_raw_accounting_output(
        command, exit_code, stdout, stderr,
    ))
}

fn shell_raw_tokens_with_combined(command: &str, combined: &str) -> usize {
    count_tokens(&shell_policy::shell_raw_accounting_output_with_payload(
        command, combined,
    ))
}

pub fn render_shell(input: ShellRenderInput<'_>) -> ShellRender {
    let combined = shell_policy::shell_stream_output(input.exit_code, input.stdout, input.stderr);
    let status = shell_input_status(&input);
    let policy = shell_policy::decide_shell_policy_with_combined(
        input.command,
        input.stdout,
        input.stderr,
        input.exit_code,
        input.mode,
        &combined,
    );
    let combined_line_count = combined.lines().count();
    let combined_tokens = shell_raw_tokens_with_combined(input.command, &combined);
    let (mut minimal_envelope, mut success_compacted) = (false, false);
    let max_t = input.max_visible_tokens;
    let cp = should_compact_tiny_shell(&input, &policy, &status);
    let (case, cd, ci) = if cp {
        (ShellViewCase::CompactTiny, false, false)
    } else if should_compact_short_failure_shell(&input, &policy, &status, &combined) {
        (ShellViewCase::CompactDiagnostic, true, false)
    } else if should_compact_repo_inventory_shell(&input, &policy, &status) {
        (ShellViewCase::CompactInventory, false, true)
    } else {
        (ShellViewCase::PolicyBased, false, false)
    };
    let context = ShellRenderContext {
        policy: &policy,
        status: &status,
        combined: &combined,
        combined_tokens,
        max_tokens: max_t,
    };
    let body = build_shell_body(case, &input, &context, &mut success_compacted);
    let visible = finalize_shell_visible(
        case,
        &input,
        &context,
        body.as_ref(),
        success_compacted,
        &mut minimal_envelope,
    );
    ShellRender {
        omitted_lines: combined_line_count.saturating_sub(visible.lines().count()),
        visible,
        policy,
        command_status: status,
        diagnostics: Vec::new(),
        output_strategy: shell_output_strategy(cp, cd, ci, success_compacted, minimal_envelope)
            .to_string(),
    }
}

fn build_shell_body<'a>(
    case: ShellViewCase,
    input: &ShellRenderInput<'_>,
    context: &'a ShellRenderContext<'a>,
    success_compacted: &mut bool,
) -> Cow<'a, str> {
    let ShellRenderContext {
        policy,
        status,
        combined,
        combined_tokens,
        max_tokens,
    } = context;
    let (combined_tokens, max_tokens) = (*combined_tokens, *max_tokens);
    match case {
        ShellViewCase::CompactTiny => Cow::Owned(compact_shell_view(input.stdout)),
        ShellViewCase::CompactDiagnostic => {
            Cow::Owned(compact_diagnostic_shell_view(input.stdout, input.stderr))
        }
        ShellViewCase::CompactInventory => {
            Cow::Owned(compact_repo_inventory_view(input.command, input.stdout))
        }
        ShellViewCase::PolicyBased => {
            let mut body = if matches!(policy.policy.as_str(), "exact" | "passthrough") {
                Cow::Borrowed(*combined)
            } else {
                Cow::Owned(match policy.policy.as_str() {
                    "diagnostic"
                        if input.exit_code == Some(0)
                            && status.pipeline_masking_warning.is_some() =>
                    {
                        diagnostic_shell_view_with_tail(input.stdout, input.stderr, max_tokens)
                    }
                    "diagnostic" => diagnostic_shell_view(input.stdout, input.stderr, max_tokens),
                    "structured" => {
                        structured_shell_view(input.command, input.stdout, input.stderr)
                    }
                    "dedupe" => dedupe_lines_impl(combined, 6, true),
                    "diff-aware" => diff_summary(combined, 160),
                    _ => summarize_lines(combined, 18, 12, ""),
                })
            };
            if should_compact_success_noise(input, status) && policy.policy != "exact" {
                let mut best_tokens = count_tokens(body.as_ref());
                if let Some(view) = success_noise_view(input.command, input.stdout, input.stderr) {
                    let view_tokens = count_tokens(&view);
                    if view_tokens < best_tokens
                        || (policy.policy == "diagnostic" && view_tokens * 2 <= combined_tokens)
                    {
                        body = Cow::Owned(view);
                        best_tokens = view_tokens;
                        *success_compacted = true;
                    }
                }
                if matches!(
                    policy.policy.as_str(),
                    "dedupe" | "passthrough" | "diagnostic"
                ) && best_tokens > shell_success_summary_budget(max_tokens)
                {
                    let squeezed = summarize_tokens(
                        body.as_ref(),
                        shell_success_summary_budget(max_tokens),
                        "",
                    );
                    if count_tokens(&squeezed) < best_tokens {
                        body = Cow::Owned(squeezed);
                        *success_compacted = true;
                    }
                }
            }
            if policy.policy != "exact" && policy.policy != "passthrough" {
                body = Cow::Owned(mask_visible_secrets(body.as_ref()));
            }
            body
        }
    }
}

fn finalize_shell_visible(
    case: ShellViewCase,
    input: &ShellRenderInput<'_>,
    context: &ShellRenderContext<'_>,
    body: &str,
    success_compacted: bool,
    minimal_envelope: &mut bool,
) -> String {
    let ShellRenderContext {
        policy,
        status,
        combined_tokens,
        max_tokens,
        ..
    } = context;
    let (combined_tokens, max_tokens) = (*combined_tokens, *max_tokens);
    match case {
        ShellViewCase::CompactTiny => body.to_string(),
        ShellViewCase::CompactDiagnostic => enforce_token_budget(
            &compact_diagnostic_shell_capsule(input, status, body),
            max_tokens,
        ),
        ShellViewCase::CompactInventory => enforce_token_budget(
            &compact_repo_inventory_shell_capsule(input, body),
            max_tokens,
        ),
        ShellViewCase::PolicyBased => {
            let mut vis = format_shell_status_header(input, policy, status, body);
            if (count_tokens(&vis) > combined_tokens || success_compacted)
                && safe_auto_success(input, status)
            {
                let minimal = format_minimal_shell_ok(input.combined_ref, body);
                if count_tokens(&minimal) < count_tokens(&vis) {
                    *minimal_envelope = true;
                    vis = minimal;
                }
            }
            enforce_token_budget(&vis, max_tokens)
        }
    }
}
fn shell_output_strategy(cp: bool, cd: bool, ci: bool, sc: bool, me: bool) -> &'static str {
    [
        (cp, "compact_adaptive_shell"),
        (cd, "compact_diagnostic_shell"),
        (ci, "compact_inventory_shell"),
        (sc, "compact_success_shell"),
        (me, "minimal_envelope_shell"),
    ]
    .into_iter()
    .find_map(|(active, strategy)| active.then_some(strategy))
    .unwrap_or("exact_first_adaptive_shell")
}

fn push_shell_kv(out: &mut String, k: &str, v: &str) {
    out.push_str(k);
    out.push_str(": ");
    out.push_str(v);
    out.push('\n');
}

fn push_optional_shell_kv(out: &mut String, k: &str, v: Option<&str>) {
    if let Some(val) = v {
        push_shell_kv(out, k, val);
    }
}

fn push_shell_status(out: &mut String, status: &CommandStatus, compact: bool) {
    push_shell_kv(
        out,
        "exit_code",
        &status
            .exit_code
            .map_or("null".to_string(), |v| v.to_string()),
    );
    for (key, value, always_mask) in [
        ("failed_segment", status.failed_segment.as_deref(), false),
        (
            "pipeline_masking_warning",
            status.pipeline_masking_warning.as_deref(),
            false,
        ),
        (
            "pipeline_rerun_command",
            status.pipeline_rerun_command.as_deref(),
            true,
        ),
    ] {
        let Some(value) = value else { continue };
        let value = if compact && key == "pipeline_masking_warning" && value.contains("mask") {
            "inspect combined_ref".to_string()
        } else if compact || always_mask {
            mask_visible_secrets(value)
        } else {
            value.to_string()
        };
        push_shell_kv(out, key, &value);
    }
}

fn format_shell_status_header(
    input: &ShellRenderInput<'_>,
    policy: &PolicyDecision,
    status: &CommandStatus,
    body: &str,
) -> String {
    let cmd = if matches!(policy.policy.as_str(), "exact" | "passthrough") {
        input.command.to_string()
    } else {
        mask_visible_secrets(input.command)
    };
    let mut vis = "# shell\n".to_string();
    push_shell_kv(&mut vis, "command", &cmd);
    vis.push_str(&format!("policy: {} ({})\n", policy.policy, policy.reason));
    push_shell_kv(&mut vis, "status", &status.status_label);
    vis.push_str(&format!("command_success: {}\n", status.command_success));
    push_shell_status(&mut vis, status, false);
    // The combined payload is the single primary recovery anchor. Stream and
    // capture refs remain machine-visible in ToolResponse::refs, but repeating
    // them in the capsule made one shell action mint up to four visible refs.
    push_optional_shell_kv(&mut vis, "combined_ref", input.combined_ref);
    vis + "\n" + body.trim_end()
}

fn format_minimal_shell_ok(combined_ref: Option<&str>, body: &str) -> String {
    let mut min = "# shell ok".to_string();
    if let Some(r) = combined_ref {
        min += &format!("\ncombined_ref: {r}");
    }
    let trimmed = body.trim_end();
    if !trimmed.is_empty() {
        min = min + "\n" + trimmed;
    }
    min
}

/// Compacts verified byte-identical successful repeats; all other runs render normally.
pub fn render_shell_repeat(input: ShellRenderInput<'_>, repeat_seen: u32) -> ShellRender {
    let status = shell_input_status(&input);
    if repeat_seen >= 2 && safe_auto_success(&input, &status) && input.combined_ref.is_some() {
        let combined =
            shell_policy::shell_stream_output(input.exit_code, input.stdout, input.stderr);
        let raw_tokens =
            shell_raw_tokens(input.command, input.exit_code, input.stdout, input.stderr);
        let mut visible = format!("# shell ok (unchanged; run {repeat_seen})");
        if let Some(r) = input.combined_ref {
            visible += &format!("\ncombined_ref: {r}");
        }
        if count_tokens(&visible) < raw_tokens {
            let visible = enforce_token_budget(&visible, input.max_visible_tokens);
            return ShellRender {
                omitted_lines: combined
                    .lines()
                    .count()
                    .saturating_sub(visible.lines().count()),
                visible,
                policy: PolicyDecision {
                    policy: "passthrough".to_string(),
                    reason: "verified unchanged repeat".to_string(),
                    family: shell_family(input.command, input.stdout, input.stderr),
                },
                command_status: status,
                diagnostics: Vec::new(),
                output_strategy: "repeat_unchanged_shell".to_string(),
            };
        }
    }
    render_shell(input)
}

/// Recognizes compiler/test diagnostic continuation lines.
fn is_critical_continuation_line(line: &str) -> bool {
    if line.starts_with([' ', '\t']) {
        return true;
    }
    let t = line.trim_start();
    let is_num = |n: &str| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit());
    starts_with_any(t, "-->|^|=|note:|help:")
        || t.split_once(' ')
            .is_some_and(|(n, r)| is_num(n) && r.trim_start().starts_with('|'))
}

const CARGO_TEST_OK_SUFFIXES: &str = "... ok|... ignored";

fn is_cargo_test_ok_line(trimmed: &str) -> bool {
    trimmed.starts_with("test ")
        && ends_with_any(trimmed, CARGO_TEST_OK_SUFFIXES)
        && !trimmed.contains("FAILED")
}

fn is_pytest_pass_marker(trimmed: &str) -> bool {
    trimmed.contains("::")
        && ends_with_any(trimmed, "PASSED|XPASS|SKIPPED")
        && !contains_any(trimmed, "FAILED|ERROR")
}

fn is_pytest_summary_line(trimmed: &str) -> bool {
    trimmed.starts_with("==")
        && trimmed.ends_with("==")
        && contains_any(trimmed, " passed| skipped")
        && !contains_any(trimmed, " failed| error")
}

const PYTEST_NOISE_PREFIXES: &str =
    "platform |rootdir:|configfile:|cachedir:|plugins:|collected |collecting ";

fn is_pytest_noise_line(t: &str) -> bool {
    starts_with_any(t, PYTEST_NOISE_PREFIXES)
        || (!t.is_empty() && t.chars().all(|c| matches!(c, '.' | 's' | 'x' | 'X')))
        || (t.starts_with("==") && t.ends_with("==") && t.contains("session starts"))
        || (t.contains("::") && (ends_with_any(t, "PASSED|SKIPPED") || t.contains(" PASSED ")))
        || ends_with_any(t, "XPASS|SKIPPED")
        || t.strip_suffix(']')
            .map(|s| s.trim_end_matches(|c: char| c.is_ascii_digit() || matches!(c, '%' | '[')))
            .is_some_and(|b| {
                !b.is_empty() && b.trim().chars().all(|c| matches!(c, '.' | 's' | 'x' | 'X'))
            })
}

const NPM_SUMMARY_PREFIXES: &str =
    "added |removed |changed |audited |found 0 vulnerabilities|up to date";

fn is_npm_summary_line(t: &str) -> bool {
    starts_with_any(t, NPM_SUMMARY_PREFIXES)
}

const NPM_NOISE_PREFIXES: &str =
    "npm http|npm timing|npm verb|npm sill|npm info|run `npm fund`|run \"npm fund\"";

fn is_npm_noise_line(t: &str) -> bool {
    starts_with_any(t, NPM_NOISE_PREFIXES) || t.contains("packages are looking for funding")
}

const GIT_PROGRESS_PREFIXES: &str = "remote: Enumerating objects|remote: Counting objects|remote: Compressing objects|remote: Total|Receiving objects|Resolving deltas|Counting objects|Compressing objects|Writing objects|Unpacking objects";

fn git_progress_prefix(t: &str) -> Option<&'static str> {
    GIT_PROGRESS_PREFIXES.split('|').find(|p| t.starts_with(p))
}

/// Selects information-dense lines within a soft budget while always retaining criticals.
pub fn summarize_tokens(text: &str, max_tokens: usize, prefix: &str) -> String {
    if max_tokens == 0 {
        return format!("{prefix}{}", text.trim_end());
    }
    let lines: Vec<&str> = text.lines().collect();
    if count_tokens(text) <= max_tokens || lines.len() <= 4 {
        return format!("{prefix}{}", text.trim_end());
    }
    let n = lines.len();
    let line_tokens: Vec<usize> = lines.iter().map(|l| count_tokens(l)).collect();
    let mut order: Vec<usize> = (0..n).collect();
    let scores: Vec<u32> = (0..n)
        .map(|idx| {
            let line = lines[idx];
            if looks_critical_line(line) {
                100
            } else if line.trim().is_empty() {
                0
            } else {
                let mut s = if idx < 3 || idx + 3 >= n { 60 } else { 0 };
                if line_information_density(line) {
                    s += 30;
                }
                s.max(1)
            }
        })
        .collect();
    order.sort_by(|a, b| scores[*b].cmp(&scores[*a]).then(a.cmp(b)));
    let mut selected = vec![false; n];
    let mut spent = 0usize;
    for &idx in &order {
        let cost = line_tokens[idx];
        if scores[idx] >= 100 || (scores[idx] != 0 && spent + cost + 13 <= max_tokens) {
            selected[idx] = true;
            spent = if scores[idx] >= 100 {
                spent.saturating_add(cost)
            } else {
                spent + cost
            };
        }
    }
    for idx in 1..n.saturating_sub(1) {
        if !selected[idx] && selected[idx - 1] && selected[idx + 1] && line_tokens[idx] <= 13 {
            selected[idx] = true;
        }
    }
    if !selected.iter().any(|v| *v) {
        return summarize_lines(text, 8, 6, prefix);
    }
    let mut out = prefix.to_string();
    let mut omitted = 0;
    for idx in 0..n {
        if !selected[idx] {
            omitted += 1;
            continue;
        }
        if omitted > 0 {
            push_summary_line(
                &mut out,
                &format!("... +{omitted} lines; exact ref available ..."),
            );
            omitted = 0;
        }
        push_summary_line(&mut out, lines[idx]);
    }
    if omitted > 0 {
        push_summary_line(
            &mut out,
            &format!("... +{omitted} lines; exact ref available ..."),
        );
    }
    out
}

fn push_summary_line(out: &mut String, line: &str) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(line);
}

/// Detects artifact identifiers, paths, line references, numbers, or hashes.
fn line_information_density(line: &str) -> bool {
    let (digits, paths) = line.chars().fold((0, 0), |(d, p), c| {
        (
            d + usize::from(c.is_ascii_digit()),
            p + usize::from(c == '/' || c == '\\'),
        )
    });
    digits >= 3 || paths >= 2 || line.contains(".rs:") || line.contains(".py:")
}

/// Shell-only dedupe also collapses digit-varying runs while preserving critical lines.
pub fn dedupe_lines_impl(text: &str, context: usize, structural: bool) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let norm = structural.then(|| {
        lines
            .iter()
            .map(|l| normalize_digit_runs(l))
            .collect::<Vec<_>>()
    });
    let mut out = Vec::new();
    let mut idx = 0;
    while idx < lines.len() {
        let line = lines[idx];
        if structural && looks_critical_line(line) {
            out.push(line.to_string());
            idx += 1;
            continue;
        }
        let exact = lines[idx..]
            .iter()
            .take_while(|candidate| **candidate == line)
            .count();
        if exact >= 3 {
            out.push(line.to_string());
            out.push(format!("... repeated {} more times ...", exact - 1));
            idx += exact;
            continue;
        }
        if let Some(n) = norm.as_ref() {
            let similar = (idx..lines.len())
                .take_while(|&i| !looks_critical_line(lines[i]) && n[i] == n[idx])
                .count();
            if similar >= 4 {
                out.push(line.to_string());
                out.push(format!(
                    "... {} similar lines collapsed (digits vary); exact ref available ...",
                    similar - 1
                ));
                idx += similar;
                continue;
            }
        }
        out.extend(lines[idx..idx + exact].iter().map(|l| l.to_string()));
        idx += exact;
    }
    compact_head_tail(out, context)
}

fn compact_head_tail(out: Vec<String>, context: usize) -> String {
    // Saturate: `context = usize::MAX` must keep the whole buffer, not wrap
    // `context * 2` and panic on `out[..context]`.
    if out.len() <= context.saturating_mul(2).saturating_add(20) {
        return out.join("\n");
    }
    format!(
        "{}\n... omitted {} lines; exact ref available ...\n{}",
        out[..context].join("\n"),
        out.len().saturating_sub(context.saturating_mul(2)),
        out[out.len() - context..].join("\n")
    )
}

fn normalize_digit_runs(line: &str) -> String {
    let (mut out, mut in_d) = (String::with_capacity(line.len()), false);
    for c in line.chars() {
        if c.is_ascii_digit() {
            if !in_d {
                out.push('#');
                in_d = true;
            }
        } else {
            in_d = false;
            out.push(c);
        }
    }
    out
}

fn should_compact_short_failure_shell(
    input: &ShellRenderInput<'_>,
    policy: &PolicyDecision,
    status: &CommandStatus,
    combined: &str,
) -> bool {
    // Exit 0 && !timeout is already command_success (tokenzero-3ry6).
    input.mode.effective_policy() == Mode::Auto
        && policy.policy == "diagnostic"
        && !status.command_success
        && input.exit_code.is_some()
        && !input.timed_out
        && input.combined_ref.is_some()
        && (!input.stdout.trim().is_empty() || !input.stderr.trim().is_empty())
        && !has_visible_secret_marker(combined)
        && !has_protected_failure_context(combined)
        && count_tokens(combined) <= 160
        && combined.lines().count() <= 20
        && (looks_diagnostic(combined) || status.failed_segment.is_some())
}

fn compact_diagnostic_shell_view(stdout: &str, stderr: &str) -> String {
    let (mut critical, mut fallback) = (None, None);
    for line in stdout.lines().chain(stderr.lines()) {
        let line = line.trim();
        if line.is_empty() || is_shell_diagnostic_boilerplate(line) {
            continue;
        }
        if looks_failure_anchor_line(line) {
            return line.to_string();
        }
        let slot = if looks_critical_line(line) {
            &mut critical
        } else {
            &mut fallback
        };
        slot.get_or_insert_with(|| line.to_string());
    }
    critical
        .or(fallback)
        .unwrap_or_else(|| "diagnostic output omitted; see combined_ref".to_string())
}

fn compact_diagnostic_shell_capsule(
    input: &ShellRenderInput<'_>,
    status: &CommandStatus,
    body: &str,
) -> String {
    let mut visible = "# shell\n".to_string();
    push_shell_kv(&mut visible, "status", &status.status_label);
    push_shell_status(&mut visible, status, true);
    if input.stderr.is_empty() {
        if let Some(stdout_ref) = input.stdout_ref.filter(|_| !input.stdout.is_empty()) {
            push_shell_kv(&mut visible, "stdout_ref", stdout_ref);
        }
    } else if let Some(stderr_ref) = input.stderr_ref.filter(|_| !input.stderr.is_empty()) {
        push_shell_kv(&mut visible, "stderr_ref", stderr_ref);
    }
    push_optional_shell_kv(&mut visible, "combined_ref", input.combined_ref);
    visible + "\n" + body.trim_end()
}

const SHELL_DIAG_BOILERPLATE_PREFIXES: &str =
    "+ CategoryInfo|+ FullyQualifiedErrorId|At line:|+ ~|~~~~";
const FAILURE_ANCHOR_NEEDLES: &str =
    "error|failure|failed|panic|traceback|exception|assertion|not ok";
const SECRET_MARKERS: &str = "aws_secret_access_key=|aws_access_key_id=|token=|password=|secret=|api_key=|apikey=|x-api-key:|api-key:|authorization:|bearer ";
const SECRET_TOKEN_PREFIXES: &str = "sk-|sk-proj-|ghp_|github_pat_|AKIA|glpat-|xoxb-|xoxp-";

fn is_shell_diagnostic_boilerplate(line: &str) -> bool {
    starts_with_any(line.trim_start(), SHELL_DIAG_BOILERPLATE_PREFIXES)
}

fn looks_failure_anchor_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    FAILURE_ANCHOR_NEEDLES
        .split('|')
        .any(|needle| lower.contains(needle))
}

fn has_visible_secret_marker(text: &str) -> bool {
    contains_any(&text.to_ascii_lowercase(), SECRET_MARKERS)
        || text
            .split_whitespace()
            .any(|word| starts_with_any(word, SECRET_TOKEN_PREFIXES))
}

fn has_protected_failure_context(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    contains_any(&lower, "assertion failed|traceback")
        || (lower.contains("left:") && lower.contains("right:"))
        || (lower.contains("test ") && lower.contains("failed") && lower.contains(".rs:"))
}

const DIFF_LINE_PREFIXES: &str = "diff --git|index |--- |+++ |@@|rename |deleted file|new file|+|-";

pub fn diff_summary(text: &str, max_lines: usize) -> String {
    let out: Vec<_> = text
        .lines()
        .filter(|l| starts_with_any(l, DIFF_LINE_PREFIXES))
        .take(max_lines.max(1))
        .collect();
    if out.is_empty() {
        summarize_lines(text, 18, 12, "")
    } else {
        out.join("\n")
    }
}

pub fn dedupe_lines(text: &str, context: usize) -> String {
    dedupe_lines_impl(text, context, false)
}

pub fn mask_visible_secrets(text: &str) -> String {
    text.lines()
        .map(mask_secret_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn mask_secret_line(line: &str) -> String {
    let low = line.to_ascii_lowercase();
    // Longer keys first so aws_secret_access_key= is not missed because
    // secret= does not match secret_access. Keep trailing space on "bearer "
    // so the marker matches SECRET_MARKERS and the mask lands after the
    // separator (not glued as "bearer[masked]").
    if let Some((key, pos)) = [
        "aws_secret_access_key=",
        "aws_access_key_id=",
        "token=",
        "password=",
        "secret=",
        "api_key=",
        "apikey=",
        "x-api-key:",
        "api-key:",
        "authorization:",
        "bearer ",
    ]
    .into_iter()
    .find_map(|key| low.find(key).map(|pos| (key, pos)))
    {
        return format!("{}[masked]", &line[..pos + key.len()]);
    }
    line.split_whitespace()
        .map(|word| {
            if starts_with_any(word, SECRET_TOKEN_PREFIXES) {
                "[masked]"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn critical_lines(text: &str, radius: usize) -> String {
    keyword_window_view(text, radius, looks_critical_line)
}

pub fn error_block(text: &str, radius: usize) -> String {
    keyword_window_view(text, radius, |line| regex_like_error(&line))
}

/// Keeps radius windows around hits and marks every omitted gap explicitly.
fn omitted_lines_marker(n: usize) -> String {
    format!("... omitted {n} lines; exact ref available ...")
}
fn keyword_window_view(text: &str, radius: usize, is_hit: impl Fn(&str) -> bool) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut keep = vec![false; lines.len()];
    for (idx, line) in lines.iter().enumerate() {
        if is_hit(line) {
            let (start, end) = (
                idx.saturating_sub(radius),
                idx.saturating_add(radius)
                    .saturating_add(1)
                    .min(lines.len()),
            );
            keep[start..end].fill(true);
        }
    }
    if !keep.iter().any(|&k| k) {
        return String::new();
    }
    let (mut out, mut idx) = (Vec::new(), 0);
    while idx < lines.len() {
        if keep[idx] {
            out.push(lines[idx].to_string());
            idx += 1;
        } else {
            let start = idx;
            while idx < lines.len() && !keep[idx] {
                idx += 1;
            }
            out.push(omitted_lines_marker(idx - start));
        }
    }
    out.join("\n")
}

const ERROR_NEEDLES: &str = "error exception traceback failed assertion panic expected actual";

fn regex_like_error(line: &&str) -> bool {
    contains_any_ws(&line.to_ascii_lowercase(), ERROR_NEEDLES)
}

pub fn line_range(text: &str, start: usize, end: usize) -> String {
    let start = start.max(1);
    let end = end.max(start);
    text.lines()
        .skip(start - 1)
        .take(end - start + 1)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn symbol_block(text: &str, symbol: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some(hit) = lines.iter().position(|line| contains_word(line, symbol)) else {
        return String::new();
    };
    let mut start = hit;
    while start > 0 && lines[start - 1].starts_with([' ', '\t']) {
        start -= 1;
    }
    let indent = leading_ws(lines[hit]);
    let mut end = hit + 1;
    while end < lines.len() {
        let line = lines[end];
        if !line.trim().is_empty() && leading_ws(line) <= indent && end > hit + 1 {
            break;
        }
        end += 1;
    }
    lines[start..end].join("\n")
}

fn contains_word(line: &str, symbol: &str) -> bool {
    line.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|word| word == symbol)
}

fn leading_ws(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

/// Generates a legacy short ref: `<prefix>` plus the first eight SHA-256 bytes.
pub fn id_for(prefix: char, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let mut out = format!("{prefix}");
    for byte in &hasher.finalize()[..8] {
        push_hex_byte(&mut out, *byte);
    }
    out
}

const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "jsx", "ts", "tsx", "go", "java", "c", "cc", "cpp", "h", "hpp",
];

fn looks_like_logs(text: &str) -> bool {
    let s: Vec<&str> = text.lines().take(20).collect();
    s.len() >= 5
        && s.iter()
            .filter(|l| {
                contains_any_ws(&l.to_ascii_uppercase(), "DEBUG INFO WARN ERROR FATAL TRACE")
            })
            .count()
            > s.len() / 3
}
pub fn detect_content_type(text: &str, path: Option<&Path>) -> ContentType {
    if let Some(ext) = path.and_then(|p| p.extension()).and_then(|v| v.to_str()) {
        match ext {
            ext if CODE_EXTENSIONS.contains(&ext) => return ContentType::Code,
            "json" => return ContentType::JsonConfig,
            "md" | "markdown" => return ContentType::Markdown,
            "diff" | "patch" => return ContentType::Diff,
            "log" => return ContentType::Logs,
            _ => {}
        }
    }
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        ContentType::JsonConfig
    } else if trimmed.starts_with("diff --git") || trimmed.starts_with("@@") {
        ContentType::Diff
    } else if text.contains("Traceback") || text.contains("FAILED") || text.contains("error:") {
        ContentType::ShellOutput
    } else if looks_like_logs(text) {
        ContentType::Logs
    } else {
        ContentType::Unknown
    }
}

pub fn ref_record(kind: &str, ref_id: String, bytes: usize) -> RefRecord {
    RefRecord {
        kind: kind.to_string(),
        ref_id,
        bytes,
        live: true,
    }
}

pub mod decision_view;
pub mod live_pareto;
pub mod model_artifacts;
pub mod operation_abi;
pub mod output_novelty;
mod protocol_atoms;
pub mod provider_cache;
pub mod reasoning_state;
mod render;
pub use live_pareto::{
    EvidenceFreshness, LiveCandidate, LiveEntry, LiveParetoDecision, MetricOrder, ProtectedOutcome,
    VerifierIdentity, decide_live_pareto,
};
pub mod representation_economics;
mod shell_display;
mod shell_family;
mod shell_parse;
mod shell_policy;
mod shell_quote;
pub mod token_classes;
mod tokens;

use render::domain::*;
use render::noise::*;
use shell_display::*;
use shell_parse::*;
use tokens::*;

pub use protocol_atoms::{
    AckClass, PORTABLE_ONE_TOKEN_ATOMS, ProtocolTokenizer, is_verified_one_token_atom,
    portable_one_token_atoms, render_ack,
};
pub use render::domain::{
    diagnostic_shell_view, diagnostic_shell_view_with_tail, is_repo_inventory_command,
    is_search_shell_command, repo_inventory_view, structured_shell_view,
};
pub use shell_display::{
    shell_display_command_from_argv, shell_display_command_from_argv_for_platform,
};
pub use shell_family::shell_family;
pub use shell_policy::{
    classify_command_status, decide_shell_policy, shell_combined_output,
    shell_raw_accounting_output,
};
pub use shell_quote::{
    argv_has_shell_operator_tokens, contains_platform_shell_syntax, contains_shell_syntax,
    host_shell_platform, is_shell_operator_token, is_windows_shell_builtin, is_windows_shell_host,
    looks_like_powershell_syntax, quote_for, quote_posix, quote_powershell, quote_windows_cmd,
    split_command_string, split_command_string_for_platform,
};
pub use tokens::{
    BYTES_ESTIMATOR_ID, FORBIDDEN_MCP_ENGINE_IDENTITY, FORBIDDEN_MCP_REGISTRY_ENGINE,
    LEXICAL_ESTIMATOR_ID, TokenizerFamily, TokenizerIdPreflightError, TokenizerMetadata,
    UNLABELED_ESTIMATE_TOKENIZER_PREFIX, VISIBLE_BUDGET_LOSSY_DECLARATION, active_model_id,
    active_tokenizer_metadata, count_tokens, count_tokens_for_model, count_tokens_tokenizer_id,
    enforce_token_budget, enforce_token_budget_with_ref, is_forbidden_mcp_tokenizer_identity,
    pack_to_token_boundary, pack_to_token_boundary_for_model,
    pack_to_token_boundary_for_model_with_char_limit, pack_to_token_boundary_with_char_limit,
    prefix_end_for_kept_lines, preflight_tokenizer_id, savings_ratio, savings_ratio_u64,
    sha256_hex, tokenizer_metadata,
};

#[cfg(test)]
mod never_worse_capsule_tests {
    use super::{Mode, count_tokens, make_capsule, make_capsule_with_recovery_ref, savings_ratio};

    #[test]
    fn exact_stub_that_costs_more_than_raw_passthroughs() {
        let text = "hi";
        let capsule = make_capsule(text, Mode::Exact, 64, None).expect("capsule");
        assert_eq!(capsule.text, text);
        assert_eq!(capsule.visible_tokens, count_tokens(text));
        assert_eq!(capsule.mode, Mode::Passthrough);
        assert_eq!(
            savings_ratio(capsule.raw_tokens, capsule.visible_tokens),
            0.0
        );
    }

    #[test]
    fn recovery_handle_wrapper_does_not_inflate_tiny_payload() {
        let text = "hi";
        let capsule = make_capsule_with_recovery_ref(
            text,
            count_tokens(text),
            Mode::Exact,
            64,
            None,
            Some("z://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#B0-2"),
        )
        .expect("capsule");
        assert_eq!(capsule.text.trim_end(), text);
        assert!(
            capsule.visible_tokens <= count_tokens(text),
            "visible={} raw={}",
            capsule.visible_tokens,
            count_tokens(text)
        );
    }

    #[test]
    fn canonical_line_and_plus_byte_fragments_are_recovery_selectors() {
        let text = (0..80)
            .map(|i| format!("tok{i:02}"))
            .collect::<Vec<_>>()
            .join(" ");
        let hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        for selector in ["#L1-3", "#L1-L3", "#B0+5"] {
            let handle = format!("tz://blob/{hash}{selector}");
            let capsule = make_capsule_with_recovery_ref(
                &text,
                count_tokens(&text),
                Mode::Structured,
                8,
                None,
                Some(&handle),
            )
            .expect("capsule");
            assert!(
                capsule.exact_refs.iter().any(|r| r.contains(selector))
                    || capsule.text.contains(selector),
                "expand grammar {selector} must be a recovery selector, got text={:?} refs={:?}",
                capsule.text,
                capsule.exact_refs
            );
        }
    }
}
