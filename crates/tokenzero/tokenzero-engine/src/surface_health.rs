//! Surface exclusivity + expand-health telemetry for CodeMode.
//!
//! Field bug (tokenzero-surface-exclusivity-1r9): agents call `tz_expand`
//! mid-CodeMode because crash-only recovery shims stay in `tools/list`, then
//! burn tokens on a policy lecture. The router owns fallback; agents see one
//! surface.
//!
//! Policy (CodeMode surface):
//! | Tool class              | tools/list | tools/call                          |
//! |-------------------------|------------|-------------------------------------|
//! | codemode execute/search | listed     | allow                               |
//! | expand / read recovery  | **hidden** | **unknown_tool** (fallback internal)|
//! | shell / edit / write    | **hidden** | **unknown_tool**                    |
//!
//! Classic surface is not gated: per-op tools are the primary surface.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokenzero_core::McpToolSurface;

/// Default: one expand-surface failure opens recovery for the window.
const DEFAULT_FAIL_THRESHOLD: u32 = 1;
/// Default unlock window after the last failure (5 minutes).
const DEFAULT_WINDOW: Duration = Duration::from_secs(300);

/// Documented recovery ladder (docs + skill + close reasons).
pub const RECOVERY_LADDER: &str = "\
CodeMode recovery ladder (router-owned, agent-invisible):\n\
1. Prefer zero.token.expand / zero.token.read inside tz_execute_code (primary).\n\
2. On expand miss / X0 the engine retries sibling stores internally before failing.\n\
3. Per-op MCP tools (tz_expand, tz_read, tz_shell, …) stay hidden from tools/list.\n\
4. CLI `tokenzero expand` / `tokenzero read` remain available outside MCP.\n\
Telemetry: resource://tokenzero/metrics → surface_health.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashOnlyDecision {
    /// Primary surface healthy — recovery shim blocked.
    Blocked,
    /// Surface unhealthy — recovery path unlocked (count + audit).
    Unlocked,
    /// Mutation/shell: never unlocked by expand health on CodeMode.
    PermanentlyLocked,
    /// Not subject to crash-only gate (classic surface or codemode primary tools).
    NotGated,
}

/// Single source of truth for CodeMode membership + crash-only gating.
/// Catalog `tools/list`, JSON-RPC `tools/call`, and FastMCP registration all
/// consult this classification (via [`tool_listed_on_surface`] /
/// [`SurfaceHealth::gate_tools_call`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolClass {
    /// Always listed/callable on CodeMode (`execute_code`, report, …).
    Primary,
    /// Crash-only recovery shims (`expand` / `read`).
    Recovery,
    /// Never unlocked by expand health (shell/edit/write/…).
    Locked,
}

/// Whether a `tools/call` name is even a candidate on this surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallAdmission {
    /// Surface does not own this tool (e.g. Classic calling `tz_execute_code`).
    UnknownTool,
    /// Proceed to crash-only [`SurfaceHealth::allow_tool_call`].
    Proceed,
}

/// How strictly to gate a tools/call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateMode {
    /// JSON-RPC: membership + crash-only health.
    Strict,
    /// FastMCP: health only. Registration already filters by surface; the call
    /// helper stays membership-open so one process can host both surfaces and
    /// unit tests can exercise CodeMode plans on a Classic-configured engine.
    HealthOnly,
}

/// Refusal from [`SurfaceHealth::gate_tools_call`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateRefusal {
    UnknownTool,
    Policy(String),
}

pub(crate) fn tool_class(tool_name: &str) -> ToolClass {
    let canonical = strip_tool_alias(tool_name);
    match canonical {
        "execute_code" | "codemode_search" | "codemode_describe" | "codemode"
        | "report_tool_issue" => ToolClass::Primary,
        "expand" | "read" => ToolClass::Recovery,
        // Everything else on CodeMode is permanently locked (shell/edit/write/…).
        _ => ToolClass::Locked,
    }
}

fn strip_tool_alias(name: &str) -> &str {
    let bare = name.strip_prefix("tz_").unwrap_or(name);
    match bare {
        "report-tool-issue" => "report_tool_issue",
        other => other,
    }
}

/// CodeMode-exclusive primaries (not listed/callable on Classic).
/// `report_tool_issue` is intentionally available on both surfaces.
fn is_codemode_exclusive(tool_name: &str) -> bool {
    matches!(
        strip_tool_alias(tool_name),
        "execute_code" | "codemode_search" | "codemode_describe" | "codemode"
    )
}

