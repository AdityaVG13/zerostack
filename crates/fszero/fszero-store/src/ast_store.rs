//! SQLite persistence for AST nodes and call edges.

use rusqlite::{Connection, params};
use std::collections::HashSet;
use std::path::Path;

const SQL_INSERT_AST_NODE: &str = "INSERT INTO ast_nodes (file_key, kind, span_start, span_end, symbol, parent, version) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)";
const SQL_INSERT_CALL_EDGE: &str =
    "INSERT INTO call_edges (file_key, caller, callee, line, version) VALUES (?1, ?2, ?3, ?4, ?5)";
const SQL_DELETE_AST_NODES_FILE: &str = "DELETE FROM ast_nodes WHERE file_key = ?1";
const SQL_DELETE_CALL_EDGES_FILE: &str = "DELETE FROM call_edges WHERE file_key = ?1";
const SQL_SELECT_SPANS_FOR_FILE: &str =
    "SELECT kind, symbol, span_start, span_end FROM ast_nodes WHERE file_key = ?1";
const SQL_DELETE_AST_SPAN: &str = "DELETE FROM ast_nodes WHERE file_key = ?1 AND kind = ?2 AND symbol = ?3 AND span_start = ?4 AND span_end = ?5";
const SQL_SELECT_AST_NODE_EXISTS: &str = "SELECT 1 FROM ast_nodes LIMIT 1";
const SQL_COUNT_AST_NODES: &str = "SELECT COUNT(*) FROM ast_nodes";
/// Shared symbol-kind filter (query_all_symbols + query_symbols_like).
const SQL_QUERY_ALL_SYMBOLS: &str = concat!(
    "SELECT symbol, file_key FROM ast_nodes WHERE ",
    "kind IN ('fn', 'method', 'type', 'enum', 'interface', 'class')",
    " AND version = ?1 ORDER BY file_key, symbol"
);
const SQL_QUERY_SYMBOLS_LIKE: &str = concat!(
    "SELECT file_key, symbol, kind, span_start, span_end FROM ast_nodes WHERE ",
    "kind IN ('fn', 'method', 'type', 'enum', 'interface', 'class')",
    " AND version = ?2 AND symbol LIKE ?1 ESCAPE '\\' ORDER BY file_key, symbol",
);
const SQL_QUERY_IMPORTS: &str = "SELECT file_key, symbol, span_start, span_end FROM ast_nodes WHERE kind='import' AND version = ?1 ORDER BY file_key, span_start";
const SQL_QUERY_CALLERS: &str = "SELECT DISTINCT file_key, caller FROM call_edges WHERE callee = ?1 AND version = ?2 AND EXISTS (SELECT 1 FROM ast_nodes WHERE kind IN ('fn', 'method') AND symbol=call_edges.callee AND version = ?2) ORDER BY file_key, caller";
const SQL_FN_SPAN: &str = "SELECT file_key, span_start, span_end FROM ast_nodes WHERE kind='fn' AND symbol = ?1 AND version = ?2 ORDER BY file_key LIMIT 1";
const SQL_FN_SPAN_ANY: &str = "SELECT file_key, span_start, span_end FROM ast_nodes WHERE kind='fn' AND symbol = ?1 ORDER BY version DESC, file_key LIMIT 1";

pub struct AstStore {
    conn: Connection,
    in_bulk: bool,
}

/// Identity of one persisted symbol/import span row (cocoindex-style AST diff).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AstSpanKey {
    pub kind: String,
    pub symbol: String,
    pub span_start: i64,
    pub span_end: i64,
}

/// How many persisted AST span rows a file-level upsert kept vs rewrote.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AstSpanDiffStats {
    pub kept: u64,
    pub deleted: u64,
    pub inserted: u64,
}

fn init_schema(conn: &Connection) {
    // Sidecar is rebuildable; MEMORY/OFF is safe (cold rebuild only on loss).
    let _ = conn.execute_batch(
        "PRAGMA journal_mode = MEMORY; PRAGMA synchronous = OFF; PRAGMA temp_store = MEMORY;",
    );
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0);
    if version != 2 {
        let _ = conn.execute_batch(
            "DROP TABLE IF EXISTS ast_nodes; DROP TABLE IF EXISTS call_edges;\
             DROP INDEX IF EXISTS idx_ast_nodes_symbol; DROP INDEX IF EXISTS idx_ast_nodes_file;\
             DROP INDEX IF EXISTS idx_call_edges_callee; DROP INDEX IF EXISTS idx_call_edges_file;",
        );
    }
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ast_nodes (id INTEGER PRIMARY KEY, file_key TEXT, kind TEXT, span_start INTEGER, span_end INTEGER, symbol TEXT, parent INTEGER, version INTEGER DEFAULT 0);\
         CREATE INDEX IF NOT EXISTS idx_ast_nodes_file ON ast_nodes(file_key, version);\
         CREATE TABLE IF NOT EXISTS call_edges (file_key TEXT, caller TEXT, callee TEXT, line INTEGER, version INTEGER DEFAULT 0);\
         CREATE INDEX IF NOT EXISTS idx_call_edges_file ON call_edges(file_key, version);\
         PRAGMA user_version = 2;",
    );
}

