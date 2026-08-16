    use super::lower;
    use serde_json::json;

    #[test]
    fn regex_alias_becomes_fs_search_query() {
        let (_engine, op, args) = lower(
            "fs",
            "compound",
            json!(["search", {"regex": "HIT_MARKER", "path": "src"}]),
        )
        .expect("lower");
        assert_eq!(op, "fs.multiSearch");
        assert_eq!(args["queries"][0]["query"], "HIT_MARKER");
        assert_eq!(args["queries"][0]["paths"], json!(["src"]));
    }

    #[test]
    fn query_wins_over_regex() {
        let (_engine, op, args) = lower(
            "fs",
            "compound",
            json!(["grep", {"query": "keep", "regex": "drop"}]),
        )
        .expect("lower");
        assert_eq!(op, "fs.search");
        assert_eq!(args["query"], "keep");
    }

    #[test]
    fn list_paths_become_multilist_items() {
        let (_engine, op, args) = lower(
            "fs",
            "compound",
            json!(["list", {"paths": ["crates/zsx-core/src", "crates/zsx/src"]}]),
        )
        .expect("lower");
        assert_eq!(op, "fs.multiList");
        assert_eq!(args["items"], json!(["crates/zsx-core/src", "crates/zsx/src"]));
        assert!(args.get("paths").is_none());
    }

    #[test]
    fn search_path_becomes_multisearch_paths() {
        let (_engine, op, args) = lower(
            "fs",
            "compound",
            json!(["search", {"query": "fs.compound", "path": "crates/"}]),
        )
        .expect("lower");
        assert_eq!(op, "fs.multiSearch");
        assert_eq!(args["queries"][0]["query"], "fs.compound");
        assert_eq!(args["queries"][0]["paths"], json!(["crates/"]));
    }

    #[test]
    fn shell_string_array_becomes_argv() {
        let (_engine, op, args) = lower(
            "token",
            "shell",
            json!(["wc", "-l", "README.md"]),
        )
        .expect("lower");
        assert_eq!(op, "shell");
        assert_eq!(args["argv"], json!(["wc", "-l", "README.md"]));
        assert!(args.get("command").is_none());
    }

    #[test]
    fn read_paths_stay_multiread_paths() {
        let (_engine, op, args) = lower(
            "fs",
            "compound",
            json!(["read", {"paths": ["Cargo.toml", "README.md"]}]),
        )
        .expect("lower");
        assert_eq!(op, "fs.multiRead");
        assert_eq!(args["paths"], json!(["Cargo.toml", "README.md"]));
        assert!(args.get("items").is_none());
    }

    #[test]
    fn expand_rejects_query_ref_without_dumping_payload() {
        let payload = format!(
            "gz://query/{}",
            r#"{"evidence_ref":"1049","noise":""# .repeat(40)
        );
        let err = lower("token", "expand", json!({"ref": payload})).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("cannot expand query ref"), "{message}");
        assert!(message.contains("tz://blob"), "{message}");
        assert!(!message.contains("1049"), "{message}");
        assert!(message.len() < 400, "{}", message.len());
    }

    #[test]
    fn expand_lists_schemes_on_bare_path() {
        let err = lower("token", "expand", json!({"ref": "AGENTS.md"})).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("tz://blob"), "{message}");
        assert!(message.contains("fz://blob"), "{message}");
        assert!(message.contains("gz://blob"), "{message}");
    }

    #[test]
    fn expand_rejects_file_line_as_window() {
        let err = lower(
            "token",
            "expand",
            json!({"ref": "fz://file/crates/zsx-core/src/lower.rs#L25"}),
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("location ref"), "{message}");
        assert!(message.contains("#Lstart-Lend") || message.contains("start_line"), "{message}");
        assert!(!message.contains("bad window"), "{message}");
    }