/// Whether `tools/list` (and FastMCP registration) should advertise `tool_name`.
///
/// CodeMode is primary-only for the whole session (`tools.listChanged=false`).
/// Recovery shims stay hidden; expand fallback is engine-internal.
pub fn tool_listed_on_surface(
    surface: McpToolSurface,
    tool_name: &str,
    _recovery_unlocked: bool,
) -> bool {
    match surface {
        McpToolSurface::Classic => !is_codemode_exclusive(tool_name),
        McpToolSurface::CodeMode => tool_class(tool_name) == ToolClass::Primary,
    }
}

/// Static membership used by one-time FastMCP registration.
pub fn surface_includes(surface: McpToolSurface, tool_name: &str) -> bool {
    tool_listed_on_surface(surface, tool_name, false)
}

/// Admit a `tools/call` before the crash-only health gate.
pub fn admit_tools_call(surface: McpToolSurface, tool_name: &str) -> CallAdmission {
    match surface {
        McpToolSurface::Classic if is_codemode_exclusive(tool_name) => CallAdmission::UnknownTool,
        // CodeMode: only Primary tools are callable. Per-op / recovery names
        // return unknown_tool (no policy lecture) so agents stay on one surface.
        McpToolSurface::CodeMode if tool_class(tool_name) != ToolClass::Primary => {
            CallAdmission::UnknownTool
        }
        _ => CallAdmission::Proceed,
    }
}

#[derive(Debug, Clone)]
struct HealthInner {
    consecutive_failures: u32,
    last_failure_at: Option<Instant>,
    last_failure_kind: Option<&'static str>,
    blocked_count: u64,
    unlocked_count: u64,
    fail_threshold: u32,
    window: Duration,
}

impl Default for HealthInner {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            last_failure_at: None,
            last_failure_kind: None,
            blocked_count: 0,
            unlocked_count: 0,
            fail_threshold: DEFAULT_FAIL_THRESHOLD,
            window: DEFAULT_WINDOW,
        }
    }
}

impl HealthInner {
    fn is_unhealthy(&self, now: Instant) -> bool {
        self.consecutive_failures >= self.fail_threshold
            && self
                .last_failure_at
                .is_some_and(|at| now.duration_since(at) < self.window)
    }

    fn record_failure(&mut self, kind: &'static str, now: Instant) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.last_failure_at = Some(now);
        self.last_failure_kind = Some(kind);
    }

    fn record_success(&mut self) {
        self.consecutive_failures = 0;
    }
}

/// Session-scoped expand/read surface health + crash-only gate.
#[derive(Debug, Default)]
pub struct SurfaceHealth {
    inner: Mutex<HealthInner>,
}

impl SurfaceHealth {
    pub fn new() -> Self {
        Self::default()
    }