impl AstStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path)
            .map_err(|e| format!("ast sidecar open failed for {}: {e}", path.display()))?;
        init_schema(&conn);
        Ok(Self {
            conn,
            in_bulk: false,
        })
    }

    pub fn memory() -> Self {
        let conn = Connection::open_in_memory().expect("rusqlite :memory:");
        init_schema(&conn);
        Self {
            conn,
            in_bulk: false,
        }
    }

    /// One transaction around the whole bulk build; fail-open (autocommit
    /// per statement) if BEGIN fails.
    pub fn begin_bulk(&mut self) {
        if !self.in_bulk && self.conn.execute_batch("BEGIN IMMEDIATE").is_ok() {
            self.in_bulk = true;
        }
    }

    pub fn end_bulk(&mut self) {
        if self.in_bulk {
            let _ = self.conn.execute_batch("COMMIT");
            self.in_bulk = false;
        }
        // Recreate only the eagerly maintained indexes; the rest are created on demand.
        let _ = self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_ast_nodes_file ON ast_nodes(file_key, version);
             CREATE INDEX IF NOT EXISTS idx_call_edges_file ON call_edges(file_key, version);",
        );
    }

    fn ensure_idx_ast_nodes_symbol(&self) {
        let _ = self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_ast_nodes_symbol ON ast_nodes(kind, symbol)",
        );
    }

    fn ensure_idx_call_edges_callee(&self) {
        let _ = self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_call_edges_callee ON call_edges(callee, version)",
        );
    }

    fn exec_cached(&self, sql: &str, params: impl rusqlite::Params) {
        if let Ok(mut stmt) = self.conn.prepare_cached(sql) {
            let _ = stmt.execute(params);
        }
    }

    /// prepare_cached + query_map + flatten (empty Vec on prepare/query failure).
    fn query_mapped<T, P, F>(&self, sql: &str, params: P, f: F) -> Vec<T>
    where
        P: rusqlite::Params,
        F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        let mut out = Vec::new();
        if let Ok(mut stmt) = self.conn.prepare_cached(sql) {
            if let Ok(rows) = stmt.query_map(params, f) {
                out.extend(rows.flatten());
            }
        }
        out
    }

    fn query_one<T, P, F>(&self, sql: &str, params: P, f: F) -> Option<T>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        self.conn
            .prepare_cached(sql)
            .ok()?
            .query_row(params, f)
            .ok()
    }

    pub fn insert_symbol_node(
        &self,
        file_key: &str,
        span_start: i64,
        span_end: i64,
        symbol: &str,
        kind: &str,
        version: i64,
    ) {
        self.exec_cached(
            SQL_INSERT_AST_NODE,
            params![file_key, kind, span_start, span_end, symbol, version],
        );
    }

    pub fn insert_import_node(
        &self,
        file_key: &str,
        span_start: i64,
        span_end: i64,
        symbol: &str,
        version: i64,
    ) {
        self.insert_symbol_node(file_key, span_start, span_end, symbol, "import", version);
    }

    pub fn insert_call_edge(
        &self,
        file_key: &str,
        caller: &str,
        callee: &str,
        line: i64,
        version: i64,
    ) {
        self.exec_cached(
            SQL_INSERT_CALL_EDGE,
            params![file_key, caller, callee, line, version],
        );
    }

    pub fn clear_all(&self) {
        let _ = self.conn.execute_batch(
            "DROP INDEX IF EXISTS idx_ast_nodes_symbol;
             DROP INDEX IF EXISTS idx_ast_nodes_file;
             DROP INDEX IF EXISTS idx_call_edges_callee;
             DROP INDEX IF EXISTS idx_call_edges_file;
             DELETE FROM ast_nodes;
             DELETE FROM call_edges;",
        );
    }

    pub fn clear_for_file(&self, file_key: &str) {
        self.exec_cached(SQL_DELETE_AST_NODES_FILE, params![file_key]);
        self.clear_call_edges_for_file(file_key);
    }

    pub fn clear_call_edges_for_file(&self, file_key: &str) {
        self.exec_cached(SQL_DELETE_CALL_EDGES_FILE, params![file_key]);
    }

    /// Prior symbol/import spans for one file (any version). Used by watch
    /// reindex AST-diff upsert so unchanged rows stay put.
    pub fn list_spans_for_file(&self, file_key: &str) -> Vec<AstSpanKey> {
        self.query_mapped(SQL_SELECT_SPANS_FOR_FILE, params![file_key], |r| {
            Ok(AstSpanKey {
                kind: r.get::<_, String>(0)?,
                symbol: r.get::<_, String>(1)?,
                span_start: r.get::<_, i64>(2)?,
                span_end: r.get::<_, i64>(3)?,
            })
        })
    }

    fn delete_span(&self, file_key: &str, span: &AstSpanKey) {
        self.exec_cached(
            SQL_DELETE_AST_SPAN,
            params![
                file_key,
                span.kind,
                span.symbol,
                span.span_start,
                span.span_end
            ],
        );
    }

    /// Rewrite only changed/added/removed symbol+import rows for `file_key`.
    /// Unchanged spans are left in place (no DELETE/INSERT). Call edges are
    /// not touched here — callers clear+reinsert those separately when needed.
    pub fn upsert_spans_diff(
        &self,
        file_key: &str,
        desired: &[AstSpanKey],
        version: i64,
    ) -> AstSpanDiffStats {
        let prior: HashSet<AstSpanKey> = self.list_spans_for_file(file_key).into_iter().collect();
        let desired_set: HashSet<AstSpanKey> = desired.iter().cloned().collect();
        let mut stats = AstSpanDiffStats::default();
        for span in &prior {
            if desired_set.contains(span) {
                stats.kept += 1;
            } else {
                self.delete_span(file_key, span);
                stats.deleted += 1;
            }
        }
        for span in &desired_set {
            if prior.contains(span) {
                continue;
            }
            self.insert_symbol_node(
                file_key,
                span.span_start,
                span.span_end,
                &span.symbol,
                &span.kind,
                version,
            );
            stats.inserted += 1;
        }
        stats
    }

    pub fn has_rows(&self) -> bool {
        self.conn
            .query_row(SQL_SELECT_AST_NODE_EXISTS, [], |_| Ok(()))
            .is_ok()
    }

    fn map_ss(r: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String)> {
        Ok((r.get(0)?, r.get(1)?))
    }
    fn map_ssii(r: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String, i64, i64)> {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    }
    fn map_sssii(r: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String, String, i64, i64)> {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    }

    fn map_fn_span(r: &rusqlite::Row<'_>) -> rusqlite::Result<(String, i64, i64)> {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    }

    pub fn query_all_symbols(&self, version: i64) -> Vec<(String, String)> {
        self.query_mapped(SQL_QUERY_ALL_SYMBOLS, params![version], Self::map_ss)
    }

    /// `pat` must already be LIKE-escaped with `\` by the caller.
    pub fn query_symbols_like(
        &self,
        pat: &str,
        version: i64,
    ) -> Vec<(String, String, String, i64, i64)> {
        self.ensure_idx_ast_nodes_symbol();
        self.query_mapped(
            SQL_QUERY_SYMBOLS_LIKE,
            params![pat, version],
            Self::map_sssii,
        )
    }

    pub fn query_imports(&self, version: i64) -> Vec<(String, String, i64, i64)> {
        self.query_mapped(SQL_QUERY_IMPORTS, params![version], Self::map_ssii)
    }

    pub fn query_callers(&self, callee: &str, version: i64) -> Vec<(String, String)> {
        self.ensure_idx_call_edges_callee();
        self.ensure_idx_ast_nodes_symbol();
        self.query_mapped(SQL_QUERY_CALLERS, params![callee, version], Self::map_ss)
    }

    pub fn fn_span(&self, symbol: &str, version: i64) -> Option<(String, i64, i64)> {
        self.query_one(SQL_FN_SPAN, params![symbol, version], Self::map_fn_span)
    }

    pub fn fn_span_any(&self, symbol: &str) -> Option<(String, i64, i64)> {
        self.query_one(SQL_FN_SPAN_ANY, params![symbol], Self::map_fn_span)
    }

    pub fn node_count(&self) -> i64 {
        self.conn
            .query_row(SQL_COUNT_AST_NODES, [], |r| r.get(0))
            .unwrap_or(0)
    }
}
