    use super::*;

    #[test]
    fn every_surface_method_has_exactly_one_help_entry() {
        let mut expected: Vec<String> = METHODS
            .iter()
            .map(|(surface, method)| format!("{surface}.{method}"))
            .collect();
        let mut actual: Vec<String> = HELP_ENTRIES.iter().map(path_of).collect();
        expected.sort();
        actual.sort();
        assert_eq!(expected, actual, "help entries must mirror METHODS exactly");
    }

    #[test]
    fn scoring_finds_bulk_reads_and_cas_writes() {
        let result = help_search(&json!([{ "query": "read many files" }]));
        let first = result["results"][0]["path"].as_str().unwrap();
        assert_eq!(first, "fs.multi_read", "got {result}");
        assert!(
            result["results"][0]["signature"]
                .as_str()
                .unwrap()
                .contains("positional array")
        );

        let result = help_search(&json!({"query": "compare and swap overwrite"}));
        let paths: Vec<&str> = result["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["path"].as_str().unwrap())
            .collect();
        assert!(paths.contains(&"fs.write"), "paths: {paths:?}");
    }

    #[test]
    fn exact_path_lookup_returns_single_entry() {
        let result = help_search(&json!({"query": "fs.transact"}));
        assert_eq!(result["total"], 1);
        assert_eq!(result["results"][0]["path"], "fs.transact");

        let result = help_search(&json!({"query": "multi edit files in parallel"}));
        let first = result["results"][0]["path"].as_str().unwrap();
        assert_eq!(first, "fs.multi_edit", "got {result}");
    }

    #[test]
    fn empty_query_browses_with_pagination_and_namespace_filter() {
        let all = help_search(&json!({}));
        assert_eq!(all["total"].as_u64().unwrap() as usize, HELP_ENTRIES.len());
        assert_eq!(all["count"], 10);
        assert_eq!(all["next"]["offset"], 10);

        let page2 = help_search(&json!({"query": "", "offset": 10, "limit": 10}));
        assert_eq!(page2["count"], 10);

        let graph = help_search(&json!({"namespace": "graph"}));
        assert_eq!(graph["total"], 9);
        assert!(
            graph["results"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["path"].as_str().unwrap().starts_with("graph."))
        );
    }

    #[test]
    fn malformed_args_degrade_to_browse_and_globals_are_documented() {
        let result = help_search(&json!(42));
        assert_eq!(result["ok"], true);
        assert!(result["total"].as_u64().unwrap() > 0);
        assert_eq!(result["sandbox_globals"][0], "Promise");
    }