    /// Test / config hook for threshold and unlock window.
    #[allow(dead_code)]
    pub fn with_policy(fail_threshold: u32, window: Duration) -> Self {
        Self {
            inner: Mutex::new(HealthInner {
                fail_threshold: fail_threshold.max(1),
                window,
                ..HealthInner::default()
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HealthInner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn is_healthy(&self) -> bool {
        !self.lock().is_unhealthy(Instant::now())
    }

    /// True when recovery expand/read may be engaged on CodeMode.
    pub fn recovery_unlocked(&self) -> bool {
        !self.is_healthy()
    }

    /// Record an expand-path failure that indicates surface unhealth.
    /// Client/precondition errors do not unlock the recovery surface.
    pub fn record_expand_outcome(&self, ok: bool, code: Option<&str>) {
        let now = Instant::now();
        let mut inner = self.lock();
        if ok {
            inner.record_success();
            return;
        }
        let kind = match code.unwrap_or("expand_failed") {
            "expand_failed" => "expand_failed",
            "ref_not_found" => "ref_not_found",
            "ref_stale" => "ref_stale",
            "store_mismatch" => "store_mismatch",
            "substrate_down" => "substrate_down",
            "expand_x0" => "expand_x0",
            _ => return,
        };
        inner.record_failure(kind, now);
    }

    /// Codemode plan ended in X0 while expand/read was in the plan.
    pub fn record_codemode_expand_x0(&self) {
        self.record_expand_outcome(false, Some("expand_x0"));
    }

    pub fn record_substrate_down(&self) {
        self.record_expand_outcome(false, Some("substrate_down"));
    }

    pub fn record_read_outcome(&self, ok: bool, code: Option<&str>) {
        if ok {
            self.record_expand_outcome(true, None);
        } else if matches!(code, Some("read_substrate_down" | "substrate_down")) {
            self.record_substrate_down();
        }
    }

    /// Never claim "primary surface healthy" when unhealthy.
    pub fn primary_surface_healthy_claim(&self) -> bool {
        self.is_healthy()
    }

    pub fn decide(&self, surface: McpToolSurface, tool_name: &str) -> CrashOnlyDecision {
        decide_static(surface, tool_name, self.recovery_unlocked())
    }

    /// Gate a tools/call: Ok(decision) when allowed, Err(policy message) when refused.
    /// Updates blocked/unlocked telemetry on recovery decisions.
    pub fn allow_tool_call(
        &self,
        surface: McpToolSurface,
        tool_name: &str,
    ) -> Result<CrashOnlyDecision, String> {
        let decision = self.decide(surface, tool_name);
        match decision {
            CrashOnlyDecision::NotGated => Ok(decision),
            CrashOnlyDecision::Unlocked => {
                let mut inner = self.lock();
                inner.unlocked_count = inner.unlocked_count.saturating_add(1);
                Ok(CrashOnlyDecision::Unlocked)
            }
            CrashOnlyDecision::Blocked => {
                let mut inner = self.lock();
                inner.blocked_count = inner.blocked_count.saturating_add(1);
                Err(blocked_message(tool_name))
            }
            CrashOnlyDecision::PermanentlyLocked => Err(format!(
                "Policy: {tool_name} is not available on the CodeMode surface \
                 (write/shell safety is never unlocked by expand health). \
                 Use zero.token.* inside tz_execute_code."
            )),
        }
    }

    /// Single entry for tools/call: optional membership admit, then crash-only health.
    pub fn gate_tools_call(
        &self,
        surface: McpToolSurface,
        tool_name: &str,
        mode: GateMode,
    ) -> Result<CrashOnlyDecision, GateRefusal> {
        if mode == GateMode::Strict
            && admit_tools_call(surface, tool_name) == CallAdmission::UnknownTool
        {
            return Err(GateRefusal::UnknownTool);
        }
        self.allow_tool_call(surface, tool_name)
            .map_err(GateRefusal::Policy)
    }

    /// Whether tools/list should advertise `tool_name` given current health.
    pub fn list_includes(&self, surface: McpToolSurface, tool_name: &str) -> bool {
        tool_listed_on_surface(surface, tool_name, self.recovery_unlocked())
    }

    pub fn telemetry(&self) -> Value {
        let inner = self.lock();
        let healthy = !inner.is_unhealthy(Instant::now());
        json!({
            "schema_version": "tokenzero.surface_health.v1",
            "primary_surface_healthy": healthy,
            "recovery_unlocked": !healthy,
            "consecutive_failures": inner.consecutive_failures,
            "last_failure_kind": inner.last_failure_kind,
            "fail_threshold": inner.fail_threshold,
            "window_secs": inner.window.as_secs(),
            "telemetry": {
                "blocked_count": inner.blocked_count,
                "unlocked_count": inner.unlocked_count,
            },
            "recovery_ladder": RECOVERY_LADDER,
            "unlocks": ["expand", "read"],
            "never_unlocks": ["shell", "edit", "write"],
            "agent_visible_recovery_tools": false,
            "worker_process_observation": crate::shell_hooks::process_observation_snapshot(),
        })
    }
}

fn blocked_message(tool_name: &str) -> String {
    let short = strip_tool_alias(tool_name);
    format!(
        "Policy: tz_{short} is not on the CodeMode agent surface. \
         Use zero.token.{short} via tz_execute_code; expand fallback is engine-internal. \
         Ladder: see resource://tokenzero/metrics surface_health."
    )
}

/// Pure policy helper (tests + call-path membership checks without engine state).
pub fn decide_static(
    surface: McpToolSurface,
    tool_name: &str,
    recovery_unlocked: bool,
) -> CrashOnlyDecision {
    if surface != McpToolSurface::CodeMode {
        return CrashOnlyDecision::NotGated;
    }
    match tool_class(tool_name) {
        ToolClass::Primary => CrashOnlyDecision::NotGated,
        ToolClass::Locked => CrashOnlyDecision::PermanentlyLocked,
        ToolClass::Recovery if recovery_unlocked => CrashOnlyDecision::Unlocked,
        ToolClass::Recovery => CrashOnlyDecision::Blocked,
    }
}
