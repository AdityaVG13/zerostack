//! Process-wide environment mutation lock for tests.

#![allow(unsafe_code)]

use std::env;
use std::ffi::{OsStr, OsString};
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

pub fn lock_env() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct ScopedEnvVars {
    _lock: MutexGuard<'static, ()>,
    entries: Vec<(&'static str, Option<OsString>)>,
}

impl ScopedEnvVars {
    pub fn new() -> Self {
        Self {
            _lock: lock_env(),
            entries: Vec::new(),
        }
    }

    pub fn set(&mut self, key: &'static str, value: impl AsRef<OsStr>) -> &mut Self {
        self.entries.push((key, env::var_os(key)));
        // SAFETY: every test environment mutation uses ENV_LOCK and the guard
        // keeps it held until all prior values have been restored.
        unsafe { env::set_var(key, value) };
        self
    }

    pub fn remove(&mut self, key: &'static str) -> &mut Self {
        self.entries.push((key, env::var_os(key)));
        // SAFETY: every test environment mutation uses ENV_LOCK.
        unsafe { env::remove_var(key) };
        self
    }

    pub fn set_one(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        let mut guard = Self::new();
        guard.set(key, value);
        guard
    }

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
        for (key, previous) in std::mem::take(&mut self.entries).into_iter().rev() {
            match previous {
                Some(value) => {
                    // SAFETY: ENV_LOCK is still held by _lock during Drop.
                    unsafe { env::set_var(key, value) };
                }
                None => {
                    // SAFETY: ENV_LOCK is still held by _lock during Drop.
                    unsafe { env::remove_var(key) };
                }
            }
        }
    }
}
