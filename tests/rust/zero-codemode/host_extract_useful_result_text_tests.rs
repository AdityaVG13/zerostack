    use super::extract_useful_result_text;
    use serde_json::json;

    #[test]
    fn prefers_payload_utf8_over_envelope_metadata() {
        let value = json!({
            "ack": "S1",
            "content": {
                "kind": "inline",
                "value": {
                    "metadata": {
                        "trace": { "request_id": "long-trace-id" },
                        "ownership": { "engine": "fszero" }
                    },
                    "value": {
                        "ok": true,
                        "operation": "fs.search",
                        "value": {
                            "payload_utf8": "HIT crates/zero-codemode/src/host.rs#L131\nHIT crates/zsx-core/src/session.rs#L83",
                            "detail": "search:2 hits"
                        }
                    }
                }
            }
        });
        let useful = extract_useful_result_text(&value).expect("payload_utf8");
        assert!(useful.starts_with("HIT "), "{useful}");
        assert!(!useful.contains("request_id"), "{useful}");
    }

    #[test]
    fn falls_back_to_hit_detail_when_payload_missing() {
        let value = json!({
            "value": { "detail": "search:1 hits\nHIT src/lib.rs#L1 kind=literal" }
        });
        let useful = extract_useful_result_text(&value).expect("detail");
        assert!(useful.contains("HIT src/lib.rs"), "{useful}");
    }

