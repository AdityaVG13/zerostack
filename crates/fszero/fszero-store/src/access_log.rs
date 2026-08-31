//! Persistent access ledger: read/search/edit paths with content hashes.

use super::recovery::{RecoveryStore, int_col, query_i64, sql_int, sql_text, text_col};
use sha2::{Digest, Sha256};
use std::path::Path;

pub fn init_access_log_table(conn: &fsqlite::Connection) {
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS access_log (ts INTEGER NOT NULL, op TEXT NOT NULL, path TEXT NOT NULL, content_hash TEXT NOT NULL, session_window INTEGER NOT NULL);\
         CREATE INDEX IF NOT EXISTS idx_access_log_path ON access_log(path);\
         CREATE INDEX IF NOT EXISTS idx_access_log_ts ON access_log(ts);\
         CREATE INDEX IF NOT EXISTS idx_access_log_window ON access_log(session_window);",);
}

pub fn content_hash_from_ref(content_ref: &str) -> &str {
    content_ref
        .rsplit('/')
        .next()
        .filter(|h| !h.is_empty() && *h != "error")
        .unwrap_or("")
}

pub fn content_hash_bytes(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    fszero_core::hexutil::sha256_hex_of(h.finalize().into())
}

pub fn rel_path_for_log(root: Option<&Path>, full: &Path) -> String {
    rel_path_for_log_with_canon(root, None, full)
}

/// Prefer a session-cached `root_canon` so warm access-log rows avoid two
/// `canonicalize` syscalls when `full` is already a root-relative resolve.
#[inline]
fn slash_rel(rel: &Path) -> String {
    rel.to_string_lossy().replace('\\', "/")
}

pub fn rel_path_for_log_with_canon(
    root: Option<&Path>,
    root_canon: Option<&Path>,
    full: &Path,
) -> String {
    if let Some(rc) = root_canon {
        if let Ok(rel) = full.strip_prefix(rc) {
            return slash_rel(rel);
        }
    }
    if let Some(root) = root {
        if let Ok(canon_root) = std::fs::canonicalize(root) {
            if let Ok(canon) = std::fs::canonicalize(full) {
                if let Ok(rel) = canon.strip_prefix(&canon_root) {
                    return slash_rel(rel);
                }
            }
            // Deleted/renamed target: canonicalize(full) fails, but `full` usually IS already
            // canonical (resolved at staging time), so strip the canonical root directly —
            // otherwise a delete-vs-edit conflict logs an absolute path instead of a repo-relative one.
            if let Ok(rel) = full.strip_prefix(&canon_root) {
                return slash_rel(rel);
            }
        }
        if let Ok(rel) = full.strip_prefix(root) {
            return slash_rel(rel);
        }
    }
    slash_rel(full)
}

/// Flush access_log buffer after this many pending rows (or sooner on query).
const ACCESS_LOG_FLUSH_WATERMARK: usize = 64;
const SQL_INSERT_ACCESS_LOG: &str = "INSERT INTO access_log (ts, op, path, content_hash, session_window) VALUES (?1, ?2, ?3, ?4, ?5)";
const SQL_ACCESS_HOT: &str =
    "SELECT path, COUNT(*) AS c FROM access_log GROUP BY path ORDER BY c DESC, path ASC LIMIT ?1";
const SQL_ACCESS_RECENT: &str = "SELECT path, MAX(ts) AS last_ts FROM access_log GROUP BY path ORDER BY last_ts DESC, path ASC LIMIT ?1";
const SQL_ACCESS_COUNT: &str = "SELECT COUNT(*) FROM access_log";
/// Shared read/search co-access pair query (session_window join).
const SQL_ACCESS_COACCESS: &str = "SELECT a.path, b.path, COUNT(DISTINCT a.session_window) AS c FROM access_log a INNER JOIN access_log b ON a.session_window = b.session_window AND a.path < b.path WHERE a.op IN ('read', 'search') AND b.op IN ('read', 'search') GROUP BY a.path, b.path ORDER BY c DESC, a.path ASC, b.path ASC LIMIT ?1";
const SQL_ACCESS_COACCESS_FOR_PATH: &str = "SELECT other.path, COUNT(DISTINCT other.session_window) AS c FROM access_log anchor INNER JOIN access_log other ON anchor.session_window = other.session_window AND anchor.path != other.path WHERE anchor.path = ?1 AND anchor.op IN ('read', 'search') AND other.op IN ('read', 'search') GROUP BY other.path ORDER BY c DESC, other.path ASC";

