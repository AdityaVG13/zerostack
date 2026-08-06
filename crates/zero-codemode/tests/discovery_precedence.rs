//! Discovery precedence contract: a harness config needs zero absolute paths.
//!
//! Every case drives resolution through an explicit DiscoveryEnv and an
//! in-memory probe, so precedence is asserted without mutating process
//! environment globals or planting files on disk.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use zero_codemode::discovery::{
    DiscoveryEnv, HARNESS_BINARIES, HarnessBinary, Source, candidates, locate_report, resolve_with,
};

fn env_with(
    home: Option<&str>,
    dev_root: Option<&str>,
    xdg: Option<&str>,
    user_home: Option<&str>,
    path: &[&str],
) -> DiscoveryEnv {
    DiscoveryEnv::new(
        home.map(PathBuf::from),
        dev_root.map(PathBuf::from),
        xdg.map(PathBuf::from),
        user_home.map(PathBuf::from),
        path.iter().map(PathBuf::from).collect(),
        vec![PathBuf::from("/opt/zerostack/bin")],
    )
}

fn present(paths: &[&str]) -> impl Fn(&Path) -> bool {
    let set: BTreeSet<PathBuf> = paths.iter().map(PathBuf::from).collect();
    move |candidate: &Path| set.contains(candidate)
}

#[test]
fn zerostack_home_wins_over_every_other_source() {
    let env = env_with(
        Some("/install/zs"),
        Some("/dev/checkouts"),
        Some("/data"),
        Some("/userhome"),
        &["/usr/bin"],
    );
    // Every source has the binary; only the highest-precedence one may be taken.
    let probe = present(&[
        "/install/zs/bin/fszero-codemode",
        "/dev/checkouts/FSZero/target/release/fszero-codemode",
        "/data/zerostack/bin/fszero-codemode",
        "/opt/zerostack/bin/fszero-codemode",
        "/usr/bin/fszero-codemode",
    ]);
    let resolved = resolve_with(HarnessBinary::FsDelegate, &env, &probe).unwrap();
    assert_eq!(resolved.source, Source::Home);
    assert_eq!(
        resolved.path,
        PathBuf::from("/install/zs/bin/fszero-codemode")
    );
}

#[test]
fn dev_checkout_override_outranks_xdg_and_path() {
    let env = env_with(
        None,
        Some("/dev/checkouts"),
        Some("/data"),
        Some("/userhome"),
        &["/usr/bin"],
    );
    let probe = present(&[
        "/dev/checkouts/TokenZero/target/release/tokenzero-codemode",
        "/data/zerostack/bin/tokenzero-codemode",
        "/usr/bin/tokenzero-codemode",
    ]);
    let resolved = resolve_with(HarnessBinary::TokenDelegate, &env, &probe).unwrap();
    assert_eq!(resolved.source, Source::DevCheckout);
    assert_eq!(
        resolved.path,
        PathBuf::from("/dev/checkouts/TokenZero/target/release/tokenzero-codemode")
    );
}

#[test]
fn xdg_data_home_outranks_platform_and_path() {
    let env = env_with(None, None, Some("/data"), Some("/userhome"), &["/usr/bin"]);
    let probe = present(&[
        "/data/zerostack/bin/graphzero-codemode",
        "/opt/zerostack/bin/graphzero-codemode",
        "/usr/bin/graphzero-codemode",
    ]);
    let resolved = resolve_with(HarnessBinary::GraphDelegate, &env, &probe).unwrap();
    assert_eq!(resolved.source, Source::XdgData);
}

#[test]
fn xdg_defaults_to_local_share_under_user_home() {
    let env = env_with(None, None, None, Some("/userhome"), &[]);
    let probe = present(&["/userhome/.local/share/zerostack/bin/zerostack-codemode-host"]);
    let resolved = resolve_with(HarnessBinary::AggregateHost, &env, &probe).unwrap();
    assert_eq!(resolved.source, Source::XdgData);
}

#[test]
fn explicit_xdg_data_home_replaces_the_default_rather_than_adding_to_it() {
    let env = env_with(None, None, Some("/data"), Some("/userhome"), &[]);
    // Only the HOME-derived default exists. An explicit XDG_DATA_HOME is a
    // redirect, so falling back to the default would silently ignore it.
    let probe = present(&["/userhome/.local/share/zerostack/bin/tokenzero-codemode"]);
    assert!(resolve_with(HarnessBinary::TokenDelegate, &env, &probe).is_err());
}

#[test]
fn platform_install_dir_outranks_path() {
    let env = env_with(None, None, None, None, &["/usr/bin"]);
    let probe = present(&[
        "/opt/zerostack/bin/fszero-codemode",
        "/usr/bin/fszero-codemode",
    ]);
    let resolved = resolve_with(HarnessBinary::FsDelegate, &env, &probe).unwrap();
    assert_eq!(resolved.source, Source::PlatformInstall);
}

