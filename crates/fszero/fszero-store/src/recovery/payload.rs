//! Payload put/get, pack encode/decode, batch flush, transient LRU.
use fsqlite::SqliteValue;
use std::collections::BTreeMap;
use std::sync::Arc;

use super::pack::*;
use super::{MAX_TRANSIENT_PAYLOADS, RecoveryStore};

/// Flush batch buffer at this many staged bytes (peak-RSS bound + sorted runs).
const PENDING_PAYLOAD_FLUSH_BYTES: usize = 32 * 1024 * 1024;
/// Bound dynamic SQL and parameter vectors while amortizing fsqlite statement setup.
pub(super) const PENDING_PAYLOAD_SQL_BATCH_ROWS: usize = 128;

type EncodedPendingPayload = (String, Arc<[u8]>, Arc<[u8]>);

/// Env-gated double-barrier timing for durable packed puts.
/// When `FSZERO_DURABLE_PUT_PHASES=1`, emit one JSON line on stderr with
/// `pack_sync_us`, `commit_us`, `pack_dirty`, `bytes`, and related fields.
fn durable_put_phases_enabled() -> bool {
    match std::env::var("FSZERO_DURABLE_PUT_PHASES") {
        Ok(v) => matches!(v.as_str(), "1" | "true" | "TRUE" | "on" | "yes"),
        Err(_) => false,
    }
}

fn emit_durable_put_phases(fields: serde_json::Value) {
    if !durable_put_phases_enabled() {
        return;
    }
    eprintln!("{fields}");
}

impl RecoveryStore {
    pub fn has_payload(&mut self, key: &str) -> bool {
        if let Some(pending) = self.pending_payloads.as_ref() {
            if pending.contains_key(key) {
                return true;
            }
        }
        if self
            .payload_key_cache
            .as_ref()
            .is_some_and(|cache| cache.contains(key))
        {
            return true;
        }
        let exists = self
            .conn
            .query_with_params(super::SQL_SELECT_PAYLOAD_EXISTS, &[super::sql_text(key)])
            .is_ok_and(|rows| !rows.is_empty());
        if exists
            && let Some(cache) = self.payload_key_cache.as_mut()
            && cache.len() < super::MAX_BATCH_PAYLOAD_KEY_CACHE
        {
            cache.insert(key.to_string());
        }
        exists
    }

    pub fn try_put_key(&mut self, key: &str, data: &[u8]) -> Result<(), String> {
        self.store_writes
            .set(self.store_writes.get().saturating_add(1));
        let payload: Arc<[u8]> = Arc::from(data.to_vec());
        let transient = key.starts_with("seq/");
        if !transient {
            self.mark_exec_txn_durable_dirty();
        }
        if self.try_buffer_payload(key, &payload, transient)? {
            return Ok(());
        }

        // Rotation and append use the same SQLite→pack lock order. Packed writes therefore cannot publish
        // a locator into a generation another process just retired. Transient bookkeeping is crash-atomic
        // too.
        let packed = payload.len() >= PACK_MIN_BYTES && !transient && self.pack.is_some();
        let requires_txn =
            (packed || transient || key.starts_with("mem://")) && self.pending_payloads.is_none();
        let began = self.begin_immediate_payload_txn(requires_txn)?;
        let persisted = self.persist_immediate_payload(key, &payload, transient, packed);
        self.finish_immediate_payload_txn(began, persisted, transient)?;
        Ok(())
    }

    fn try_buffer_payload(
        &mut self,
        key: &str,
        payload: &Arc<[u8]>,
        transient: bool,
    ) -> Result<bool, String> {
        if transient {
            return Ok(false);
        }
        let Some(pending) = self.pending_payloads.as_mut() else {
            return Ok(false);
        };
        let added = payload.len();
        if pending
            .insert(key.to_string(), Arc::clone(payload))
            .is_none()
        {
            self.pending_bytes += added;
        }
        if let Some(cache) = self.payload_key_cache.as_mut()
            && cache.len() < super::MAX_BATCH_PAYLOAD_KEY_CACHE
        {
            cache.insert(key.to_string());
        }
        if self.pending_bytes > PENDING_PAYLOAD_FLUSH_BYTES {
            self.flush_pending_payloads()?;
        }
        Ok(true)
    }

    fn persist_immediate_payload(
        &mut self,
        key: &str,
        payload: &[u8],
        transient: bool,
        allow_pack: bool,
    ) -> Result<(), String> {
        let (row, pack_dirty) = self.encode_payload_row(payload, allow_pack)?;
        let pack_sync_us = if pack_dirty {
            let t0 = std::time::Instant::now();
            self.sync_pack()?;
            Some(t0.elapsed().as_micros() as u64)
        } else {
            None
        };
        // Stash for finish_immediate_payload_txn emission (one line per put).
        self.last_pack_sync_us = pack_sync_us;
        self.last_pack_dirty = pack_dirty;
        self.last_put_bytes = payload.len();
        self.insert_payload_row(key, Arc::clone(&row))?;
        self.track_payload_open_maintenance(key, &row)?;
        if !transient {
            return Ok(());
        }
        self.touch_payload(key)?;
        self.prune_transient_payloads()?;
        self.put_meta_i64("next_id", self.next_id as i64)
    }

