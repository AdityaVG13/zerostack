//! Pattern 65 subprocess abort injection.
//!
//! Armed only when `TOKENZERO_ARM_CRASH_BOUNDARY` equals a named window.
//! Production processes leave the env unset; `maybe_crash` is then a no-op.
//! Abort is `std::process::abort` (no Drop, no extra flush) so the child
//! looks like a power cut.

pub const ARM_ENV: &str = "TOKENZERO_ARM_CRASH_BOUNDARY";

pub const BEFORE_PERSIST_UNREADABLE: &str = "BeforePersistOnUnreadableSnapshot";
pub const BEFORE_PRUNE_UNREADABLE: &str = "BeforePruneOnUnreadableSnapshot";
pub const AFTER_JOURNAL_APPEND: &str = "AfterJournalAppendBeforeSnapshotRewrite";
pub const AFTER_WAL_APPEND: &str = "AfterWalAppendSession";
pub const AFTER_TMP_BEFORE_RENAME: &str = "AfterTmpWriteBeforeRename";

/// Abort this process if `boundary` is the armed window.
#[inline]
pub fn maybe_crash(boundary: &str) {
    match std::env::var(ARM_ENV) {
        Ok(armed) if armed == boundary => std::process::abort(),
        _ => {}
    }
}
