//! Bounded `fs.lookup` behavior coverage (audit debt `zerostack-q33d`).

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use zsx_core::ZsxSession;

struct Fixture {
    base: PathBuf,
    root: PathBuf,
    session: ZsxSession,
    request_id: u64,
}

impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "zsx-lookup-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let root = base.join("project");
        let state_root = base.join("state");
        std::fs::create_dir_all(root.join("a")).expect("a directory");
        std::fs::create_dir_all(root.join("b")).expect("b directory");
        std::fs::write(root.join("a/alpha.rs"), "TOP_SECRET_PAYLOAD").expect("alpha rs");
        std::fs::write(root.join("b/alpha.txt"), "other secret bytes").expect("alpha txt");
        std::fs::write(root.join("zeta.md"), "zeta").expect("zeta");

        let session = ZsxSession::builder(root.clone())
            .with_state_root(state_root)
            .with_session_id("lookup-test")
            .build_canonical()
            .expect("canonical session");
        Self {
            base,
            root,
            session,
            request_id: 1,
        }
    }

    fn execute(&mut self, expression: &str) -> Result<Value, String> {
        let request_id = self.request_id;
        self.request_id += 1;
        self.session
            .execute(
                self.session.generation().expect("generation"),
                request_id,
                &format!("return await {expression};"),
                Duration::from_secs(10),
            )
            .map(|result| {
                let value = result.value;
                if value["ack"] == "ok" {
                    value["content"]["value"].clone()
                } else {
                    value
                }
            })
            .map_err(|error| error.to_string())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn paths(value: &Value) -> Vec<&str> {
    value["paths"]
        .as_array()
        .expect("paths array")
        .iter()
        .map(|path| path.as_str().expect("path string"))
        .collect()
}

#[test]
fn lookup_is_sorted_bounded_truncated_and_content_free() {
    let mut fixture = Fixture::new();
    let first = fixture
        .execute(r#"zero.fs.lookup({root: ".", query: "alpha", limit: 1})"#)
        .expect("lookup");
    let second = fixture
        .execute(r#"zero.fs.lookup({root: ".", query: "alpha", limit: 1})"#)
        .expect("repeat lookup");

    assert_eq!(first, second, "lookup order must be deterministic");
    assert_eq!(first["count"], 1);
    assert_eq!(first["total"], 2);
    assert_eq!(first["truncated"], true);
    assert_eq!(first["result_truncated"], true);
    assert_eq!(first["scan_truncated"], false);
    assert!(
        first["visited"]
            .as_u64()
            .is_some_and(|visited| visited >= 3)
    );
    assert_eq!(paths(&first), vec!["a/alpha.rs"]);
    let encoded = first.to_string();
    assert!(!encoded.contains("TOP_SECRET_PAYLOAD"));
    assert!(!encoded.contains("other secret bytes"));
}

#[test]
fn lookup_supports_globs_and_enforces_limit_bounds() {
    let mut fixture = Fixture::new();
    let glob = fixture
        .execute(r#"zero.fs.lookup({root: ".", query: "**/*.rs", limit: 10})"#)
        .expect("glob lookup");
    assert_eq!(paths(&glob), vec!["a/alpha.rs"]);
    assert_eq!(glob["truncated"], false);
    let zero_directory_glob = fixture
        .execute(r#"zero.fs.lookup({root: ".", query: "a/**/alpha.rs", limit: 10})"#)
        .expect("double-star zero-directory lookup");
    assert_eq!(paths(&zero_directory_glob), vec!["a/alpha.rs"]);
    assert_eq!(zero_directory_glob["result_truncated"], false);
    assert_eq!(zero_directory_glob["scan_truncated"], false);

    for expression in [
        r#"zero.fs.lookup({root: ".", limit: 0})"#,
        r#"zero.fs.lookup({root: ".", limit: 101})"#,
    ] {
        let error = fixture.execute(expression).expect_err("limit rejected");
        assert!(error.contains("limit must be 1..=100"), "{error}");
    }

    let oversized_query = "x".repeat(257);
    let expression = format!(
        "zero.fs.lookup({{root: \".\", query: {}, limit: 10}})",
        serde_json::to_string(&oversized_query).expect("query JSON")
    );
    let error = fixture
        .execute(&expression)
        .expect_err("oversized query rejected");
    assert!(
        error.contains("query exceeds max 256 UTF-8 bytes"),
        "{error}"
    );

    let oversized_root = "r".repeat(4_097);
    let expression = format!(
        "zero.fs.lookup({{root: {}, query: \"alpha\", limit: 10}})",
        serde_json::to_string(&oversized_root).expect("root JSON")
    );
    let error = fixture
        .execute(&expression)
        .expect_err("oversized root rejected");
    assert!(
        error.contains("root exceeds max 4096 UTF-8 bytes"),
        "{error}"
    );
}

#[test]
fn lookup_rejects_root_escape_and_skips_symlinks() {
    let mut fixture = Fixture::new();
    let absolute = fixture.root.to_string_lossy();
    let error = fixture
        .execute(&format!(
            "zero.fs.lookup({{root: {}, query: \"alpha\"}})",
            serde_json::to_string(absolute.as_ref()).expect("absolute JSON")
        ))
        .expect_err("absolute root rejected");
    assert!(error.contains("workspace-relative"), "{error}");

    let error = fixture
        .execute(r#"zero.fs.lookup({root: "../", query: "alpha"})"#)
        .expect_err("parent root rejected");
    assert!(error.contains("escapes approved root"), "{error}");

    #[cfg(unix)]
    {
        let outside = fixture.base.join("outside");
        std::fs::create_dir_all(&outside).expect("outside directory");
        std::fs::write(outside.join("alpha-secret.txt"), "DO_NOT_EXPOSE").expect("outside file");
        std::os::unix::fs::symlink(&outside, fixture.root.join("outside-link"))
            .expect("outside symlink");

        let error = fixture
            .execute(r#"zero.fs.lookup({root: "outside-link", query: "alpha"})"#)
            .expect_err("symlink root rejected");
        assert!(error.contains("via symlink"), "{error}");

        let root_lookup = fixture
            .execute(r#"zero.fs.lookup({root: ".", query: "alpha", limit: 10})"#)
            .expect("root lookup");
        assert!(
            paths(&root_lookup)
                .iter()
                .all(|path| !path.contains("outside-link")),
            "directory walk must skip symlink entries"
        );
        assert!(!root_lookup.to_string().contains("DO_NOT_EXPOSE"));
    }
}

#[test]
fn lookup_missing_root_is_empty_not_unconfined() {
    let mut fixture = Fixture::new();
    let value = fixture
        .execute(r#"zero.fs.lookup({root: "missing", query: "alpha", limit: 10})"#)
        .expect("missing root result");
    assert_eq!(value["count"], 0);
    assert_eq!(value["total"], 0);
    assert_eq!(value["truncated"], false);
    assert!(paths(&value).is_empty());
}

#[test]
fn lookup_reports_depth_pruning_as_scan_truncation() {
    let mut fixture = Fixture::new();
    let mut directory = fixture.root.join("deep");
    for _ in 0..66 {
        std::fs::create_dir_all(&directory).expect("deep directory");
        directory = directory.join("d");
    }

    let value = fixture
        .execute(r#"zero.fs.lookup({root: ".", query: "never-match", limit: 10})"#)
        .expect("depth-bounded lookup");
    assert_eq!(value["count"], 0);
    assert_eq!(value["scan_truncated"], true);
    assert_eq!(value["truncated"], true);
}

#[test]
fn lookup_omits_oversized_matching_paths_and_reports_truncation() {
    let mut fixture = Fixture::new();
    let mut directory = fixture.root.clone();
    for index in 0..18 {
        directory = directory.join(format!("segment-{index:02}-abcdefghijklmnopqr"));
    }
    std::fs::create_dir_all(&directory).expect("long path directories");
    std::fs::write(
        directory.join("oversized-match.rs"),
        "content is never read",
    )
    .expect("long path file");

    let value = fixture
        .execute(r#"zero.fs.lookup({root: ".", query: "oversized-match", limit: 10})"#)
        .expect("output-bounded lookup");
    assert_eq!(value["count"], 0);
    assert_eq!(value["scan_truncated"], false);
    assert_eq!(value["oversized_results_omitted"], 1);
    assert_eq!(value["truncated"], true);
}
#[test]
fn lookup_hard_caps_visited_entries_and_reports_incompleteness() {
    let mut fixture = Fixture::new();
    let crowded = fixture.root.join("crowded");
    std::fs::create_dir_all(&crowded).expect("crowded directory");
    for index in 0..10_001 {
        std::fs::File::create(crowded.join(format!("entry-{index:05}"))).expect("crowded entry");
    }

    let value = fixture
        .execute(r#"zero.fs.lookup({root: ".", query: "never-match", limit: 10})"#)
        .expect("visited-bounded lookup");
    assert_eq!(value["visited"], 10_000);
    assert_eq!(value["scan_truncated"], true);
    assert_eq!(value["truncated"], true);
}