    fn begin_immediate_payload_txn(&mut self, required: bool) -> Result<bool, String> {
        if !required {
            return Ok(false);
        }
        self.conn
            .execute("BEGIN IMMEDIATE")
            .map(|_| true)
            .map_err(|e| format!("payload begin: {e}"))
    }

    fn finish_immediate_payload_txn(
        &mut self,
        began: bool,
        result: Result<(), String>,
        transient: bool,
    ) -> Result<(), String> {
        if !began {
            return result;
        }
        if let Err(error) = result {
            let _ = self.conn.execute("ROLLBACK");
            return Err(error);
        }
        // A transient transaction only touches inline `seq/` rows, their LRU ticks,
        // retention state, and counters that name future transient rows.
        let relaxed = transient && self.set_synchronous("NORMAL");
        let t0 = std::time::Instant::now();
        let committed = self.conn.execute("COMMIT").map(|_| ()).map_err(|error| {
            let _ = self.conn.execute("ROLLBACK");
            format!("payload commit: {error}")
        });
        let commit_us = t0.elapsed().as_micros() as u64;
        if relaxed {
            self.set_synchronous("FULL");
        }
        if committed.is_ok() {
            emit_durable_put_phases(serde_json::json!({
                "durable_put_phases_us": {
                    "pack_sync_us": self.last_pack_sync_us,
                    "commit_us": commit_us,
                },
                "pack_dirty": self.last_pack_dirty,
                "bytes": self.last_put_bytes,
                "transient": transient,
                "commit_relaxed": relaxed,
            }));
            if !transient {
                self.note_durable_mutation();
            }
        }
        committed
    }

    /// Steady state is FULL; this is only flipped around a transient-only
    /// commit and always restored. Reports whether the pragma took, so a
    /// failure to relax cannot leave the store stuck at NORMAL.
    pub(super) fn set_synchronous(&self, mode: &str) -> bool {
        self.conn
            .execute(&format!("PRAGMA synchronous={mode}"))
            .is_ok()
    }

    fn insert_payload_row(&mut self, key: &str, row: Arc<[u8]>) -> Result<(), String> {
        self.exec_params_ctx(
            super::SQL_INSERT_PAYLOAD_KV,
            &[super::sql_text(key), SqliteValue::Blob(row)],
            format!("store failed for {key}"),
        )
    }

    /// Tag + (for large payloads on durable stores) divert bytes to the pack
    /// sidecar, returning the row and whether the pack needs a durability
    /// barrier before its locator can commit.
    fn encode_payload_row(
        &mut self,
        data: &[u8],
        allow_pack: bool,
    ) -> Result<(Arc<[u8]>, bool), String> {
        if allow_pack && data.len() >= PACK_MIN_BYTES {
            if let Some((offset, len)) = self.append_active_pack(data) {
                return Ok((Arc::from(encode_packed_locator(offset, len)), true));
            }
        }
        let mut v = Vec::with_capacity(data.len() + 1);
        v.push(PAYLOAD_TAG_INLINE);
        v.extend_from_slice(data);
        Ok((Arc::from(v), false))
    }

    /// Caller holds SQLite's write transaction. Rebind stale process handles
    /// before locking and appending to the selected generation.
    fn append_active_pack(&mut self, data: &[u8]) -> Option<(u64, u32)> {
        let generation = load_pack_gen(&self.conn);
        self.refresh_pack_generation(generation).ok()?;
        let pack = self.pack.as_mut()?;
        pack.lock_exclusive().ok()?;
        let appended = pack.append_locked(data);
        pack.unlock();
        appended
    }

    fn sync_pack(&mut self) -> Result<(), String> {
        if let Some(pack) = self.pack.as_mut() {
            pack.sync_all()?;
        }
        Ok(())
    }

    /// Inverse of encode_payload_row. Legacy rows (written before tagging)
    /// have neither tag byte layout and are returned verbatim.
    /// Packed locators that cannot be read set `pack_integrity_error`.
    fn decode_payload_row(&self, key: &str, row: &[u8]) -> Option<Vec<u8>> {
        if let Some((offset, len)) = decode_packed_locator(row) {
            match self.pack.as_ref().and_then(|p| p.read(offset, len)) {
                Some(bytes) => Some(bytes),
                None => {
                    let detail = if self.pack.is_some() {
                        format!(
                            "pack_torn: {key} (offset={offset} len={len}; expand a fresh ref or reindex)"
                        )
                    } else {
                        format!("pack_torn: {key} (packed locator but no pack sidecar)")
                    };
                    self.pack_integrity_error.set(Some(detail));
                    None
                }
            }
        } else {
            match row.first() {
                Some(&PAYLOAD_TAG_INLINE) => Some(row[1..].to_vec()),
                _ => Some(row.to_vec()),
            }
        }
    }

