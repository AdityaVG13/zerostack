    use super::{HELP_ENTRIES, help_search};
    use crate::lower::METHODS;
    use serde_json::json;

    #[test]
    fn help_entries_match_methods() {
        assert_eq!(HELP_ENTRIES.len(), METHODS.len());
        for ((surface, method), entry) in METHODS.iter().zip(HELP_ENTRIES.iter()) {
            assert_eq!(*surface, entry.surface, "{surface}.{method}");
            assert_eq!(*method, entry.method, "{surface}.{method}");
        }
    }

    fn paths(query: &str, limit: usize, offset: usize) -> Vec<String> {
        let value = help_search(&json!({"query": query, "limit": limit, "offset": offset}));
        value["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["path"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn case_fold_and_separators_same_paths() {
        for group in [
            ["read many files", "READ MANY FILES", "read_many_files"],
            ["multi_edit", "multi edit", "MULTI-EDIT"],
        ] {
            let first = paths(group[0], 50, 0);
            assert!(!first.is_empty(), "{}", group[0]);
            for q in &group[1..] {
                assert_eq!(paths(q, 50, 0), first, "{q}");
            }
        }
    }

    #[test]
    fn pagination_concat_matches_unpaged_prefix() {
        for query in ["", "read"] {
            let unlimited = paths(query, 50, 0);
            let n = 3.min(unlimited.len().saturating_div(2).max(1));
            let page1 = paths(query, n, 0);
            let page2 = paths(query, n, n);
            let mut concat = page1.clone();
            concat.extend(page2);
            assert_eq!(concat, unlimited[..concat.len()].to_vec(), "query={query:?}");
            let paged = help_search(&json!({"query": query, "limit": n, "offset": 0}));
            let remaining = paged["remaining"].as_u64().unwrap() as usize;
            assert_eq!(remaining, unlimited.len().saturating_sub(page1.len()));
        }
    }

    #[test]
    fn adding_a_token_does_not_drop_prior_matches() {
        let base = paths("read", 50, 0);
        let broader = paths("read many", 50, 0);
        for path in &base {
            assert!(
                broader.contains(path),
                "adding token dropped {path}: {broader:?}"
            );
        }
    }

