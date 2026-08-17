//! Process-global hot-path counters for the hub's real surface.
//!
//! Counters are `AtomicU64`. There is no `static mut`. Increment at the
//! measurement site (or later at the product site). Algebraically redundant
//! totals are derived at snapshot time, not stored.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

/// Relaxed is enough for plain counts. Ordering-dependent sites would use
/// acquire/release; none of the current rows need that.
const INC: Ordering = Ordering::Relaxed;
const LOAD: Ordering = Ordering::Relaxed;

struct HotPathAtoms {
    cas_write: AtomicU64,
    cas_read: AtomicU64,
    atomic_write_fsync: AtomicU64,
    atomic_write_rename: AtomicU64,
    journal_recover: AtomicU64,
    codemode_execute: AtomicU64,
    mcp_inflight: AtomicU64,
}

static HOT_PATH: HotPathAtoms = HotPathAtoms {
    cas_write: AtomicU64::new(0),
    cas_read: AtomicU64::new(0),
    atomic_write_fsync: AtomicU64::new(0),
    atomic_write_rename: AtomicU64::new(0),
    journal_recover: AtomicU64::new(0),
    codemode_execute: AtomicU64::new(0),
    mcp_inflight: AtomicU64::new(0),
};

/// Point-in-time copy of the hub hot-path counters.
///
/// Rows match the hub composition surface, not a SQL-class opcode table:
/// CAS write/read, `atomic_write` fsync/rename, journal recover, CodeMode
/// execute, MCP inflight.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct HotPathProfileSnapshot {
    pub cas_write: u64,
    pub cas_read: u64,
    pub atomic_write_fsync: u64,
    pub atomic_write_rename: u64,
    pub journal_recover: u64,
    pub codemode_execute: u64,
    pub mcp_inflight: u64,
    /// Derived: `cas_write + cas_read`. Not a stored counter.
    pub cas_ops_total: u64,
    /// Derived: `atomic_write_fsync + atomic_write_rename`.
    pub atomic_write_ops_total: u64,
}

impl HotPathProfileSnapshot {
    pub fn record_cas_write() {
        HOT_PATH.cas_write.fetch_add(1, INC);
    }

    pub fn record_cas_read() {
        HOT_PATH.cas_read.fetch_add(1, INC);
    }

    pub fn record_atomic_write_fsync() {
        HOT_PATH.atomic_write_fsync.fetch_add(1, INC);
    }

    pub fn record_atomic_write_rename() {
        HOT_PATH.atomic_write_rename.fetch_add(1, INC);
    }

    pub fn record_journal_recover() {
        HOT_PATH.journal_recover.fetch_add(1, INC);
    }

    pub fn record_codemode_execute() {
        HOT_PATH.codemode_execute.fetch_add(1, INC);
    }

    pub fn record_mcp_inflight() {
        HOT_PATH.mcp_inflight.fetch_add(1, INC);
    }

    pub fn record_mcp_complete() {
        HOT_PATH
            .mcp_inflight
            .fetch_update(INC, LOAD, |n| Some(n.saturating_sub(1)))
            .ok();
    }

    pub fn snapshot() -> Self {
        let cas_write = HOT_PATH.cas_write.load(LOAD);
        let cas_read = HOT_PATH.cas_read.load(LOAD);
        let atomic_write_fsync = HOT_PATH.atomic_write_fsync.load(LOAD);
        let atomic_write_rename = HOT_PATH.atomic_write_rename.load(LOAD);
        Self {
            cas_write,
            cas_read,
            atomic_write_fsync,
            atomic_write_rename,
            journal_recover: HOT_PATH.journal_recover.load(LOAD),
            codemode_execute: HOT_PATH.codemode_execute.load(LOAD),
            mcp_inflight: HOT_PATH.mcp_inflight.load(LOAD),
            cas_ops_total: cas_write.saturating_add(cas_read),
            atomic_write_ops_total: atomic_write_fsync.saturating_add(atomic_write_rename),
        }
    }

    pub fn reset_for_test() {
        HOT_PATH.cas_write.store(0, INC);
        HOT_PATH.cas_read.store(0, INC);
        HOT_PATH.atomic_write_fsync.store(0, INC);
        HOT_PATH.atomic_write_rename.store(0, INC);
        HOT_PATH.journal_recover.store(0, INC);
        HOT_PATH.codemode_execute.store(0, INC);
        HOT_PATH.mcp_inflight.store(0, INC);
    }

    pub fn diff(&self, later: &Self) -> Self {
        Self {
            cas_write: later.cas_write.saturating_sub(self.cas_write),
            cas_read: later.cas_read.saturating_sub(self.cas_read),
            atomic_write_fsync: later
                .atomic_write_fsync
                .saturating_sub(self.atomic_write_fsync),
            atomic_write_rename: later
                .atomic_write_rename
                .saturating_sub(self.atomic_write_rename),
            journal_recover: later.journal_recover.saturating_sub(self.journal_recover),
            codemode_execute: later.codemode_execute.saturating_sub(self.codemode_execute),
            mcp_inflight: later.mcp_inflight.saturating_sub(self.mcp_inflight),
            cas_ops_total: later.cas_ops_total.saturating_sub(self.cas_ops_total),
            atomic_write_ops_total: later
                .atomic_write_ops_total
                .saturating_sub(self.atomic_write_ops_total),
        }
    }
}

impl fmt::Display for HotPathProfileSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Sorted keys so two snapshots of the same counts diff cleanly.
        let rows = [
            ("atomic_write_fsync", self.atomic_write_fsync),
            ("atomic_write_ops_total", self.atomic_write_ops_total),
            ("atomic_write_rename", self.atomic_write_rename),
            ("cas_ops_total", self.cas_ops_total),
            ("cas_read", self.cas_read),
            ("cas_write", self.cas_write),
            ("codemode_execute", self.codemode_execute),
            ("journal_recover", self.journal_recover),
            ("mcp_inflight", self.mcp_inflight),
        ];
        for (key, value) in rows {
            writeln!(f, "{key}={value}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::HotPathProfileSnapshot;

    #[test]
    fn snapshot_serializes() {
        HotPathProfileSnapshot::reset_for_test();
        HotPathProfileSnapshot::record_cas_write();
        HotPathProfileSnapshot::record_cas_read();
        HotPathProfileSnapshot::record_atomic_write_fsync();
        HotPathProfileSnapshot::record_atomic_write_rename();
        HotPathProfileSnapshot::record_journal_recover();
        HotPathProfileSnapshot::record_codemode_execute();
        HotPathProfileSnapshot::record_mcp_inflight();
        let snap = HotPathProfileSnapshot::snapshot();
        let value = serde_json::to_value(&snap).expect("serialize snapshot");
        assert_eq!(value["cas_write"], 1);
        assert_eq!(value["cas_read"], 1);
        assert_eq!(value["cas_ops_total"], 2);
        assert_eq!(value["atomic_write_fsync"], 1);
        assert_eq!(value["atomic_write_rename"], 1);
        assert_eq!(value["atomic_write_ops_total"], 2);
        assert_eq!(value["journal_recover"], 1);
        assert_eq!(value["codemode_execute"], 1);
        assert_eq!(value["mcp_inflight"], 1);
        let text = serde_json::to_string(&snap).expect("string");
        assert!(text.contains("\"cas_write\":1"), "{text}");
        let rendered = snap.to_string();
        let keys: Vec<&str> = rendered
            .lines()
            .map(|line| line.split('=').next().unwrap())
            .collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted, "Display keys must be sorted");
    }
}
