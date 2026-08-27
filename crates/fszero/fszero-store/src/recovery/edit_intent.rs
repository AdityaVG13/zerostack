//! Durable write-ahead intents for crash-atomic edits.
use super::{RecoveryStore, sql_int, sql_text};
use fsqlite::{Connection, Row, SqliteValue};
use std::fs;
use std::path::{Path, PathBuf};

const DDL: &str = "CREATE TABLE IF NOT EXISTS edit_intents (id INTEGER PRIMARY KEY, root TEXT NOT NULL, path TEXT NOT NULL, state TEXT NOT NULL, pre BLOB NOT NULL, post BLOB NOT NULL, pre_ref TEXT NOT NULL, post_ref TEXT NOT NULL, pre_mtime_ns INTEGER NOT NULL, pre_mode INTEGER NOT NULL, pre_xattrs TEXT NOT NULL, created_ns INTEGER NOT NULL)";
fn blob(v: &[u8]) -> SqliteValue {
    SqliteValue::Blob(v.to_vec().into())
}
fn ensure_schema(c: &Connection) -> Result<(), String> {
    c.execute(DDL)
        .map(|_| ())
        .map_err(|e| format!("edit intent schema failed: {e}"))
}
fn open(path: &Path) -> Result<Connection, String> {
    let c = Connection::open(path.to_string_lossy().into_owned())
        .map_err(|e| format!("edit intent open failed: {e}"))?;
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;").map_err(|e|format!("edit intent pragmas failed: {e}"))?;
    ensure_schema(&c)?;
    Ok(c)
}
fn row_bytes(row: &Row, n: usize) -> Vec<u8> {
    match row.get(n) {
        Some(SqliteValue::Blob(v)) => v.as_ref().to_vec(),
        Some(SqliteValue::Text(v)) => v.as_bytes().to_vec(),
        _ => Vec::new(),
    }
}
fn row_text(row: &Row, n: usize) -> String {
    String::from_utf8_lossy(&row_bytes(row, n)).into_owned()
}
fn row_int(row: &Row, n: usize) -> i64 {
    match row.get(n) {
        Some(SqliteValue::Integer(v)) => *v,
        _ => 0,
    }
}

