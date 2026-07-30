//! `zerostack locate` manifest contract.
//!
//! The manifest is the whole point of the bead: a harness must read one command
//! instead of hand-authoring seven absolute paths, so its shape, its precedence
//! order, and its refusals are locked here.

use std::path::{Path, PathBuf};

use zero_codemode::{
    artifact_candidates, is_ephemeral, locate_manifest, resolve_artifact, ArtifactEnv,
    DiscoveryEnv, HarnessArtifact, ManifestFacts, Source, StorePaths, EPHEMERAL_REASON,
    HARNESS_ARTIFACTS, MANIFEST_SCHEMA,
};

fn env_with(home: Option<&str>, dev_root: Option<&str>, path: &[&str]) -> DiscoveryEnv {
    DiscoveryEnv::new(
        home.map(PathBuf::from),
        dev_root.map(PathBuf::from),
        None,
        None,
        path.iter().map(PathBuf::from).collect(),
        Vec::new(),
    )
}

fn present(paths: &[&str]) -> impl Fn(&Path) -> bool {
    let owned: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    move |candidate: &Path| owned.iter().any(|path| path == candidate)
}

fn facts() -> ManifestFacts {
    ManifestFacts {
        host_version: "9.9.9".to_owned(),
        protocol: "zerostack-codemode-host/v1".to_owned(),
        store: StorePaths::from_store_root(PathBuf::from("/proj/.zerostack/tokenzero")),
    }
}

#[test]
fn one_install_root_resolves_every_field_a_harness_needs() {
    let env = env_with(Some("/install/zs"), None, &[]);
    let installed = [
        "/install/zs/bin/zerostack-codemode-host",
        "/install/zs/bin/fszero-codemode",
        "/install/zs/bin/graphzero-codemode",
        "/install/zs/bin/tokenzero-codemode",
        "/install/zs/bin/node",
        "/install/zs/lib/raw-runtime.js",
        "/install/zs/lib/substrates.js",
    ];
    let probe = present(&installed);
    let manifest = locate_manifest(&env, &ArtifactEnv::default(), &facts(), &probe, &probe);

    assert_eq!(manifest["schema"], MANIFEST_SCHEMA);
    assert_eq!(
        manifest["aggregate_host"]["path"],
        "/install/zs/bin/zerostack-codemode-host"
    );
    for (key, path) in [
        ("fs", "/install/zs/bin/fszero-codemode"),
        ("graph", "/install/zs/bin/graphzero-codemode"),
        ("token", "/install/zs/bin/tokenzero-codemode"),
    ] {
        assert_eq!(manifest["binaries"][key]["resolved"], true);
        assert_eq!(manifest["binaries"][key]["path"], path);
    }
    assert_eq!(manifest["node"]["path"], "/install/zs/bin/node");
    assert_eq!(
        manifest["runtime_module"]["path"],
        "/install/zs/lib/raw-runtime.js"
    );
    assert_eq!(
        manifest["substrate_module"]["path"],
        "/install/zs/lib/substrates.js"
    );
    assert_eq!(manifest["store_root"], "/proj/.zerostack/tokenzero");
    assert_eq!(
        manifest["journal_dir"],
        "/proj/.zerostack/tokenzero/journal"
    );
    assert_eq!(manifest["versions"]["host"], "9.9.9");
    assert_eq!(manifest["versions"]["manifest_schema"], MANIFEST_SCHEMA);
    assert_eq!(
        manifest["capabilities"],
        serde_json::json!(["fs", "graph", "token"])
    );
}

#[test]
fn manifest_order_leads_with_explicit_then_mirrors_binary_discovery() {
    let env = env_with(Some("/install/zs"), None, &[]);
    let manifest = locate_manifest(
        &env,
        &ArtifactEnv::default(),
        &facts(),
        &present(&[]),
        &present(&[]),
    );
    assert_eq!(
        manifest["order"],
        serde_json::json!([
            "explicit",
            "zerostack_home",
            "dev_checkout",
            "xdg_data",
            "platform_install",
            "path"
        ])
    );
}

