//! Shared CodeMode wall-limit environment contract.

use std::ffi::OsString;
use std::time::Duration;
use zero_codemode::{CODEMODE_WALL_MS_ENVS, HostLimits, MAX_WALL_MS, effective_max_wall_ms};

struct RestoreEnv(Vec<(&'static str, Option<OsString>)>);

impl RestoreEnv {
    fn clear() -> Self {
        let saved = CODEMODE_WALL_MS_ENVS
            .iter()
            .map(|key| (*key, std::env::var_os(key)))
            .collect();
        for key in CODEMODE_WALL_MS_ENVS {
            // SAFETY: this test target has one test and is run with one test thread.
            unsafe { std::env::remove_var(key) };
        }
        Self(saved)
    }
}

impl Drop for RestoreEnv {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..) {
            // SAFETY: this test target has one test and is run with one test thread.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

#[test]
fn named_environment_overrides_share_priority_and_floor() {
    let _restore = RestoreEnv::clear();
    assert_eq!(effective_max_wall_ms(), MAX_WALL_MS);
    assert_eq!(HostLimits::default().wall_timeout, Duration::from_millis(MAX_WALL_MS));

    // SAFETY: this test target has one test and is run with one test thread.
    unsafe { std::env::set_var("GRAPHZERO_CODEMODE_MAX_WALL_MS", "432") };
    assert_eq!(effective_max_wall_ms(), 432);
    // SAFETY: this test target has one test and is run with one test thread.
    unsafe { std::env::remove_var("GRAPHZERO_CODEMODE_MAX_WALL_MS") };

    // SAFETY: this test target has one test and is run with one test thread.
    unsafe { std::env::set_var("TOKENZERO_CODEMODE_WALL_MS", "0") };
    assert_eq!(effective_max_wall_ms(), 1);

    // SAFETY: this test target has one test and is run with one test thread.
    unsafe { std::env::set_var("ZEROSTACK_CODEMODE_WALL_MS", "345") };
    assert_eq!(effective_max_wall_ms(), 345);

    // SAFETY: this test target has one test and is run with one test thread.
    unsafe { std::env::set_var("FSZERO_CODEMODE_WALL_MS", "789") };
    assert_eq!(effective_max_wall_ms(), 789);
    assert_eq!(HostLimits::default().wall_timeout, Duration::from_millis(789));
}
