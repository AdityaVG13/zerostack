    use super::lower;
    use serde_json::json;

    #[test]
    fn contents_alias_becomes_content() {
        let (_engine, op, args) = lower(
            "fs",
            "write",
            json!({"path": "tmp.txt", "contents": "LINE1\nLINE2"}),
        )
        .expect("lower");
        assert_eq!(op, "fs.write");
        assert_eq!(args["path"], "tmp.txt");
        assert_eq!(args["content"], "LINE1\nLINE2");
        assert!(args.get("contents").is_none());
    }

    #[test]
    fn body_alias_on_compound_write() {
        let (_engine, op, args) = lower(
            "fs",
            "compound",
            json!(["write", {"path": "a.rs", "body": "fn main() {}\n"}]),
        )
        .expect("lower");
        assert_eq!(op, "fs.write");
        assert_eq!(args["content"], "fn main() {}\n");
        assert!(args.get("body").is_none());
    }

    #[test]
    fn transact_contents_alias_writes_bytes() {
        let (_engine, op, args) = lower(
            "fs",
            "transact",
            json!([{"op": "write", "path": "n.txt", "contents": "A\nB\nC"}]),
        )
        .expect("lower");
        assert_eq!(op, "fs.transact");
        assert_eq!(args["steps"][0]["content"], "A\nB\nC");
        assert!(args["steps"][0].get("contents").is_none());
    }

    #[test]
    fn matching_content_and_contents_keeps_content() {
        let (_engine, _op, args) = lower(
            "fs",
            "write",
            json!({"path": "t.txt", "content": "same", "contents": "same"}),
        )
        .expect("lower");
        assert_eq!(args["content"], "same");
        assert!(args.get("contents").is_none());
    }

    #[test]
    fn conflicting_content_and_contents_fails() {
        let err = lower(
            "fs",
            "write",
            json!({"path": "t.txt", "content": "a", "contents": "b"}),
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("write_content_conflict"), "{text}");
    }

    #[test]
    fn path_only_write_fails_loud() {
        let err = lower("fs", "write", json!({"path": "empty.txt"})).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("missing_write_content"), "{text}");
    }

    #[test]
    fn unknown_write_key_fails_loud() {
        let err = lower(
            "fs",
            "write",
            json!({"path": "t.txt", "payload": "nope"}),
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("unknown_write_field"), "{text}");
        assert!(text.contains("payload"), "{text}");
    }

    #[test]
    fn empty_string_content_is_allowed() {
        let (_engine, op, args) =
            lower("fs", "write", json!({"path": "empty.txt", "content": ""}))
                .expect("lower");
        assert_eq!(op, "fs.write");
        assert_eq!(args["content"], "");
    }

    #[test]
    fn object_content_still_rejected() {
        let err = lower(
            "fs",
            "write",
            json!({"path": "t.txt", "content": {"ack": "ok"}}),
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("non_byte_provenance"), "{text}");
    }
