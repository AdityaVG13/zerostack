    use super::{normalize_public_result, unwrap_worker_envelope};
    use serde_json::{Value, json};

    #[test]
    fn unwraps_worker_envelope_to_domain_payload() {
        let wrapped = json!({
            "metadata": {
                "ownership": { "engine": "fszero", "refs": ["fz://blob/aa"] }
            },
            "value": { "operation": "fs.search", "ok": true }
        });
        let domain = unwrap_worker_envelope(wrapped);
        assert_eq!(domain["operation"], "fs.search");
        assert_eq!(domain["refs"], json!(["fz://blob/aa"]));
    }

    #[test]
    fn normalize_puts_domain_in_content_value() {
        let encoded = serde_json::to_string(&json!({
            "metadata": { "ownership": { "engine": "fszero", "refs": [] } },
            "value": { "operation": "fs.search", "ok": true }
        }))
        .unwrap();
        let public: Value = serde_json::from_str(&normalize_public_result(&encoded).unwrap()).unwrap();
        assert_eq!(public["content"]["kind"], "inline");
        assert_eq!(public["content"]["value"]["operation"], "fs.search");
        assert!(public["content"]["value"].get("metadata").is_none());
    }

    #[test]
    fn search_payload_exposes_snap_targets() {
        let wrapped = json!({
            "metadata": { "ownership": { "engine": "fszero", "refs": [] } },
            "value": {
                "operation": "fs.search",
                "payload_utf8": "HIT crates/zsx-core/src/lower.rs#L10-L14 kind=literal\n| 10: fn lower"
            }
        });
        let domain = unwrap_worker_envelope(wrapped);
        assert_eq!(domain["targets"][0]["path"], "crates/zsx-core/src/lower.rs");
        assert_eq!(domain["targets"][0]["start_line"], 10);
        assert_eq!(domain["targets"][0]["end_line"], 14);
    }