    pub(super) fn flush_pending_payloads(&mut self) -> Result<(), String> {
        let Some(pending) = self.pending_payloads.take() else {
            return Ok(());
        };
        self.pending_payloads = Some(BTreeMap::new());
        self.pending_bytes = 0;
        let (encoded, pack_dirty) = match self.encode_pending_payloads(&pending) {
            Ok(encoded) => encoded,
            Err(error) => {
                self.restore_pending_payloads(pending);
                return Err(error);
            }
        };
        let pack_sync_us = if pack_dirty {
            let t0 = std::time::Instant::now();
            if let Err(error) = self.sync_pack() {
                self.restore_pending_payloads(pending);
                return Err(error);
            }
            Some(t0.elapsed().as_micros() as u64)
        } else {
            None
        };
        let bytes: usize = encoded.iter().map(|(_, v, _)| v.len()).sum();
        let result = self.insert_encoded_payloads(encoded);
        if result.is_ok() {
            emit_durable_put_phases(serde_json::json!({
                "durable_put_phases_us": {
                    "pack_sync_us": pack_sync_us,
                    "commit_us": null,
                },
                "pack_dirty": pack_dirty,
                "bytes": bytes,
                "path": "flush_pending",
            }));
        }
        result
    }

    fn encode_pending_payloads(
        &mut self,
        pending: &BTreeMap<String, Arc<[u8]>>,
    ) -> Result<(Vec<EncodedPendingPayload>, bool), String> {
        let mut encoded = Vec::with_capacity(pending.len());
        let mut pack_dirty = false;
        for (key, value) in pending {
            let allow_pack = !key.starts_with("seq/");
            let (row, dirty) = self.encode_payload_row(value, allow_pack)?;
            pack_dirty |= dirty;
            encoded.push((key.clone(), Arc::clone(value), row));
        }
        Ok((encoded, pack_dirty))
    }

    fn restore_pending_payloads(&mut self, remaining: BTreeMap<String, Arc<[u8]>>) {
        if let Some(pending) = self.pending_payloads.as_mut() {
            pending.extend(remaining);
        } else {
            self.pending_payloads = Some(remaining);
        }
        self.pending_bytes = self
            .pending_payloads
            .as_ref()
            .map(|pending| pending.values().map(|value| value.len()).sum())
            .unwrap_or(0);
    }

    fn insert_encoded_payloads(
        &mut self,
        encoded: Vec<EncodedPendingPayload>,
    ) -> Result<(), String> {
        let persisted = self
            .insert_encoded_payload_chunks(&encoded)
            .and_then(|()| self.track_encoded_payload_maintenance(&encoded));
        let Err(error) = persisted else {
            return Ok(());
        };
        self.restore_pending_payloads(
            encoded
                .into_iter()
                .map(|(key, value, _)| (key, value))
                .collect(),
        );
        Err(error)
    }

