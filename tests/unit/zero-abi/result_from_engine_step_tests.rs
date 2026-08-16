    use super::*;
    use serde_json::json;

    #[test]
    fn inline_string_payload_uses_tagged_inline() {
        let result =
            zero_result_from_engine_step("R2", true, "fs.read", "read", &json!("bytes"), None);
        assert_eq!(result.kind(), "inline");
        assert_eq!(result.inline_value().unwrap(), &json!("bytes"));
        assert_eq!(
            result.reference_value(),
            Err(ZeroResultAccessError::ExpectedRef { actual: "inline" })
        );
        assert_eq!(
            zero_result_to_wire(&result),
            json!({"ack":"R2","content":{"kind":"inline","value":"bytes"}})
        );
    }

    #[test]
    fn canonical_fz_ref_uses_tagged_ref() {
        const HASH: &str =
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let reference = format!("fz://blob/{HASH}");
        let wire = json!({"ref": reference, "preview": "head"});
        let result = zero_result_from_engine_step(
            "R2",
            true,
            "fs.read",
            &reference,
            &wire,
            Some("read"),
        );
        assert_eq!(result.kind(), "ref");
        assert_eq!(result.reference_value().unwrap(), reference.as_str());
        assert_eq!(result.preview().unwrap(), Some("head"));
    }

    #[test]
    fn noncanonical_alias_stays_inline_not_ref() {
        let result = zero_result_from_engine_step(
            "L",
            true,
            "fs.ls",
            "ls_manifest",
            &json!({"entries":[]}),
            None,
        );
        assert_eq!(result.kind(), "inline");
        assert!(result.reference_value().is_err());
    }

    #[test]
    fn failure_is_inline_x0() {
        let result = zero_result_from_engine_step(
            "X0",
            false,
            "fs.read",
            "error",
            &Value::Null,
            Some("boom"),
        );
        assert_eq!(result.ack(), "X0");
        assert_eq!(result.kind(), "inline");
        let value = result.inline_value().unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["detail"], "boom");
    }
