//! MCP cancel settlement: late Ok is commit_race; late domain Err stays.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use zero_mcp::{
    McpCallContext, McpDispatchError, McpTransportConfig, execute_call, execute_call_with_cancel,
};

#[test]
fn late_ok_after_cancel_is_commit_race_and_keeps_payload() {
    let started = Arc::new(AtomicBool::new(false));
    let started_flag = Arc::clone(&started);
    let dispatcher = Arc::new(move |_tool: &str, _args: Value, _ctx: &McpCallContext| {
        started_flag.store(true, Ordering::Release);
        thread::sleep(Duration::from_millis(40));
        Ok(serde_json::json!({"committed": "late-ok"}))
    });
    let err = execute_call_with_cancel(
        dispatcher,
        "zero_execute",
        serde_json::json!({}),
        McpTransportConfig {
            tool_timeout: Duration::ZERO,
            max_inflight: 1,
        },
        || started.load(Ordering::Acquire),
    )
    .expect_err("late Ok after cancel must be commit_race");
    assert_eq!(err.kind, "commit_race");
    assert!(!err.retryable, "commit_race is not retryable");
    let data = err.data.expect("committed payload stays attached");
    assert_eq!(data["result"]["committed"], "late-ok");

    let second = execute_call(
        Arc::new(|_tool: &str, _args: Value, _ctx: &McpCallContext| {
            Ok(serde_json::json!({"ok": true}))
        }),
        "zero_execute",
        serde_json::json!({}),
        McpTransportConfig {
            tool_timeout: Duration::ZERO,
            max_inflight: 1,
        },
    )
    .expect("inflight permit released after commit_race");
    assert_eq!(second["ok"], true);
}

#[test]
fn late_domain_err_after_cancel_stays_that_err() {
    let started = Arc::new(AtomicBool::new(false));
    let started_flag = Arc::clone(&started);
    let dispatcher = Arc::new(move |_tool: &str, _args: Value, _ctx: &McpCallContext| {
        started_flag.store(true, Ordering::Release);
        thread::sleep(Duration::from_millis(40));
        Err(McpDispatchError::new("policy_denied", "domain refused", false))
    });
    let err = execute_call_with_cancel(
        dispatcher,
        "zero_execute",
        serde_json::json!({}),
        McpTransportConfig {
            tool_timeout: Duration::ZERO,
            max_inflight: 1,
        },
        || started.load(Ordering::Acquire),
    )
    .expect_err("late domain Err must stay that Err");
    assert_eq!(err.kind, "policy_denied");
    assert!(!err.retryable);
    assert_eq!(err.message, "domain refused");
}