#[test]
fn explicit_pin_outranks_every_install_root() {
    let env = env_with(Some("/install/zs"), None, &[]);
    let artifacts = ArtifactEnv::new(
        Some(PathBuf::from("/opt/node/bin/node")),
        None,
        Some(PathBuf::from("/pkg/router/substrates.js")),
    );
    let probe = present(&[
        "/opt/node/bin/node",
        "/install/zs/bin/node",
        "/pkg/router/substrates.js",
        "/install/zs/lib/substrates.js",
    ]);
    let manifest = locate_manifest(&env, &artifacts, &facts(), &probe, &probe);
    assert_eq!(manifest["node"]["source"], "explicit");
    assert_eq!(manifest["node"]["path"], "/opt/node/bin/node");
    assert_eq!(manifest["substrate_module"]["source"], "explicit");
}

#[test]
fn relative_pin_is_discarded_rather_than_half_honored() {
    let env = env_with(Some("/install/zs"), None, &[]);
    let artifacts = ArtifactEnv::new(Some(PathBuf::from("node")), None, None);
    let candidates = artifact_candidates(HarnessArtifact::Node, &env, &artifacts);
    assert!(candidates.iter().all(|c| c.path.is_absolute()));
    assert_eq!(candidates[0].source, Source::Home);
}

#[test]
fn dev_checkout_puts_modules_beside_the_repository_not_beside_the_executable() {
    let env = env_with(None, Some("/work"), &[]);
    let candidates = artifact_candidates(
        HarnessArtifact::RuntimeModule,
        &env,
        &ArtifactEnv::default(),
    );
    assert_eq!(candidates[0].source, Source::DevCheckout);
    assert_eq!(
        candidates[0].path,
        PathBuf::from("/work/ZeroStack/lib/raw-runtime.js")
    );
}

#[test]
fn ephemeral_multishell_node_is_refused_with_a_reason_not_resolved() {
    let ephemeral = "/home/u/.local/state/fnm_multishells/1347_1785364489620/bin";
    assert!(is_ephemeral(Path::new(ephemeral)));
    let env = env_with(None, None, &[ephemeral]);
    let outcome = resolve_artifact(
        HarnessArtifact::Node,
        &env,
        &ArtifactEnv::default(),
        &present(&[&format!("{ephemeral}/node")]),
    );
    assert!(outcome.resolved.is_none());
    assert_eq!(outcome.refused.len(), 1);
    assert_eq!(outcome.refused[0].reason, EPHEMERAL_REASON);

    let manifest = locate_manifest(
        &env,
        &ArtifactEnv::default(),
        &facts(),
        &present(&[&format!("{ephemeral}/node")]),
        &present(&[]),
    );
    assert_eq!(manifest["node"]["resolved"], false);
    assert_eq!(manifest["node"]["refused"][0]["reason"], EPHEMERAL_REASON);
}

#[test]
fn unresolved_entries_report_probed_paths_and_shrink_capabilities() {
    let env = env_with(Some("/install/zs"), None, &[]);
    let probe = present(&["/install/zs/bin/tokenzero-codemode"]);
    let manifest = locate_manifest(&env, &ArtifactEnv::default(), &facts(), &probe, &probe);
    assert_eq!(manifest["capabilities"], serde_json::json!(["token"]));
    assert_eq!(manifest["aggregate_host"]["resolved"], false);
    for artifact in HARNESS_ARTIFACTS {
        let entry = &manifest[artifact.manifest_key()];
        assert_eq!(entry["resolved"], false, "{}", artifact.manifest_key());
        assert!(entry["probed"]
            .as_array()
            .is_some_and(|probed| !probed.is_empty()));
    }
}

#[test]
fn artifact_keys_and_file_names_are_distinct() {
    let keys: std::collections::BTreeSet<&str> =
        HARNESS_ARTIFACTS.iter().map(|a| a.manifest_key()).collect();
    assert_eq!(keys.len(), HARNESS_ARTIFACTS.len());
    let envs: std::collections::BTreeSet<&str> =
        HARNESS_ARTIFACTS.iter().map(|a| a.env_var()).collect();
    assert_eq!(envs.len(), HARNESS_ARTIFACTS.len());
}
