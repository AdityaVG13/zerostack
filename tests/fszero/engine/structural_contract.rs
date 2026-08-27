#[path = "../common/mod.rs"]
mod common;

use common::{TestRoot, expand_text};
use fs_zero::{FSZeroSession, codemode_execute_plan};

fn assert_recipe_succeeds(session: &FSZeroSession, ack: &str) {
    if ack != "C" {
        let error = session
            .expand("codemode/error")
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_else(|| "(no error payload)".to_string());
        panic!("expected CodeMode completion, got {ack}: {error}");
    }
}

#[test]
fn structural_defs_indexes_rust_types_and_tests_lower_to_callers() {
    let root = TestRoot::new("structural_contract");
    root.write(
        "Cargo.toml",
        b"[package]\nname = \"structural-contract\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    root.write("src/lib.rs", b"pub mod engine;\n");
    root.write(
        "src/engine.rs",
        br#"pub struct ProofEngine {
    seed: u64,
}

pub fn snap_target(engine: &ProofEngine) -> u64 {
    engine.seed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_target_is_stable() {
        let fingerprint = snap_target(&ProofEngine { seed: 7 });
        assert_eq!(fingerprint, 7);
    }
}
"#,
    );

    let mut session = FSZeroSession::with_repo_store(root.path());

    let ack = codemode_execute_plan(&mut session, "structural:defs:ProofEngine");
    assert_recipe_succeeds(&session, &ack);
    let definitions = expand_text(&session, "search");
    assert!(
        definitions.contains("DEF: src/engine.rs: ProofEngine"),
        "Rust type definition missing from structural defs: {definitions}"
    );

    let ack = codemode_execute_plan(&mut session, "structural:tests:snap_target");
    assert_recipe_succeeds(&session, &ack);
    let tests = expand_text(&session, "search");
    assert!(
        tests.contains("snap_target_is_stable"),
        "tests query must lower to target callers: {tests}"
    );
}
