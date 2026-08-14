//! Process-wide environment mutation lock and RAII scoped env mutation.
//!
//! Rust's `std::env::{set_var,remove_var}` is not thread-safe (and is
//! `unsafe` in edition 2024). Tests that mutate process env must hold
//! [`lock_env`] for the entire mutate+restore window so they do not race
//! other env-mutating tests across crates in one process.
//!
//! Prefer [`ScopedEnvVars`] over per-file `EnvGuard` structs: a local guard
//! cannot prove it holds the same lock identity used by every other
//! env-mutating test, and a panic between a raw mutate and its manual
//! restore leaks the mutation. A `ScopedEnvVars` acquires the process-wide
//! lock before reading/mutating and keeps it until after `Drop`
//! restoration, so restore is panic-safe. All keys captured by one guard
//! are restored in reverse mutation order.
//!
//! The lock is not reentrant: do not call [`lock_env`] and then create a
//! [`ScopedEnvVars`] in the same scope, and do not nest guards.
//!
//! Ported verbatim (module layout merged) from GraphZero
//! `graphzero-test-support` so all three engines share one lock helper.

// The whole point of this module is to fence `std::env` mutation behind one
// shared lock; edition 2024 makes those std calls `unsafe`.
#![allow(unsafe_code)]

use std::env;
use std::ffi::{OsStr, OsString};
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the process-wide env lock. Poisoned locks are recovered so one
/// failing test does not permanently block later env-mutating tests.
pub fn lock_env() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Holds the process-wide env lock for its whole lifetime and restores every
/// captured key on `Drop` (panic-safe).
pub struct ScopedEnvVars {
    _lock: MutexGuard<'static, ()>,
    entries: Vec<(&'static str, Option<OsString>)>,
}

impl ScopedEnvVars {
    /// Acquire the shared env lock without mutating. Use the builder methods
    /// to mutate one or more keys; `Drop` restores them while still holding
    /// the lock.
    pub fn new() -> Self {
        Self {
            _lock: lock_env(),
            entries: Vec::new(),
        }
    }

    /// Save `key`, set `key=value`, and return `self` (builder).
    pub fn set(&mut self, key: &'static str, value: impl AsRef<OsStr>) -> &mut Self {
        self.entries.push((key, env::var_os(key)));
        // SAFETY: the shared env lock is held for this guard's whole lifetime,
        // so no other env-mutating test/thread races this write.
        unsafe { env::set_var(key, value) };
        self
    }

    /// Save `key`, remove it, and return `self` (builder).
    pub fn remove(&mut self, key: &'static str) -> &mut Self {
        self.entries.push((key, env::var_os(key)));
        // SAFETY: the shared env lock is held for this guard's whole lifetime.
        unsafe { env::remove_var(key) };
        self
    }

    /// One-key convenience: set `key=value` under the shared lock; `Drop`
    /// restores the prior value (or removes the key) while still holding it.
    pub fn set_one(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        let mut guard = Self::new();
        guard.set(key, value);
        guard
    }

    /// One-key convenience: remove `key` under the shared lock; `Drop`
    /// restores the prior value while still holding it.
    pub fn remove_one(key: &'static str) -> Self {
        let mut guard = Self::new();
        guard.remove(key);
        guard
    }
}

impl Default for ScopedEnvVars {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ScopedEnvVars {
    fn drop(&mut self) {
        let entries = std::mem::take(&mut self.entries);
        for (key, prev) in entries.into_iter().rev() {
            match prev {
                Some(value) => {
                    // SAFETY: the shared env lock (field `_lock`) is still held
                    // during Drop, so restore cannot race other env mutators.
                    unsafe { env::set_var(key, value) };
                }
                None => {
                    // SAFETY: see above -- lock held through restoration.
                    unsafe { env::remove_var(key) };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ScopedEnvVars;

    const TEST_KEY: &str = "ZERO_TESTKIT_SCOPED_ENV_VARS_TEST";

    #[test]
    fn restores_original_value_after_repeated_set_and_remove() {
        let original = std::env::var_os(TEST_KEY);
        {
            let mut guard = ScopedEnvVars::new();
            guard.set(TEST_KEY, "first");
            guard.set(TEST_KEY, "second");
            guard.remove(TEST_KEY);
            assert_eq!(std::env::var_os(TEST_KEY), None);
            guard.set(TEST_KEY, "final");
            assert_eq!(
                std::env::var_os(TEST_KEY).as_deref(),
                Some(std::ffi::OsStr::new("final"))
            );
        }
        assert_eq!(std::env::var_os(TEST_KEY), original);
    }

    #[test]
    fn set_one_restores_prior_absence() {
        const KEY: &str = "ZERO_TESTKIT_SET_ONE_TEST";
        assert_eq!(std::env::var_os(KEY), None, "test key must start unset");
        {
            let _guard = ScopedEnvVars::set_one(KEY, "temp");
            assert_eq!(
                std::env::var_os(KEY).as_deref(),
                Some(std::ffi::OsStr::new("temp"))
            );
        }
        assert_eq!(std::env::var_os(KEY), None);
    }
}
