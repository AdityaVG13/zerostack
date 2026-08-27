//! Append-only reservation audit ledger (delta WAL).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use graphzero_store::ContentHash;
use graphzero_store::store::delta_log::{DeltaEntry, DeltaLog, entry_type};
use graphzero_store::store::lock::WriterLock;
use serde_json;

use crate::schema::{IntentReservation, ReservationStatus};

pub struct ReservationLedger {
    store_root: PathBuf,
    records: Vec<IntentReservation>,
    /// Last-write-wins tip by reservation_id (avoids full WAL rescan per check).
    tip: std::collections::BTreeMap<String, IntentReservation>,
    log: Option<DeltaLog>,
    writer_lock: Option<WriterLock>,
}

impl ReservationLedger {
    pub fn open(store_root: &Path) -> Result<Self> {
        Self::open_with_writer(store_root, None)
    }

    /// Open after acquiring the store-wide WAL writer domain.
    ///
    /// ReserveService uses this before replaying records so its conflict check
    /// and append are one cross-process read-modify-write transaction.
    pub fn open_for_write(store_root: &Path) -> Result<Self> {
        let writer_lock = WriterLock::acquire(store_root)?;
        Self::open_with_writer(store_root, Some(writer_lock))
    }

    fn open_with_writer(store_root: &Path, writer_lock: Option<WriterLock>) -> Result<Self> {
        let mut records = Vec::new();
        let wal = store_root.join("wal");
        if wal.is_dir() {
            for (seg, entries) in graphzero_store::store::delta_log::read_all_segments(&wal)? {
                for e in entries {
                    if e.entry_type != entry_type::RESERVATION {
                        continue;
                    }
                    let rec = serde_json::from_slice::<IntentReservation>(&e.payload)
                        .with_context(|| {
                            format!("decode reservation ledger entry in segment {seg}")
                        })?;
                    records.push(rec);
                }
            }
        }
        let mut tip = std::collections::BTreeMap::new();
        for rec in &records {
            tip.insert(rec.reservation_id.clone(), rec.clone());
        }
        Ok(Self {
            store_root: store_root.to_path_buf(),
            records,
            tip,
            log: None,
            writer_lock,
        })
    }

    pub fn records(&self) -> &[IntentReservation] {
        &self.records
    }

    pub fn append(&mut self, record: IntentReservation) -> Result<()> {
        if self.writer_lock.is_none() {
            self.writer_lock = Some(WriterLock::acquire(&self.store_root)?);
        }
        let payload = serde_json::to_vec(&record)?;
        let hash = ContentHash::of(&payload);
        if self.log.is_none() {
            self.log = Some(DeltaLog::open(&self.store_root)?);
        }
        let log = self
            .log
            .as_mut()
            .context("reservation ledger log should be initialized")?;
        log.append(DeltaEntry {
            entry_type: entry_type::RESERVATION,
            blob_hash: hash.0,
            payload,
        })?;
        log.commit()?;
        self.tip
            .insert(record.reservation_id.clone(), record.clone());
        self.records.push(record);
        Ok(())
    }

    pub fn materialized(&self, now: u64) -> Vec<IntentReservation> {
        // Tip already collapses WAL history to last-write-wins; only apply expiry.
        self.tip
            .values()
            .map(|rec| {
                let mut r = rec.clone();
                if matches!(
                    r.status,
                    ReservationStatus::Active | ReservationStatus::Declared
                ) && r.expires_at <= now
                {
                    r.status = ReservationStatus::Expired;
                }
                r
            })
            .collect()
    }

    pub fn active(&self, repo_id: &str, now: u64) -> Vec<IntentReservation> {
        self.materialized(now)
            .into_iter()
            .filter(|r| r.repo_id == repo_id && r.status == ReservationStatus::Active)
            .collect()
    }

    /// Declared + active reservations that should block other agents (P5.2).
    pub fn blocking_for_conflict(&self, repo_id: &str, now: u64) -> Vec<IntentReservation> {
        self.materialized(now)
            .into_iter()
            .filter(|r| {
                r.repo_id == repo_id
                    && matches!(
                        r.status,
                        ReservationStatus::Active | ReservationStatus::Declared
                    )
            })
            .collect()
    }
}

pub fn replay_ledger(store_root: &Path) -> Result<Vec<IntentReservation>> {
    Ok(ReservationLedger::open(store_root)?.records().to_vec())
}

pub fn ledger_state_hash<T: serde::Serialize + ?Sized>(records: &T) -> Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(records)?;
    let mut h = Sha256::new();
    h.update(bytes);
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
#[path = "../../../../tests/graphzero/unit/graphzero-reserve/ledger_tests.rs"]
mod tests;
