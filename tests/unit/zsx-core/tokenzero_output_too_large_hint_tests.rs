    use super::{MAX_OUTPUT_BYTES, output_too_large_range_hint};
    use serde_json::json;
    use zero_abi::CallRequest;
    use zero_abi::WorkerTrace;

    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn request(op: &str, args: serde_json::Value) -> CallRequest {
        CallRequest {
            request_id: "expand-hint".into(),
            op: op.into(),
            args,
            deadline_unix_ms: None,
            trace: WorkerTrace {
                runtime_id: String::new(),
                cell_id: String::new(),
                request_id: String::new(),
                trace_id: String::new(),
                parent_span_id: None,
                worker_revision: String::new(),
                contract_digest: String::new(),
            },
            approval_grant: None,
            telemetry_request: None,
        }
    }

    #[test]
    fn hinted_range_accounts_for_envelope_overhead() {
        let reference = format!("tz://blob/{HASH}");
        let visible = "x".repeat(32_768);
        let value = json!({ "visible": visible, "status": "ok" });
        let output_bytes = MAX_OUTPUT_BYTES + 12_000;
        let hint = output_too_large_range_hint(
            &request("expand", json!({ "ref": reference })),
            &value,
            output_bytes,
        );
        assert!(
            hint.contains(&format!("retry with tz://blob/{HASH}#B0-")),
            "{hint}"
        );
        let end: u64 = hint
            .split("#B0-")
            .nth(1)
            .and_then(|tail| tail.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .expect("end");
        assert!(end < 32_768, "hint end {end} must be smaller than the failed 32768 slice");
        assert!(end >= 1024, "{end}");
    }

    #[test]
    fn existing_fragment_still_gets_a_smaller_hint() {
        let reference = format!("tz://blob/{HASH}#B0-32768");
        let visible = "x".repeat(32_768);
        let value = json!({ "visible": visible });
        let hint = output_too_large_range_hint(
            &request("expand", json!({ "ref": reference })),
            &value,
            MAX_OUTPUT_BYTES + 11_557,
        );
        assert!(
            hint.contains(&format!("tz://blob/{HASH}#B0-")),
            "{hint}"
        );
        assert!(!hint.contains("#B0-32768"), "{hint}");
    }

