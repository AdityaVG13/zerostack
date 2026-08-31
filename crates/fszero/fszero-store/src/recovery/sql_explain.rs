//! Env-gated EXPLAIN / query-plan capture for hot RecoveryStore SQL. Gate: `FSZERO_SQL_EXPLAIN=1`.
//! Writes plan text under `tests/artifacts/perf/<run-id>/db/explain_<name>.txt` when a path is
//! provided, and always returns structured captures for harnesses.

use fsqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// One hot-catalog entry: stable name + SQL text (parameters unbound for plan).
#[derive(Debug, Clone, Copy)]
pub struct HotSqlEntry {
    pub name: &'static str,
    pub sql: &'static str,
    pub note: &'static str,
}

/// Plan-sensitive RecoveryStore SQL used by the profiling harness.
pub fn hot_sql_catalog() -> &'static [HotSqlEntry] {
    &[
        HotSqlEntry {
            name: "select_live_refs",
            sql: super::SQL_SELECT_LIVE_REFS,
            note: "9-way UNION CAS GC root mark",
        },
        HotSqlEntry {
            name: "delete_transient_overflow",
            sql: super::SQL_DELETE_TRANSIENT_OVERFLOW,
            note: "LIKE + LEFT JOIN + ORDER retention sweep",
        },
        HotSqlEntry {
            name: "delete_memory_paths_by_store_key",
            sql: super::SQL_DELETE_MEMORY_PATHS_BY_STORE_KEY,
            note: "no index on memory_paths.store_key (product follow-up)",
        },
        HotSqlEntry {
            name: "pack_validation_pending_scan",
            sql: "SELECT key, offset, len FROM pack_validation_pending WHERE generation = ?1 AND key > ?2 ORDER BY key LIMIT 256",
            note: "no index on generation (PK is key only)",
        },
        HotSqlEntry {
            name: "edit_intents_by_root_path",
            sql: "SELECT COUNT(*) FROM edit_intents WHERE root=?1 AND path=?2",
            note: "no composite index on (root, path)",
        },
        HotSqlEntry {
            name: "count_transient_payloads",
            sql: super::SQL_COUNT_TRANSIENT_PAYLOADS,
            note: "LIKE seq/% prefix on PK unproven",
        },
        HotSqlEntry {
            name: "select_memory_paths_prefix",
            sql: super::SQL_SELECT_MEMORY_PATHS_PREFIX,
            note: "has idx_memory_paths_prefix -- verify plan",
        },
    ]
}

#[derive(Debug, Clone)]
pub struct SqlExplainCapture {
    pub name: String,
    pub sql: String,
    pub note: String,
    pub prepare_explain: Result<String, String>,
    pub explain_query_plan: Result<String, String>,
    pub plan_class: String,
}

fn classify_plan(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut tags = Vec::new();
    if lower.contains("scan") || lower.contains("table") {
        tags.push("scan");
    }
    if lower.contains("index") || lower.contains("search") {
        tags.push("index");
    }
    if lower.contains("union") {
        tags.push("union");
    }
    if lower.contains("temp") || lower.contains("sort") {
        tags.push("temp/sort");
    }
    if tags.is_empty() {
        "unknown".into()
    } else {
        tags.join("+")
    }
}

fn explain_prepare(conn: &Connection, sql: &str) -> Result<String, String> {
    let stmt = conn
        .prepare(sql)
        .map_err(|e| format!("prepare failed: {e}"))?;
    Ok(stmt.explain())
}

fn explain_query_plan(conn: &Connection, sql: &str) -> Result<String, String> {
    let wrapped = format!("EXPLAIN QUERY PLAN {sql}");
    match conn.query(&wrapped) {
        Ok(rows) => {
            let mut lines = Vec::new();
            for row in rows {
                let cols: Vec<String> = row.values().iter().map(|v| format!("{v:?}")).collect();
                lines.push(cols.join(" | "));
            }
            if lines.is_empty() {
                Ok("(no rows)".into())
            } else {
                Ok(lines.join("\n"))
            }
        }
        Err(e) => Err(format!("EXPLAIN QUERY PLAN failed: {e}")),
    }
}

