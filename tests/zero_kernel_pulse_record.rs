//! Hub composition records Pulse from TokenEngine measure/project.

use std::fs;

use tempfile::tempdir;
use zero_abi::KernelBudget;
use zero_kernel::ZeroKernel;
use zero_pulse::default_ledger_path;

fn budget() -> KernelBudget {
    KernelBudget {
        wall_ms: 30_000,
        cpu_ms: 30_000,
        memory_bytes: 64 * 1024 * 1024,
        call_limit: 32,
        task_limit: 4,
        output_byte_limit: 32 * 1024,
    }
}

#[test]
fn canonical_read_records_hub_pulse() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("note.txt"), b"pulse-kernel-record").unwrap();
    let store = root.path().join(".zerostack");
    let kernel = ZeroKernel::canonical(root.path(), &store, "pulse-record", budget()).unwrap();
    let response = kernel
        .execute_cell(r#"return await z.read("note.txt");"#)
        .expect("cell");
    assert_eq!(
        response.outcome,
        zero_abi::ZeroKernelOutcome::Completed,
        "error={:?}",
        response.error
    );
    let pulse_path = default_ledger_path(root.path());
    let text = fs::read_to_string(&pulse_path).unwrap_or_else(|err| {
        panic!(
            "kernel composition must persist Pulse at {}: {err}",
            pulse_path.display()
        )
    });
    assert!(
        text.contains("tool_call") && (text.contains("measure") || text.contains("project")),
        "Pulse ledger missing kernel tool_call: {text}"
    );
    assert!(
        text.contains("estimator:") || text.contains("tiktoken:") || text.contains('@'),
        "Pulse row must carry a labeled tokenizer id: {text}"
    );
}

#[test]
fn canonical_uses_explicit_store_for_hub_pulse() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("note.txt"), b"pulse-explicit-store").unwrap();
    let store = root.path().join("external-state");
    let kernel =
        ZeroKernel::canonical(root.path(), &store, "pulse-explicit-store", budget()).unwrap();
    let response = kernel
        .execute_cell(r#"return await z.read("note.txt");"#)
        .expect("cell");
    assert_eq!(
        response.outcome,
        zero_abi::ZeroKernelOutcome::Completed,
        "error={:?}",
        response.error
    );

    let pulse_path = store.join("tokenzero/pulse/events.jsonl");
    assert!(
        pulse_path.is_file(),
        "explicit state root must own the Pulse ledger at {}",
        pulse_path.display()
    );
    assert!(
        !default_ledger_path(root.path()).exists(),
        "Pulse must not leak into the project-local default when state root is explicit"
    );
}

#[test]
fn canonical_pulse_carries_kernel_attribution() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("note.txt"), b"pulse-attribution").unwrap();
    let store = root.path().join(".zerostack");
    let kernel = ZeroKernel::canonical(root.path(), &store, "pulse-attribution", budget()).unwrap();
    let response = kernel
        .execute_cell(r#"return await z.read("note.txt");"#)
        .expect("cell");
    assert_eq!(
        response.outcome,
        zero_abi::ZeroKernelOutcome::Completed,
        "error={:?}",
        response.error
    );

    let text = fs::read_to_string(default_ledger_path(root.path())).unwrap();
    let events: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).expect("Pulse event JSON"))
        .collect();
    assert!(
        events.iter().any(|event| {
            event.get("event").and_then(|value| value.as_str()) == Some("tool_call")
                && event.get("session_id").and_then(|value| value.as_str())
                    == Some("pulse-attribution")
                && event
                    .get("call_id")
                    .and_then(|value| value.as_str())
                    .is_some_and(|value| !value.is_empty())
        }),
        "kernel Pulse events must carry session_id and call_id: {text}"
    );
}