impl RecoveryStore {
    fn intent_connection(&self) -> Result<Connection, String> {
        self.db_path
            .as_deref()
            .ok_or_else(|| "durable edit intents require durable store".to_string())
            .and_then(open)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn create_edit_intent(
        &self,
        root: &str,
        path: &str,
        pre: &[u8],
        post: &[u8],
        pre_ref: &str,
        post_ref: &str,
        mtime: i64,
        mode: i64,
        xattrs: &str,
    ) -> Result<i64, String> {
        if self.db_path.is_none() {
            return Ok(0);
        }
        if self.exec_txn_active.get() {
            return Err(
                "edit intent requires the outer execution transaction to be suspended".into(),
            );
        }
        let c = self.intent_connection()?;
        c.execute("BEGIN IMMEDIATE")
            .map_err(|e| format!("edit intent begin failed: {e}"))?;
        let id = match c
            .query("SELECT COALESCE(MAX(id),0)+1 FROM edit_intents")
            .ok()
            .and_then(|r| r.first().cloned())
            .and_then(|r| match r.get(0) {
                Some(SqliteValue::Integer(v)) => Some(*v),
                _ => None,
            }) {
            Some(v) => v,
            None => 1,
        };
        let result=c.execute_with_params("INSERT INTO edit_intents(id,root,path,state,pre,post,pre_ref,post_ref,pre_mtime_ns,pre_mode,pre_xattrs,created_ns) VALUES(?1,?2,?3,'prepared',?4,?5,?6,?7,?8,?9,?10,?11)",&[sql_int(id),sql_text(root),sql_text(path),blob(pre),blob(post),sql_text(pre_ref),sql_text(post_ref),sql_int(mtime),sql_int(mode),sql_text(xattrs),sql_int(super::unix_epoch_nanos() as i64)]).map_err(|e|format!("edit intent insert failed: {e}"));
        match result {
            Ok(_) => {
                c.execute("COMMIT")
                    .map_err(|e| format!("edit intent commit failed: {e}"))?;
                self.note_durable_mutation();
                Ok(id)
            }
            Err(e) => {
                let _ = c.execute("ROLLBACK");
                Err(e)
            }
        }
    }
    pub fn set_edit_intent_refs(
        &self,
        id: i64,
        pre_ref: &str,
        post_ref: &str,
    ) -> Result<(), String> {
        if id == 0 {
            return Ok(());
        }
        if !self.exec_txn_active.get() {
            return Err("edit intent refs require the evidence transaction".into());
        }
        self.conn
            .execute_with_params(
                "UPDATE edit_intents SET pre_ref=?1,post_ref=?2,state='evidence_ready' WHERE id=?3",
                &[sql_text(pre_ref), sql_text(post_ref), sql_int(id)],
            )
            .map(|_| ())
            .map_err(|e| format!("edit intent evidence update failed: {e}"))
    }
    pub fn clear_edit_intent(&self, id: i64) -> Result<(), String> {
        if id == 0 {
            return Ok(());
        }
        if self.exec_txn_active.get() {
            return self
                .conn
                .execute_with_params("DELETE FROM edit_intents WHERE id=?1", &[sql_int(id)])
                .map(|_| ())
                .map_err(|e| format!("edit intent finalize failed: {e}"));
        }
        let c = self.intent_connection()?;
        c.execute("BEGIN IMMEDIATE").map_err(|e| e.to_string())?;
        let r = c
            .execute_with_params("DELETE FROM edit_intents WHERE id=?1", &[sql_int(id)])
            .map_err(|e| e.to_string());
        match r {
            Ok(_) => {
                c.execute("COMMIT").map(|_| ()).map_err(|e| e.to_string())?;
                self.note_durable_mutation();
                Ok(())
            }
            Err(e) => {
                let _ = c.execute("ROLLBACK");
                Err(e)
            }
        }
    }
    #[cfg(test)]
    pub fn edit_intent_count(&self) -> usize {
        self.intent_connection()
            .ok()
            .and_then(|c| c.query("SELECT COUNT(*) FROM edit_intents").ok())
            .and_then(|r| r.first().cloned())
            .and_then(|r| match r.get(0) {
                Some(SqliteValue::Integer(v)) => Some(*v as usize),
                _ => None,
            })
            .unwrap_or(0)
    }
    pub fn has_edit_intent(&self, root: &str, path: &str) -> bool {
        self.intent_connection()
            .ok()
            .and_then(|c| {
                c.query_with_params(
                    "SELECT COUNT(*) FROM edit_intents WHERE root=?1 AND path=?2",
                    &[sql_text(root), sql_text(path)],
                )
                .ok()
            })
            .and_then(|rows| rows.first().cloned())
            .is_some_and(|row| row_int(&row, 0) > 0)
    }
    pub fn reconcile_edit_intents(&self, workspace: &Path) -> Result<(), String> {
        let c = self.intent_connection()?;
        c.execute("BEGIN IMMEDIATE")
            .map_err(|e| format!("edit intent reconcile begin failed: {e}"))?;
        let rows = match c.query(
            "SELECT id,root,path,state,pre,post,pre_mtime_ns,pre_mode,pre_xattrs FROM edit_intents",
        ) {
            Ok(v) => v,
            Err(e) => {
                let _ = c.execute("ROLLBACK");
                return Err(format!("edit intent scan failed: {e}"));
            }
        };
        let mut failure = None;
        let workspace_canon = fs::canonicalize(workspace)
            .map_err(|error| format!("edit intent workspace root failed: {error}"))?;
        for row in rows {
            let id = row_int(&row, 0);
            let declared_root = PathBuf::from(row_text(&row, 1));
            let relative = PathBuf::from(row_text(&row, 2));
            let state = row_text(&row, 3);
            let pre = row_bytes(&row, 4);
            let post = row_bytes(&row, 5);
            let mtime = row_int(&row, 6);
            let mode = row_int(&row, 7);
            let xattrs = row_text(&row, 8);
            let declared_canon = match fs::canonicalize(&declared_root) {
                Ok(path) => path,
                Err(error) => {
                    let _ = c.execute_with_params(
                        "UPDATE edit_intents SET state='indeterminate' WHERE id=?1",
                        &[sql_int(id)],
                    );
                    failure = Some(format!("edit intent {id} root unavailable: {error}"));
                    break;
                }
            };
            if declared_canon != workspace_canon
                || relative.is_absolute()
                || relative.components().any(|component| {
                    matches!(
                        component,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )
                })
            {
                let _ = c.execute_with_params(
                    "UPDATE edit_intents SET state='indeterminate' WHERE id=?1",
                    &[sql_int(id)],
                );
                failure = Some(format!("edit intent {id} outside workspace root"));
                break;
            }
            let target = match crate::path::validate_rollback_path(&workspace_canon, &relative) {
                Ok(path) => path,
                Err(error) => {
                    let _ = c.execute_with_params(
                        "UPDATE edit_intents SET state='indeterminate' WHERE id=?1",
                        &[sql_int(id)],
                    );
                    failure = Some(format!("edit intent {id} unsafe target: {error}"));
                    break;
                }
            };
            let delete_intent = || {
                c.execute_with_params("DELETE FROM edit_intents WHERE id=?1", &[sql_int(id)])
                    .map_err(|e| e.to_string())
                    .map(|_| ())
            };
            // Metadata only: fs::read on a FIFO/socket blocks the worker.
            if let Err(e) = crate::path::refuse_non_regular_file(&target) {
                let _ = c.execute_with_params(
                    "UPDATE edit_intents SET state='indeterminate' WHERE id=?1",
                    &[sql_int(id)],
                );
                failure = Some(format!("edit intent {id} {e}"));
                break;
            }
            let current = fs::read(&target);
            let action = match current {
                Ok(ref bytes) if state == "evidence_ready" && *bytes == post => delete_intent(),
                _ if state == "evidence_ready" => Err(format!(
                    "edit intent {id} committed evidence but target is not the postimage"
                )),
                Ok(ref bytes) if *bytes == pre => crate::path::set_mode(&target, mode)
                    .and_then(|_| crate::path::restore_xattrs(&target, &xattrs))
                    .and_then(|_| crate::path::set_mtime_ns(&target, mtime))
                    .and_then(|_| crate::path::sync_file(&target))
                    .and_then(|_| delete_intent()),
                Ok(ref bytes) if *bytes == post => crate::path::atomic_write(&target, &pre)
                    .and_then(|_| crate::path::set_mode(&target, mode))
                    .and_then(|_| crate::path::restore_xattrs(&target, &xattrs))
                    .and_then(|_| crate::path::set_mtime_ns(&target, mtime))
                    .and_then(|_| crate::path::sync_file(&target))
                    .and_then(|_| delete_intent()),
                Err(ref error)
                    if post.is_empty() && error.kind() == std::io::ErrorKind::NotFound =>
                {
                    crate::path::atomic_write(&target, &pre)
                        .and_then(|_| crate::path::set_mode(&target, mode))
                        .and_then(|_| crate::path::restore_xattrs(&target, &xattrs))
                        .and_then(|_| crate::path::set_mtime_ns(&target, mtime))
                        .and_then(|_| crate::path::sync_file(&target))
                        .and_then(|_| delete_intent())
                }
                _ => Err(format!(
                    "edit intent {id} is neither preimage nor postimage"
                )),
            };
            if let Err(e) = action {
                let _ = c.execute_with_params(
                    "UPDATE edit_intents SET state='indeterminate' WHERE id=?1",
                    &[sql_int(id)],
                );
                failure = Some(format!("edit intent {id} reconciliation failed: {e}"));
                break;
            }
        }
        c.execute("COMMIT")
            .map_err(|e| format!("edit intent reconcile commit failed: {e}"))?;
        self.note_durable_mutation();
        match failure {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::FileTypeExt;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn reconcile_edit_intents_refuses_fifo_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("ws");
        fs::create_dir(&workspace).unwrap();
        let fifo = workspace.join("pipe.fifo");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("spawn mkfifo");
        assert!(status.success(), "mkfifo failed: {status}");

        let db = dir.path().join("store.sqlite3");
        let root = workspace.to_str().expect("utf8 workspace").to_string();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let store = RecoveryStore::with_durable(&db);
            store
                .create_edit_intent(&root, "pipe.fifo", b"pre", b"post", "", "", 0, 0, "{}")
                .expect("insert edit intent");
            let _ = tx.send(store.reconcile_edit_intents(Path::new(&root)));
        });
        let result = rx
            .recv_timeout(Duration::from_millis(1500))
            .expect("reconcile_edit_intents hung on FIFO instead of failing closed");
        let err = result.expect_err("FIFO reconcile must fail closed");
        assert!(
            err.contains("unsupported file kind") && err.contains("fifo"),
            "expected unsupported file kind fifo, got {err}"
        );
        let meta = fs::symlink_metadata(&fifo).expect("fifo metadata");
        assert!(
            meta.file_type().is_fifo(),
            "{} must remain a FIFO",
            fifo.display()
        );
    }
}