/// Capture VDBE + query-plan text for every hot catalog entry.
pub fn capture_hot_sql_explains(conn: &Connection) -> Vec<SqlExplainCapture> {
    hot_sql_catalog()
        .iter()
        .map(|entry| {
            let prepare_explain = explain_prepare(conn, entry.sql);
            let explain_query_plan = explain_query_plan(conn, entry.sql);
            let combined = format!(
                "{}\n{}",
                prepare_explain.as_deref().unwrap_or(""),
                explain_query_plan.as_deref().unwrap_or("")
            );
            SqlExplainCapture {
                name: entry.name.into(),
                sql: entry.sql.into(),
                note: entry.note.into(),
                prepare_explain,
                explain_query_plan,
                plan_class: classify_plan(&combined),
            }
        })
        .collect()
}

/// Write `explain_<name>.txt` (+ summary JSON) under `out_dir/db/`.
pub fn write_sql_explain_artifacts(
    out_dir: &Path,
    captures: &[SqlExplainCapture],
) -> Result<PathBuf, String> {
    let db_dir = out_dir.join("db");
    fs::create_dir_all(&db_dir).map_err(|e| format!("mkdir {}: {e}", db_dir.display()))?;
    let mut summary = Vec::new();
    for c in captures {
        let path = db_dir.join(format!("explain_{}.txt", c.name));
        let mut body = String::new();
        body.push_str(&format!("# name: {}\n", c.name));
        body.push_str(&format!("# note: {}\n", c.note));
        body.push_str(&format!("# plan_class: {}\n", c.plan_class));
        body.push_str(&format!("# sql:\n{}\n\n", c.sql));
        body.push_str("## PreparedStatement::explain()\n");
        match &c.prepare_explain {
            Ok(t) => body.push_str(t),
            Err(e) => body.push_str(&format!("ERROR: {e}")),
        }
        body.push_str("\n\n## EXPLAIN QUERY PLAN\n");
        match &c.explain_query_plan {
            Ok(t) => body.push_str(t),
            Err(e) => body.push_str(&format!("ERROR: {e}")),
        }
        body.push('\n');
        fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
        summary.push(json!({
            "name": c.name,
            "plan_class": c.plan_class,
            "note": c.note,
            "prepare_ok": c.prepare_explain.is_ok(),
            "eqp_ok": c.explain_query_plan.is_ok(),
            "artifact": path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
        }));
    }
    let summary_path = db_dir.join("explain_summary.json");
    fs::write(
        &summary_path,
        serde_json::to_string_pretty(&json!({
            "schema": "fszero-sql-explain",
            "entries": summary,
        }))
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("write summary: {e}"))?;
    Ok(db_dir)
}

/// Default run ID for gated SQL diagnostic dumps.
pub fn default_explain_artifact_dir() -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    PathBuf::from(format!("tests/artifacts/perf/sql-explain-{millis}"))
}

pub fn sql_explain_env_enabled() -> bool {
    match std::env::var("FSZERO_SQL_EXPLAIN") {
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

/// If env-gated, capture plans and write artifacts (returns dir or None if off).
pub fn maybe_capture_sql_explains(
    conn: &Connection,
    out_dir: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    if !sql_explain_env_enabled() {
        return Ok(None);
    }
    let captures = capture_hot_sql_explains(conn);
    let dir = out_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(default_explain_artifact_dir);
    let db = write_sql_explain_artifacts(&dir, &captures)?;
    Ok(Some(db))
}

/// JSON summary suitable for root_report / doctor (enabled flag always).
pub fn sql_explain_status_json() -> serde_json::Value {
    json!({
        "enabled": sql_explain_env_enabled(),
        "env": "FSZERO_SQL_EXPLAIN",
        "catalog_len": hot_sql_catalog().len(),
        "api": "RecoveryStore::capture_sql_explains / maybe_capture_sql_explains",
    })
}