#[test]
fn path_is_the_last_resort_and_honors_entry_order() {
    let env = env_with(None, None, None, None, &["/a/bin", "/b/bin"]);
    let probe = present(&["/a/bin/fszero-codemode", "/b/bin/fszero-codemode"]);
    let resolved = resolve_with(HarnessBinary::FsDelegate, &env, &probe).unwrap();
    assert_eq!(resolved.source, Source::Path);
    assert_eq!(resolved.path, PathBuf::from("/a/bin/fszero-codemode"));
}

#[test]
fn empty_and_relative_pins_are_discarded_not_half_honored() {
    // An exported-but-empty variable is a shell artifact; a relative root would
    // make resolution depend on the spawning cwd.
    let env = DiscoveryEnv::new(
        Some(PathBuf::from("   ")),
        Some(PathBuf::from("relative/checkouts")),
        None,
        None,
        vec![PathBuf::from("/usr/bin")],
        Vec::new(),
    );
    let all: Vec<Source> = candidates(HarnessBinary::FsDelegate, &env)
        .into_iter()
        .map(|candidate| candidate.source)
        .collect();
    assert_eq!(all, vec![Source::Path]);
}

#[test]
fn relative_path_entries_are_skipped() {
    let env = env_with(None, None, None, None, &["", "relative", "/usr/bin"]);
    let paths: Vec<PathBuf> = candidates(HarnessBinary::FsDelegate, &env)
        .into_iter()
        .filter(|candidate| candidate.source == Source::Path)
        .map(|candidate| candidate.path)
        .collect();
    assert_eq!(paths, vec![PathBuf::from("/usr/bin/fszero-codemode")]);
}

#[test]
fn duplicate_directories_are_probed_once() {
    let env = env_with(None, None, None, None, &["/usr/bin", "/usr/bin"]);
    let paths: Vec<PathBuf> = candidates(HarnessBinary::FsDelegate, &env)
        .into_iter()
        .map(|candidate| candidate.path)
        .collect();
    let unique: BTreeSet<&PathBuf> = paths.iter().collect();
    assert_eq!(paths.len(), unique.len());
}

#[test]
fn unresolved_binary_reports_every_probed_candidate() {
    let env = env_with(
        Some("/install/zs"),
        Some("/dev/checkouts"),
        Some("/data"),
        None,
        &["/usr/bin"],
    );
    let error = resolve_with(HarnessBinary::FsDelegate, &env, &|_| false).unwrap_err();
    assert_eq!(error.binary, HarnessBinary::FsDelegate);
    let rendered = error.to_string();
    // A bare ENOENT from spawn is undiagnosable; the failure must name what it tried.
    assert!(rendered.contains("fszero-codemode"), "{rendered}");
    assert!(rendered.contains("/install/zs/bin"), "{rendered}");
    assert!(rendered.contains("zerostack_home"), "{rendered}");
    assert_eq!(error.probed.len(), 5);
}

#[test]
fn each_binary_resolves_from_one_install_root_with_no_absolute_config() {
    // The shipped-config case: one env var, four binaries, zero hand-written paths.
    let env = env_with(Some("/install/zs"), None, None, None, &[]);
    let installed: Vec<String> = HARNESS_BINARIES
        .iter()
        .map(|binary| format!("/install/zs/bin/{}", binary.file_stem()))
        .collect();
    let refs: Vec<&str> = installed.iter().map(String::as_str).collect();
    let probe = present(&refs);
    for binary in HARNESS_BINARIES {
        let resolved = resolve_with(binary, &env, &probe).unwrap();
        assert_eq!(resolved.source, Source::Home);
    }
}

#[test]
fn binary_stems_and_config_keys_are_distinct() {
    let stems: BTreeSet<&str> = HARNESS_BINARIES.iter().map(|b| b.file_stem()).collect();
    let keys: BTreeSet<&str> = HARNESS_BINARIES.iter().map(|b| b.config_key()).collect();
    assert_eq!(stems.len(), HARNESS_BINARIES.len());
    assert_eq!(keys.len(), HARNESS_BINARIES.len());
}

#[test]
fn locate_report_is_stable_and_labels_resolution_source() {
    let env = env_with(Some("/install/zs"), None, None, None, &[]);
    let probe = present(&["/install/zs/bin/tokenzero-codemode"]);
    let report = locate_report(&env, &probe);
    assert_eq!(report["schema"], "zerostack.binary_discovery.v1");
    assert_eq!(
        report["order"],
        serde_json::json!([
            "zerostack_home",
            "dev_checkout",
            "xdg_data",
            "platform_install",
            "path"
        ])
    );
    assert_eq!(report["binaries"]["token"]["resolved"], true);
    assert_eq!(report["binaries"]["token"]["source"], "zerostack_home");
    assert_eq!(
        report["binaries"]["token"]["path"],
        "/install/zs/bin/tokenzero-codemode"
    );
    // An engine that is not installed is reported, not fatal to the others.
    assert_eq!(report["binaries"]["fs"]["resolved"], false);
    assert!(
        report["binaries"]["fs"]["probed"]
            .as_array()
            .is_some_and(|probed| !probed.is_empty())
    );
}
