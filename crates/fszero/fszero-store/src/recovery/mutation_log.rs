//! Mutation journal (fs.history / fs.undo durable basis).
use fsqlite::{Row, SqliteValue};

use super::{RecoveryStore, int_col, query_i64, sql_int, sql_text, text_col};

const MUTATION_ROW_COLS: &str =
    "seq, ts, op, path, pre_ref, post_ref, created, agent, pre_mtime_ns, pre_mode, pre_xattrs";
const SQL_NEXT_MUTATION_SEQ: &str = "SELECT COALESCE(MAX(seq), 0) + 1 FROM mutation_log";
const SQL_INSERT_MUTATION: &str = "INSERT INTO mutation_log (seq, ts, op, path, pre_ref, post_ref, created, session_window, agent, pre_mtime_ns, pre_mode, pre_xattrs) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)";
const SQL_MUTATIONS_AFTER: &str = "SELECT seq, ts, op, path, pre_ref, post_ref, created, agent, pre_mtime_ns, pre_mode, pre_xattrs FROM mutation_log WHERE seq > ?1 ORDER BY seq ASC LIMIT ?2";

/// One mutation-journal row (the durable basis for fs.history / fs.undo).
#[derive(Debug, Clone)]
pub struct MutationRow {
    pub seq: i64,
    pub ts: i64,
    pub op: String,
    pub path: String,
    pub pre_ref: String,
    pub post_ref: String,
    pub created: bool,
    pub agent: String,
    /// File mtime before the mutation, ns since UNIX_EPOCH (0 = unknown).
    pub pre_mtime_ns: i64,
    /// Permission bits before the mutation, `& 0o7777` (-1 = unknown).
    pub pre_mode: i64,
    /// Extended attributes before the mutation as JSON name->hex
    /// ('' = unknown, '{}' = readable and none present).
    pub pre_xattrs: String,
}

impl RecoveryStore {
    /// Append one row to the mutation journal. `pre_ref`/`post_ref` are
    /// content-addressed fz://blob refs (empty pre_ref + created=true for
    /// files that did not exist). `pre_mtime_ns` is the file's mtime before
    /// the mutation, nanoseconds since UNIX_EPOCH (0 = unknown/new file), so
    /// undo can restore the timestamp bit-perfect (fszero-md6). `pre_mode`
    /// is the pre-mutation permission bits, -1 = unknown (fszero-7be).
    #[allow(clippy::too_many_arguments)]
    pub fn append_mutation(
        &mut self,
        ts: i64,
        op: &str,
        path: &str,
        pre_ref: &str,
        post_ref: &str,
        created: bool,
        session_window: i64,
        agent: &str,
        pre_mtime_ns: i64,
        pre_mode: i64,
        pre_xattrs: &str,
    ) -> Result<i64, String> {
        // Fail-closed (fszero-w2g.12 / .46): never ack a mutation whose
        // journal INSERT failed — silent holes break undo/history.
        let seq = query_i64(&self.conn, SQL_NEXT_MUTATION_SEQ).unwrap_or(1);
        self.conn
            .execute_with_params(
                SQL_INSERT_MUTATION,
                &[
                    sql_int(seq),
                    sql_int(ts),
                    sql_text(op),
                    sql_text(path),
                    sql_text(pre_ref),
                    sql_text(post_ref),
                    sql_int(i64::from(created)),
                    sql_int(session_window),
                    sql_text(agent),
                    sql_int(pre_mtime_ns),
                    sql_int(pre_mode),
                    sql_text(pre_xattrs),
                ],
            )
            .map_err(|e| {
                let msg = format!("mutation journal insert failed: {e}");
                self.last_store_error = Some(msg.clone());
                msg
            })?;
        Ok(seq)
    }

    /// One-query, ascending, bounded journal page after an exclusive cursor.
    /// SQL failures remain errors so feed callers cannot mistake them for EOF.
    pub fn query_mutations_after(
        &self,
        after_seq: i64,
        limit: usize,
    ) -> Result<Vec<MutationRow>, String> {
        let bounded_limit = i64::try_from(limit).unwrap_or(i64::MAX);
        self.conn
            .query_with_params(
                SQL_MUTATIONS_AFTER,
                &[sql_int(after_seq), sql_int(bounded_limit)],
            )
            .map(|rows| rows.into_iter().map(mutation_row_from).collect())
            .map_err(|error| format!("mutation journal feed query failed: {error}"))
    }

    /// Mutation journal rows, newest first. `path` filters when non-empty;
    /// `seq` selects one exact row (path ignored).
    pub fn query_mutations(&self, path: &str, seq: Option<i64>, limit: usize) -> Vec<MutationRow> {
        let (sql, params): (String, Vec<SqliteValue>) = match (seq, path.is_empty()) {
            (Some(s), _) => (
                format!("SELECT {MUTATION_ROW_COLS} FROM mutation_log WHERE seq = ?1"),
                vec![sql_int(s)],
            ),
            (None, false) => (
                format!(
                    "SELECT {MUTATION_ROW_COLS} FROM mutation_log WHERE path = ?1 ORDER BY seq DESC LIMIT ?2"
                ),
                vec![sql_text(path), sql_int(limit as i64)],
            ),
            (None, true) => (
                format!("SELECT {MUTATION_ROW_COLS} FROM mutation_log ORDER BY seq DESC LIMIT ?1"),
                vec![sql_int(limit as i64)],
            ),
        };
        self.conn
            .query_with_params(&sql, &params)
            .map(|rows| rows.into_iter().map(mutation_row_from).collect())
            .unwrap_or_default()
    }
}

fn mutation_row_from(row: Row) -> MutationRow {
    MutationRow {
        seq: int_col(&row, 0),
        ts: int_col(&row, 1),
        op: text_col(&row, 2),
        path: text_col(&row, 3),
        pre_ref: text_col(&row, 4),
        post_ref: text_col(&row, 5),
        created: int_col(&row, 6) != 0,
        agent: text_col(&row, 7),
        pre_mtime_ns: int_col(&row, 8),
        pre_mode: int_col(&row, 9),
        pre_xattrs: text_col(&row, 10),
    }
}
