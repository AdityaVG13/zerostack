//! Node runtime resolution contract: stable across shells, never ephemeral.
//!
//! Every case drives resolution through an explicit NodeEnv and an in-memory
//! probe, so precedence and refusals are asserted without mutating process
//! environment globals or planting files on disk.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use zero_codemode::node::{
    EPHEMERAL_REASON, NODE_SCHEMA, NodeEnv, NodeSource, is_ephemeral, node_candidates, node_report,
    resolve_node_with,
};

const MULTISHELL: &str = "/home/u/.local/state/fnm_multishells/1347_1785364489620/bin/node";

fn env_with(
    explicit: Option<&str>,
    zerostack_home: Option<&str>,
    fnm_dir: Option<&str>,
    user_home: Option<&str>,
    path: &[&str],
) -> NodeEnv {
    NodeEnv::new(
        explicit.map(PathBuf::from),
        zerostack_home.map(PathBuf::from),
        fnm_dir.map(PathBuf::from),
        user_home.map(PathBuf::from),
        path.iter().map(PathBuf::from).collect(),
        vec![PathBuf::from("/usr/local/bin")],
    )
}

fn present(paths: &[&str]) -> impl Fn(&Path) -> bool {
    let set: BTreeSet<PathBuf> = paths.iter().map(PathBuf::from).collect();
    move |candidate: &Path| set.contains(candidate)
}

#[test]
fn explicit_pin_wins_over_every_other_source() {
    let env = env_with(
        Some("/pinned/node"),
        Some("/install/zs"),
        Some("/fnm"),
        Some("/home/u"),
        &["/usr/bin"],
    );
    // Every source has a runtime; only the highest-precedence one may be taken.
    let probe = present(&[
        "/pinned/node",
        "/install/zs/bin/node",
        "/fnm/aliases/default/bin/node",
        "/usr/local/bin/node",
        "/usr/bin/node",
    ]);
    let resolved = resolve_node_with(&env, &probe).require().unwrap();
    assert_eq!(resolved.source, NodeSource::Explicit);
    assert_eq!(resolved.path, PathBuf::from("/pinned/node"));
}

#[test]
fn precedence_falls_through_home_then_well_known_then_path() {
    let env = env_with(
        None,
        Some("/install/zs"),
        Some("/fnm"),
        Some("/home/u"),
        &["/usr/bin"],
    );

    let resolved = resolve_node_with(&env, &present(&["/install/zs/bin/node"]))
        .require()
        .unwrap();
    assert_eq!(resolved.source, NodeSource::Home);

    let resolved = resolve_node_with(&env, &present(&["/fnm/aliases/default/bin/node"]))
        .require()
        .unwrap();
    assert_eq!(resolved.source, NodeSource::WellKnown);
    assert_eq!(
        resolved.path,
        PathBuf::from("/fnm/aliases/default/bin/node")
    );

    let resolved = resolve_node_with(&env, &present(&["/usr/bin/node"]))
        .require()
        .unwrap();
    assert_eq!(resolved.source, NodeSource::Path);
}

#[test]
fn well_known_dirs_are_ordered_and_include_defaults_without_fnm_dir() {
    let env = env_with(None, None, None, Some("/home/u"), &[]);
    let paths: Vec<String> = node_candidates(&env)
        .iter()
        .map(|candidate| candidate.path.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        paths,
        vec![
            "/home/u/.local/share/fnm/aliases/default/bin/node",
            "/home/u/.volta/bin/node",
            "/home/u/.local/bin/node",
            "/usr/local/bin/node",
        ]
    );
}

#[test]
fn ephemeral_multishell_pin_is_refused_and_never_probed() {
    let env = env_with(Some(MULTISHELL), None, None, Some("/home/u"), &["/usr/bin"]);
    // The stale pin exists on this machine, yet must not be resolved.
    let outcome = resolve_node_with(&env, &present(&[MULTISHELL, "/usr/bin/node"]));
    let resolved = outcome.resolved.clone().unwrap();
    assert_eq!(resolved.path, PathBuf::from("/usr/bin/node"));
    assert!(
        !outcome
            .probed
            .iter()
            .any(|candidate| candidate.path == Path::new(MULTISHELL))
    );
    assert_eq!(outcome.refused.len(), 1);
    assert_eq!(outcome.refused[0].reason, EPHEMERAL_REASON);
    assert_eq!(outcome.refused[0].candidate.source, NodeSource::Explicit);
}

#[test]
fn ephemeral_path_entry_is_refused_even_when_it_is_the_only_runtime() {
    let multishell_dir = "/home/u/.local/state/fnm_multishells/1347_1785364489620/bin";
    let env = env_with(None, None, None, None, &[multishell_dir]);
    let outcome = resolve_node_with(&env, &present(&[MULTISHELL]));
    assert!(outcome.resolved.is_none());
    assert_eq!(outcome.refused.len(), 1);
    let error = outcome.require().unwrap_err();
    let rendered = error.to_string();
    assert!(rendered.contains(EPHEMERAL_REASON), "{rendered}");
    assert!(rendered.contains("ZEROSTACK_NODE"), "{rendered}");
}

#[test]
fn relative_pins_and_blank_path_entries_are_discarded() {
    let env = NodeEnv::new(
        Some(PathBuf::from("relative/node")),
        Some(PathBuf::from("also/relative")),
        None,
        None,
        vec![PathBuf::from(""), PathBuf::from("rel/bin")],
        Vec::new(),
    );
    assert!(node_candidates(&env).is_empty());
}

#[test]
fn is_ephemeral_flags_multishell_paths_only() {
    assert!(is_ephemeral(Path::new(MULTISHELL)));
    assert!(!is_ephemeral(Path::new(
        "/home/u/.local/share/fnm/node-versions/v24.14.1/installation/bin/node"
    )));
    assert!(!is_ephemeral(Path::new("/usr/bin/node")));
}

#[test]
fn report_is_canonical_and_carries_order_probed_and_refused() {
    let env = env_with(Some(MULTISHELL), None, None, None, &["/usr/bin"]);
    let report = node_report(&env, &present(&["/usr/bin/node"]));
    assert_eq!(report["schema"], NODE_SCHEMA);
    assert_eq!(
        report["order"],
        serde_json::json!(["explicit", "zerostack_home", "well_known", "path"])
    );
    assert_eq!(report["resolved"], serde_json::json!(true));
    assert_eq!(report["source"], serde_json::json!("path"));
    assert_eq!(report["path"], serde_json::json!("/usr/bin/node"));
    assert_eq!(
        report["refused"],
        serde_json::json!([{
            "source": "explicit",
            "path": MULTISHELL,
            "reason": EPHEMERAL_REASON,
        }])
    );
    assert_eq!(
        report["probed"],
        serde_json::json!([
            { "source": "well_known", "path": "/usr/local/bin/node" },
            { "source": "path", "path": "/usr/bin/node" },
        ])
    );
}

#[test]
fn unresolved_report_lists_candidates_without_resolving() {
    let env = env_with(None, Some("/install/zs"), None, None, &["/usr/bin"]);
    let report = node_report(&env, &present(&[]));
    assert_eq!(report["resolved"], serde_json::json!(false));
    assert_eq!(report["path"], serde_json::Value::Null);
    assert_eq!(
        report["probed"],
        serde_json::json!([
            { "source": "zerostack_home", "path": "/install/zs/bin/node" },
            { "source": "well_known", "path": "/usr/local/bin/node" },
            { "source": "path", "path": "/usr/bin/node" },
        ])
    );
}