    fn insert_encoded_payload_chunks(
        &mut self,
        encoded: &[EncodedPendingPayload],
    ) -> Result<(), String> {
        for chunk in encoded.chunks(PENDING_PAYLOAD_SQL_BATCH_ROWS) {
            let mut sql = String::from("INSERT OR REPLACE INTO payloads (key, value) VALUES ");
            let mut params = Vec::with_capacity(chunk.len() * 2);
            for (index, (key, _, row)) in chunk.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                let key_param = index * 2 + 1;
                let value_param = key_param + 1;
                sql.push_str(&format!("(?{key_param}, ?{value_param})"));
                params.push(super::sql_text(key));
                params.push(SqliteValue::Blob(Arc::clone(row)));
            }
            self.exec_params_ctx(&sql, &params, "store failed for buffered payload batch")?;
        }
        Ok(())
    }

    fn track_encoded_payload_maintenance(
        &mut self,
        encoded: &[EncodedPendingPayload],
    ) -> Result<(), String> {
        let inline_keys: Vec<&str> = encoded
            .iter()
            .filter(|(_, _, row)| decode_packed_locator(row).is_none())
            .map(|(key, _, _)| key.as_str())
            .collect();
        for chunk in inline_keys.chunks(PENDING_PAYLOAD_SQL_BATCH_ROWS) {
            let placeholders = (1..=chunk.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!("DELETE FROM pack_validation_pending WHERE key IN ({placeholders})");
            let params: Vec<SqliteValue> = chunk.iter().map(|key| super::sql_text(key)).collect();
            self.exec_params_ctx(&sql, &params, "clear buffered pack validation")?;
        }
        for (key, _, row) in encoded {
            if decode_packed_locator(row).is_some() {
                self.track_payload_open_maintenance(key, row)?;
            } else if key.starts_with("mem://") {
                self.exec_params_ctx(
                    "INSERT OR IGNORE INTO memory_backfill_pending (store_key) VALUES (?1)",
                    &[super::sql_text(key)],
                    "queue buffered memory backfill",
                )?;
            }
        }
        Ok(())
    }

    pub fn put_key(&mut self, key: &str, data: &[u8]) {
        if let Err(e) = self.try_put_key(key, data) {
            self.last_store_error = Some(e);
        }
    }

    /// Mint a content-ref for `data` and park the bytes under a named recovery key.
    pub fn put_payload_at_key(&mut self, key: &str, data: &[u8]) -> String {
        let content_ref = self.put_content_ref(data);
        self.put_key(key, data);
        content_ref
    }

    /// Mint a content-ref for `data`, store payload under `name` and the ref under `name/ref`.
    pub fn put_named_payload(&mut self, name: &str, data: &[u8]) -> String {
        let content_ref = self.put_payload_at_key(name, data);
        self.put_key(&format!("{name}/ref"), content_ref.as_bytes());
        content_ref
    }

    fn touch_payload(&mut self, key: &str) -> Result<(), String> {
        let tick = self.next_payload_tick()?;
        self.exec_params_ctx(
            super::SQL_INSERT_PAYLOAD_LRU,
            &[super::sql_text(key), super::sql_int(tick)],
            "store failed for payload_lru",
        )
    }

    fn next_payload_tick(&mut self) -> Result<i64, String> {
        let tick = super::meta_i64(&self.conn, "payload_tick")
            .map(|i| i + 1)
            .unwrap_or(1);
        self.put_meta_i64("payload_tick", tick)?;
        Ok(tick)
    }

    /// Enforce the persisted transient cap in two bounded SQL statements.
    /// Missing legacy LRU rows sort oldest, so restart repairs pre-LRU stores
    /// instead of retaining untracked sequence refs forever.
    pub(super) fn prune_transient_payloads(&mut self) -> Result<usize, String> {
        let count = super::query_i64(&self.conn, super::SQL_COUNT_TRANSIENT_PAYLOADS)
            .ok_or_else(|| "transient retention count failed".to_string())?
            as usize;
        let overflow = count.saturating_sub(MAX_TRANSIENT_PAYLOADS);
        if overflow > 0 {
            self.exec_params_ctx(
                super::SQL_DELETE_TRANSIENT_OVERFLOW,
                &[super::sql_int(overflow as i64)],
                "transient retention sweep",
            )?;
        }
        self.exec_params_ctx(
            super::SQL_DELETE_ORPHAN_TRANSIENT_LRU,
            &[],
            "transient retention lru sweep",
        )?;
        Ok(overflow)
    }

    pub fn payload(&self, key: &str) -> Option<Vec<u8>> {
        self.get_payload(key)
    }

    fn note_payload_hit(&self, nbytes: usize) {
        self.cache_hits.set(self.cache_hits.get().saturating_add(1));
        self.bytes_materialized
            .set(self.bytes_materialized.get().saturating_add(nbytes as u64));
    }

    pub fn get_payload(&self, key: &str) -> Option<Vec<u8>> {
        if let Some(value) = self.pending_payloads.as_ref().and_then(|p| p.get(key)) {
            self.note_payload_hit(value.len());
            return Some(value.to_vec());
        }
        if let Ok(rows) = self.conn.query_with_params(
            super::SQL_SELECT_PAYLOAD_VALUE_BY_KEY,
            &[super::sql_text(key)],
        ) {
            if let Some(row) = rows.first() {
                if let Some(SqliteValue::Blob(b)) = row.get(0) {
                    let Some(bytes) = self.decode_payload_row(key, b) else {
                        // A committed locator whose pack bytes are gone is a torn/truncated
                        // pack tail: report it, then treat as a miss so outer tiers can recover.
                        if let Some(loc) = decode_packed_locator(b) {
                            let pack_len = self.pack.as_ref().map(|p| p.len).unwrap_or(0);
                            self.note_integrity(format!("torn_pack: {key} locator {loc:?} unreadable (pack len {pack_len}); everything before the tear stays readable"));
                        }
                        self.cache_misses
                            .set(self.cache_misses.get().saturating_add(1));
                        return None;
                    };
                    self.note_payload_hit(bytes.len());
                    return Some(bytes);
                }
            }
        }
        self.cache_misses
            .set(self.cache_misses.get().saturating_add(1));
        None
    }
}