impl RecoveryStore {
    pub fn append_access_log(
        &mut self,
        ts: i64,
        op: &str,
        path: &str,
        content_hash: &str,
        session_window: i64,
    ) {
        self.pending_access.borrow_mut().push((
            ts,
            op.to_string(),
            path.to_string(),
            content_hash.to_string(),
            session_window,
        ));
        if self.pending_access.borrow().len() >= ACCESS_LOG_FLUSH_WATERMARK {
            self.flush_pending_access();
        }
    }

    /// Persist any buffered access_log rows. Idempotent; called before every
    /// access query so hot/recent/coaccess see the full stream.
    pub fn flush_pending_access(&self) {
        let rows = {
            let mut pending = self.pending_access.borrow_mut();
            if pending.is_empty() {
                return;
            }
            std::mem::take(&mut *pending)
        };
        // One transaction for the batch (fail-open if already in a txn).
        let began = self.conn.execute("BEGIN").is_ok();
        for (ts, op, path, content_hash, session_window) in rows {
            let _ = self.conn.execute_with_params(
                SQL_INSERT_ACCESS_LOG,
                &[
                    sql_int(ts),
                    sql_text(&op),
                    sql_text(&path),
                    sql_text(&content_hash),
                    sql_int(session_window),
                ],
            );
        }
        if began {
            let _ = self.conn.execute("COMMIT");
        }
    }

    /// Map rows to (text0, int1) after a parameterized query (empty on fail).
    fn query_text_int_pairs(
        &self,
        sql: &str,
        params: &[fsqlite::SqliteValue],
    ) -> Vec<(String, i64)> {
        let mut out = Vec::new();
        if let Ok(rows) = self.conn.query_with_params(sql, params) {
            for row in rows {
                out.push((text_col(&row, 0), int_col(&row, 1)));
            }
        }
        out
    }

    /// Shared path+metric query after flushing pending access rows.
    fn query_access_path_metric(&self, sql: &str, limit: usize) -> Vec<(String, i64)> {
        self.flush_pending_access();
        self.query_text_int_pairs(sql, &[sql_int(limit as i64)])
    }

    pub fn query_access_hot(&self, limit: usize) -> Vec<(String, i64)> {
        self.query_access_path_metric(SQL_ACCESS_HOT, limit)
    }

    pub fn query_access_recent(&self, limit: usize) -> Vec<(String, i64)> {
        self.query_access_path_metric(SQL_ACCESS_RECENT, limit)
    }

    pub fn query_access_coaccess(&self, limit: usize) -> Vec<(String, String, i64)> {
        self.flush_pending_access();
        let mut out = Vec::new();
        if let Ok(rows) = self
            .conn
            .query_with_params(SQL_ACCESS_COACCESS, &[sql_int(limit as i64)])
        {
            for row in rows {
                out.push((text_col(&row, 0), text_col(&row, 1), int_col(&row, 2)));
            }
        }
        out
    }

    pub fn query_coaccess_for_path(&self, path: &str) -> Vec<(String, i64)> {
        self.flush_pending_access();
        self.query_text_int_pairs(SQL_ACCESS_COACCESS_FOR_PATH, &[sql_text(path)])
    }

    pub fn access_log_row_count(&self) -> usize {
        self.flush_pending_access();
        query_i64(&self.conn, SQL_ACCESS_COUNT).unwrap_or(0) as usize
    }
}

impl Drop for RecoveryStore {
    fn drop(&mut self) {
        self.flush_pending_access();
    }
}