#[test]
fn canonical_read_handle_round_trip_is_z_blob() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("note.txt"), b"identity-handle-bytes").unwrap();
    let store = root.path().join(".zerostack");
    let kernel = ZeroKernel::canonical(root.path(), &store, "identity-handle", budget()).unwrap();
    let first = kernel
        .execute_cell(r#"return await z.read("note.txt");"#)
        .expect("cell");
    assert_eq!(
        first.outcome,
        zero_abi::ZeroKernelOutcome::Completed,
        "error={:?}",
        first.error
    );
    let handle = first
        .handles
        .iter()
        .find(|h| h.as_str().starts_with("z://blob/"))
        .map(|h| h.as_str().to_string())
        .unwrap_or_else(|| first.event.as_str().to_string());
    assert!(
        handle.starts_with("z://blob/"),
        "live identity is z://blob, got {handle}"
    );
    assert!(
        !handle.starts_with("fz://")
            && !handle.starts_with("tz://")
            && !handle.starts_with("gz://"),
        "product schemes must not appear on kernel handles: {handle}"
    );
    let cell = format!("return await z.read({handle:?});");
    let second = kernel.execute_cell(&cell).expect("handle read");
    assert_eq!(
        second.outcome,
        zero_abi::ZeroKernelOutcome::Completed,
        "handle read error={:?}",
        second.error
    );
}

#[test]
fn retired_product_scheme_read_fails_closed() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("note.txt"), b"x").unwrap();
    let store = root.path().join(".zerostack");
    let kernel = ZeroKernel::canonical(root.path(), &store, "fail-close", budget()).unwrap();
    let response = kernel
        .execute_cell(
            r#"return await z.read("tz://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");"#,
        )
        .expect("cell must return, not panic");
    assert_ne!(
        response.outcome,
        zero_abi::ZeroKernelOutcome::Completed,
        "retired tz:// must not complete as a live handle"
    );
    let err = response
        .error
        .expect("retired scheme must surface an error");
    assert_eq!(
        err.kind,
        zero_abi::EngineErrorKind::InvalidInput,
        "retired scheme must fail as invalid_input before path lookup, got {err:?}"
    );
    assert!(
        err.detail.contains("retired product scheme"),
        "detail must name the retired scheme, got {}",
        err.detail
    );
    assert!(
        !err.detail.contains("os error") && !err.detail.to_ascii_lowercase().contains("not found"),
        "must not fall through to filesystem NotFound: {}",
        err.detail
    );
    for scheme in [
        "fz://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "gz://blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ] {
        let cell = format!("return await z.read({scheme:?});");
        let response = kernel.execute_cell(&cell).expect("cell must return");
        let err = response
            .error
            .expect("retired scheme must surface an error");
        assert_eq!(
            err.kind,
            zero_abi::EngineErrorKind::InvalidInput,
            "{scheme} {err:?}"
        );
        assert!(!err.detail.contains("os error"), "{scheme} {}", err.detail);
    }
}

#[test]
fn empty_retired_product_schemes_fail_closed() {
    let root = tempdir().unwrap();
    let store = root.path().join(".zerostack");
    let kernel = ZeroKernel::canonical(root.path(), &store, "empty-retired", budget()).unwrap();

    for scheme in ["fz://", "gz://", "tz://"] {
        let cell = format!("return await z.read({scheme:?});");
        let response = kernel.execute_cell(&cell).expect("cell must return");
        assert_ne!(
            response.outcome,
            zero_abi::ZeroKernelOutcome::Completed,
            "{scheme} must fail closed"
        );
        let err = response.error.expect("retired scheme error");
        assert_eq!(
            err.kind,
            zero_abi::EngineErrorKind::InvalidInput,
            "{scheme} {err:?}"
        );
        assert!(
            err.detail.contains("retired product scheme"),
            "{scheme} must fail at the retired-scheme guard: {}",
            err.detail
        );
        assert!(
            !err.detail.contains("os error")
                && !err.detail.to_ascii_lowercase().contains("not found"),
            "{scheme} must not reach path lookup: {}",
            err.detail
        );
    }
}
