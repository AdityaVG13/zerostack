//! Env-gated fsqlite statement PROFILE aggregation (fszero-sql-stmt-profile-trace-x38n).
//!
//! Off by default. Set `FSZERO_SQL_PROFILE=1` before opening a RecoveryStore so
//! `Connection::trace(TraceMask::PROFILE, …)` records per-SQL elapsed_ns into
//! a process-global top-N table.

use fsqlite::{Connection, TraceEvent, TraceMask};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

const MAX_SQL_KEY_CHARS: usize = 160;
const DEFAULT_TOP_N: usize = 20;

#[derive(Debug, Clone, Copy, Default)]
struct Agg {
    calls: u64,
    total_ns: u64,
}

fn env_enabled() -> bool {
    match std::env::var("FSZERO_SQL_PROFILE") {
        Ok(v) => {
            let t = v.trim();
            t == "1"
                || t.eq_ignore_ascii_case("true")
                || t.eq_ignore_ascii_case("yes")
                || t.eq_ignore_ascii_case("on")
        }
        Err(_) => false,
    }
}

fn registry() -> &'static Mutex<HashMap<String, Agg>> {
    static REG: OnceLock<Mutex<HashMap<String, Agg>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

fn normalize_sql(sql: &str) -> String {
    let flat: String = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_SQL_KEY_CHARS {
        flat
    } else {
        flat.chars().take(MAX_SQL_KEY_CHARS).collect::<String>() + "…"
    }
}

fn record_profile(sql: &str, elapsed_ns: u64) {
    let key = normalize_sql(sql);
    if let Ok(mut map) = registry().lock() {
        let e = map.entry(key).or_default();
        e.calls = e.calls.saturating_add(1);
        e.total_ns = e.total_ns.saturating_add(elapsed_ns);
    }
}

/// Install PROFILE trace on a RecoveryStore connection when env-gated.
pub(super) fn maybe_install_sql_profile(conn: &Connection) {
    if !env_enabled() {
        return;
    }
    let cb = Arc::new(|event: TraceEvent| {
        if let TraceEvent::Profile { sql, elapsed_ns } = event {
            record_profile(&sql, elapsed_ns);
        }
    });
    conn.trace_v2(TraceMask::PROFILE, Some(cb));
}

/// Whether SQL PROFILE sampling is enabled for this process env.
pub fn sql_profile_env_enabled() -> bool {
    env_enabled()
}

/// Clear accumulated statement timings (tests / explicit reset).
pub fn reset_sql_profile() {
    if let Ok(mut map) = registry().lock() {
        map.clear();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlProfileRow {
    pub sql: String,
    pub calls: u64,
    pub total_ns: u64,
    pub mean_ns: u64,
}

/// Top-N statements by total elapsed nanoseconds.
pub fn sql_profile_top(n: usize) -> Vec<SqlProfileRow> {
    let Ok(map) = registry().lock() else {
        return Vec::new();
    };
    let mut rows: Vec<SqlProfileRow> = map
        .iter()
        .map(|(sql, a)| SqlProfileRow {
            sql: sql.clone(),
            calls: a.calls,
            total_ns: a.total_ns,
            mean_ns: if a.calls == 0 {
                0
            } else {
                a.total_ns / a.calls
            },
        })
        .collect();
    rows.sort_by(|a, b| {
        b.total_ns
            .cmp(&a.total_ns)
            .then_with(|| b.calls.cmp(&a.calls))
    });
    rows.truncate(n.max(1));
    rows
}

/// Structured JSON for telemetry / perf artifacts.
pub fn sql_profile_json() -> serde_json::Value {
    if !env_enabled() {
        return json!({"enabled": false});
    }
    let top = sql_profile_top(DEFAULT_TOP_N);
    let statements: Vec<serde_json::Value> = top
        .into_iter()
        .map(|r| {
            json!({
                "sql": r.sql,
                "calls": r.calls,
                "total_ns": r.total_ns,
                "mean_ns": r.mean_ns,
            })
        })
        .collect();
    json!({
        "enabled": true,
        "top_n": DEFAULT_TOP_N,
        "sort": "total_ns_desc",
        "statements": statements,
        "source": "fsqlite::Connection::trace(TraceMask::PROFILE)",
        "env": "FSZERO_SQL_PROFILE",
    })
}
