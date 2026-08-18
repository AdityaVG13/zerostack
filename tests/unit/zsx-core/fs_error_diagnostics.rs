//! Fused-edit no-target diagnostic coverage (audit debt `zerostack-yurz`).

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
            "zsx-fused-diagnostic-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let root = base.join("project");
        let state_root = base.join("state");
        std::fs::create_dir_all(&root).expect("project root");
        std::fs::write(root.join("target.txt"), "TOP_SECRET_FILE_CONTENT\n").expect("target file");
        let session = ZsxSession::builder(root.clone())
            .with_state_root(state_root)
            .with_session_id("fused-diagnostic-test")
            .build_canonical()
            .expect("canonical session");
        Self {
            base,
            root,
            session,
            request_id: 1,
        }
    }

    fn execute(&mut self, program: &str) -> Result<Value, String> {
        let request_id = self.request_id;
        self.request_id += 1;
        self.session
            .execute(
                self.session.generation().expect("generation"),
                request_id,
                program,
                Duration::from_secs(10),
            )
            .map(|result| result.value)
            .map_err(|error| error.to_string())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn edit_program(query: &str, scope: &str) -> String {
    format!(
        "return await zero.fs.edit({{query:{},scope:{},uniqueness:\"exactly_one\",preimage:\"absent preimage\",replacement:\"replacement\"}});",
        serde_json::to_string(query).expect("query JSON"),
        serde_json::to_string(scope).expect("scope JSON"),
    )
}

#[test]
fn fused_edit_no_target_is_bounded_actionable_and_non_mutating() {
    let mut fixture = Fixture::new();
    let before = std::fs::read(fixture.root.join("target.txt")).expect("before bytes");
    let query = format!(
        "{} second-token third-token fourth-token TOP_SECRET_QUERY_SUFFIX",
        "界".repeat(180)
    );
    let scope = ".";

    let error = fixture
        .execute(&edit_program(&query, &scope))
        .expect_err("missing fused target must fail");
    assert!(error.contains("has no target"), "{error}");
    assert!(error.contains("query="), "{error}");
    assert!(error.contains("tokens="), "{error}");
    assert!(error.contains("scope="), "{error}");
    assert!(
        error.contains("literal case-sensitive substring"),
        "{error}"
    );
    assert!(error.contains("uniqueness remains exactly_one"), "{error}");
    assert!(error.contains("preimage is still required"), "{error}");
    assert!(error.contains("+2 more count=5"), "{error}");
    assert!(
        error.contains("…"),
        "long previews must be visibly truncated: {error}"
    );
    assert!(
        !error.contains("TOP_SECRET_QUERY_SUFFIX"),
        "diagnostic must not retain the unbounded query tail: {error}"
    );
    assert!(
        !error.contains("TOP_SECRET_FILE_CONTENT"),
        "diagnostic must never leak file contents: {error}"
    );
    assert!(
        error.len() <= 700,
        "diagnostic must remain bounded: {} bytes",
        error.len()
    );
    assert_eq!(
        std::fs::read(fixture.root.join("target.txt")).expect("after bytes"),
        before,
        "no-target failure must not mutate the file"
    );
}

#[test]
fn ordinary_fs_failure_does_not_gain_fused_edit_guidance() {
    let mut fixture = Fixture::new();
    let value = fixture
        .execute(
            r#"return await zero.fs.compound("search", {query:"definitely absent", path:"."});"#,
        )
        .expect("ordinary search remains a read-only result");
    let encoded = value.to_string();
    assert!(!encoded.contains("uniqueness remains exactly_one"));
    assert!(!encoded.contains("preimage is still required"));
}
