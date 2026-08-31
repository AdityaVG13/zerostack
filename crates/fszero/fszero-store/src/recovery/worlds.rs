//! Speculative world registry (active worlds + edit rows). Persist / rehydrate caller-local worlds
//! across process restart. Synchronous no daemon; world state lives in the same durable recovery store.

use super::{RecoveryStore, meta_i64, sql_int, sql_text, text_col, unix_epoch_secs};

const SQL_SELECT_ACTIVE_WORLDS: &str =
    "SELECT wid, cert_ref FROM worlds WHERE state = 'active' ORDER BY created_ts ASC";
const SQL_SELECT_COMMITTING_WORLDS: &str =
    "SELECT wid FROM worlds WHERE state = 'committing' ORDER BY created_ts ASC";
const SQL_SELECT_WORLD_EDITS: &str =
    "SELECT path, cert_ref FROM world_edits WHERE wid = ?1 ORDER BY ord ASC";
const SQL_DELETE_WORLD_EDITS: &str = "DELETE FROM world_edits WHERE wid = ?1";
const SQL_INSERT_WORLD_ACTIVE: &str = "INSERT OR REPLACE INTO worlds (wid, state, cert_ref, created_ts, session_window) VALUES (?1, 'active', ?2, ?3, ?4)";
const SQL_INSERT_WORLD_EDIT: &str =
    "INSERT INTO world_edits (wid, ord, path, cert_ref) VALUES (?1, ?2, ?3, ?4)";
const SQL_UPDATE_WORLD_STATE: &str = "UPDATE worlds SET state = ?1 WHERE wid = ?2";

impl RecoveryStore {
    /// Persist an active speculative world. Synchronous — no daemon.
    pub fn upsert_active_world(
        &mut self,
        wid: &str,
        cert_ref: &str,
        edits: &[(String, String)],
        session_window: i64,
    ) -> Result<(), String> {
        let ts = unix_epoch_secs();
        self.exec_params_ctx(
            SQL_INSERT_WORLD_ACTIVE,
            &[
                sql_text(wid),
                sql_text(cert_ref),
                sql_int(ts),
                sql_int(session_window),
            ],
            "world upsert failed",
        )?;
        self.delete_world_edits(wid);
        for (ord, (path, edit_cert)) in edits.iter().enumerate() {
            self.exec_params_ctx(
                SQL_INSERT_WORLD_EDIT,
                &[
                    sql_text(wid),
                    sql_int(ord as i64),
                    sql_text(path.as_str()),
                    sql_text(edit_cert.as_str()),
                ],
                "world_edits insert failed",
            )?;
        }
        let next = wid
            .strip_prefix('W')
            .and_then(|n| n.parse::<i64>().ok())
            .map(|n| n + 1)
            .unwrap_or(1);
        let _ = self.put_meta_i64("world_next_id", next);
        Ok(())
    }

    fn delete_world_edits(&mut self, wid: &str) {
        let _ = self.exec_params(SQL_DELETE_WORLD_EDITS, &[sql_text(wid)]);
    }

    pub fn set_world_state(&mut self, wid: &str, state: &str) -> Result<(), String> {
        self.exec_params_ctx(
            SQL_UPDATE_WORLD_STATE,
            &[sql_text(state), sql_text(wid)],
            "world state update failed",
        )?;
        // 'committing' is a crash-recovery waypoint, not a terminal state: the edit rows must
        // survive it so a compensating rollback can re-publish the world as active and a caller can retry.
        if state == "committed" || state == "dropped" {
            self.delete_world_edits(wid);
        }
        Ok(())
    }

    /// Rows needed to rehydrate active worlds after process restart.
    pub fn list_active_world_rows(&self) -> Vec<(String, String, Vec<(String, String)>)> {
        let Ok(world_rows) = self.conn.query(SQL_SELECT_ACTIVE_WORLDS) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for row in world_rows {
            let wid = text_col(&row, 0);
            let cert_ref = text_col(&row, 1);
            if wid.is_empty() {
                continue;
            }
            let edits = self
                .conn
                .query_with_params(SQL_SELECT_WORLD_EDITS, &[sql_text(wid.as_str())])
                .map(|ers| {
                    ers.into_iter()
                        .map(|er| (text_col(&er, 0), text_col(&er, 1)))
                        .collect()
                })
                .unwrap_or_default();
            out.push((wid, cert_ref, edits));
        }
        out
    }

    /// Worlds killed mid-publish: state was moved to 'committing' before the
    /// first workspace write and only leaves it on ack or rollback.
    pub fn list_committing_worlds(&self) -> Vec<String> {
        let Ok(rows) = self.conn.query(SQL_SELECT_COMMITTING_WORLDS) else {
            return Vec::new();
        };
        rows.iter()
            .map(|row| text_col(row, 0))
            .filter(|wid| !wid.is_empty())
            .collect()
    }

    pub fn load_world_next_id(&self) -> u32 {
        meta_i64(&self.conn, "world_next_id")
            .map(|v| (v as u32).max(1))
            .unwrap_or(1)
    }
}
