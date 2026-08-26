//! MCP cancel settlement: late Ok is commit_race; late domain Err stays.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use serde_json::Value;
use zero_mcp::{
    McpCallContext, McpDispatchError, McpTransportConfig, ZERO_CARRIER_PLAN_BYTE_LIMIT,
    ZeroCarrierCapabilities, ZeroCarrierDispatcher, ZeroCarrierSampling,
    decode_zero_carrier_request, execute_call, execute_call_with_cancel, zero_carrier_catalog,
};

#[test]
fn late_ok_after_cancel_is_commit_race_and_keeps_payload() {
    let started = Arc::new(AtomicBool::new(false));
    let started_flag = Arc::clone(&started);
    let rendezvous = Arc::new(Barrier::new(2));
    let worker_rendezvous = Arc::clone(&rendezvous);
    let dispatcher = Arc::new(move |_tool: &str, _args: Value, _ctx: &McpCallContext| {
        started_flag.store(true, Ordering::Release);
        worker_rendezvous.wait();
        Ok(serde_json::json!({"committed": "late-ok"}))
    });
    let err = execute_call_with_cancel(
        dispatcher,
        "zero",
        serde_json::json!({}),
        McpTransportConfig {
            tool_timeout: Duration::ZERO,
            max_inflight: 1,
        },
        move || {
            if !started.load(Ordering::Acquire) {
                return false;
            }
            rendezvous.wait();
            true
        },
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
        "zero",
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
    let rendezvous = Arc::new(Barrier::new(2));
    let worker_rendezvous = Arc::clone(&rendezvous);
    let dispatcher = Arc::new(move |_tool: &str, _args: Value, _ctx: &McpCallContext| {
        started_flag.store(true, Ordering::Release);
        worker_rendezvous.wait();
        Err(McpDispatchError::new(
            "policy_denied",
            "domain refused",
            false,
        ))
    });
    let err = execute_call_with_cancel(
        dispatcher,
        "zero",
        serde_json::json!({}),
        McpTransportConfig {
            tool_timeout: Duration::ZERO,
            max_inflight: 1,
        },
        move || {
            if !started.load(Ordering::Acquire) {
                return false;
            }
            rendezvous.wait();
            true
        },
    )
    .expect_err("late domain Err must stay that Err");
    assert_eq!(err.kind, "policy_denied");
    assert!(!err.retryable);
    assert_eq!(err.message, "domain refused");
}

fn carrier_capabilities() -> ZeroCarrierCapabilities {
    ZeroCarrierCapabilities {
        cancellation: true,
        progress: true,
        sampling: ZeroCarrierSampling::Unavailable,
        maximum_inbound_bytes: 1024 * 1024,
        maximum_outbound_bytes: 1024 * 1024,
        native_package_digest: "a".repeat(64),
    }
}

#[test]
fn carrier_catalog_exposes_exactly_one_closed_zero_tool() {
    let catalog = zero_carrier_catalog();
    let tools = catalog.as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "zero");
    assert_eq!(tools[0]["inputSchema"]["additionalProperties"], false);
    assert_eq!(
        tools[0]["outputSchema"]["properties"]["protocol"]["const"],
        "ZeroKernel"
    );
    assert_eq!(tools[0]["outputSchema"]["additionalProperties"], false);
    let description = tools[0]["description"].as_str().unwrap();
    assert!(description.contains("z.read"));
    assert!(description.contains("No other z.* methods exist"));
    assert_eq!(
        tools[0]["inputSchema"]["required"],
        serde_json::json!(["plan"])
    );
    assert!(tools[0].to_string().find("projectRoot").is_none());
    assert!(tools[0].to_string().find("sessionId").is_none());
}

#[test]
fn carrier_request_is_closed_and_bounded() {
    let capabilities = carrier_capabilities();
    let request = decode_zero_carrier_request(
        serde_json::json!({"plan": "return await z.read('Cargo.toml');"}),
        &capabilities,
    )
    .unwrap();
    assert_eq!(request.plan, "return await z.read('Cargo.toml');");
    assert!(
        decode_zero_carrier_request(
            serde_json::json!({"plan": "return 1;", "root": "/private"}),
            &capabilities,
        )
        .is_err()
    );
    let oversized = "x".repeat(ZERO_CARRIER_PLAN_BYTE_LIMIT + 1);
    assert!(
        decode_zero_carrier_request(serde_json::json!({"plan": oversized}), &capabilities).is_err()
    );
}

#[test]
fn carrier_capabilities_require_finite_messages_and_package_digest() {
    let mut capabilities = carrier_capabilities();
    assert!(capabilities.validate().is_ok());
    capabilities.maximum_outbound_bytes = 0;
    assert!(capabilities.validate().is_err());
    capabilities = carrier_capabilities();
    capabilities.native_package_digest = "not-a-digest".into();
    assert!(capabilities.validate().is_err());
}

fn completed_response() -> zero_abi::ZeroKernelResponse {
    zero_abi::ZeroKernelResponse {
        protocol: zero_abi::ZERO_KERNEL_PROTOCOL.into(),
        outcome: zero_abi::ZeroKernelOutcome::Completed,
        value: Some(serde_json::json!({"ok": true})),
        error: None,
        operations: vec![zero_abi::ZeroOperationTrace {
            sequence: 1,
            method: "read".into(),
            status: zero_abi::ZeroOperationStatus::Completed,
            capsule_root: "a".repeat(64),
            occurrence: 1,
            parallel_group: None,
            target: Some("src/lib.rs".into()),
            detail: Some("11 bytes visible".into()),
            result_count: None,
            changed_files: None,
            duration_ns: 1,
        }],
        operations_truncated: false,
        handles: Vec::new(),
        event: zero_abi::ZeroHandle::from_digest(&"b".repeat(64)).unwrap(),
        state: zero_abi::StateEvidence {
            before: None,
            after: None,
            unchanged: true,
        },
        ledger: zero_abi::KernelLedger {
            wall_ns: 1,
            cpu_ns_upper_bound: 1,
            calls: 1,
            tasks: 0,
            bytes_read: 0,
            bytes_written: 0,
            bytes_visible: 11,
        },
        turn: None,
    }
}

#[test]
fn carrier_dispatcher_forwards_only_zero_and_preserves_response() {
    let executor = Arc::new(|plan: &str, _context: &McpCallContext| {
        assert_eq!(plan, "return 1;");
        Ok(completed_response())
    });
    let dispatcher =
        Arc::new(ZeroCarrierDispatcher::new(executor, carrier_capabilities()).unwrap());
    let result = execute_call(
        dispatcher.clone(),
        "zero",
        serde_json::json!({"plan": "return 1;"}),
        McpTransportConfig::default(),
    )
    .unwrap();
    assert_eq!(result["protocol"], "ZeroKernel");
    assert_eq!(result["value"]["ok"], true);
    assert_eq!(result["operations"][0]["method"], "read");
    assert_eq!(result["operations"][0]["target"], "src/lib.rs");

    let error = execute_call(
        dispatcher,
        "fs.read",
        serde_json::json!({"plan": "return 1;"}),
        McpTransportConfig::default(),
    )
    .unwrap_err();
    assert_eq!(error.kind, "unknown_tool");
}
