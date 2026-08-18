//! Bounded one-file read grant capability (bead `zerostack-k91e`).
//!
//! Exercises the pure capability core (`zsx_core::read_grant`) and the
//! surface lowering for `fs.readGrant` with real temp files and symlinks:
//! mint bounds the grant to one canonical regular file outside the session
//! root, take consumes it all-or-nothing on an exact canonical match, plan
//! re-verifies fresh on the session side and selects a root no wider than
//! the granted file's own parent, and post-verification fails closed when
//! the file is swapped or re-symlinked during the read.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;
use zsx_core::lower;
use zsx_core::read_grant::{
    MAX_SESSION_READ_GRANTS, MAX_SESSION_READ_GRANT_LIFETIME_MS, GrantedReadFile,
    SessionReadGrant, absolute_read_paths, mint_read_grant, plan_granted_read, post_verify_read,
    take_read_grants,
};
use zsx_core::read_grant::SESSION_READ_GRANT_SCHEMA;

static DIR_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One isolated fixture: a canonical workspace root and an external
/// directory (not under the root) with one file each.
struct Fixture {
    root: PathBuf,
    external: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let sequence = DIR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!(
            "zerostack-read-grant-{}-{sequence}",
            std::process::id()
        ));
        let root = base.join("workspace");
        let external = base.join("external");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&external).unwrap();
        let root = root.canonicalize().unwrap();
        let external = external.canonicalize().unwrap();
        fs::write(root.join("inside.txt"), "inside").unwrap();
        fs::write(external.join("granted.txt"), "granted").unwrap();
        Self { root, external }
    }

    fn in_root_file(&self) -> PathBuf {
        self.root.join("inside.txt")
    }

    fn external_file(&self) -> PathBuf {
        self.external.join("granted.txt")
    }

    fn other_external_file(&self) -> PathBuf {
        let path = self.external.join("other.txt");
        fs::write(&path, "other").unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(
            self.root
                .parent()
                .expect("fixture base has a parent")
                .to_path_buf(),
        );
    }
}

fn mint(
    active: &mut Vec<SessionReadGrant>,
    root: &Path,
    path: &str,
    now: u64,
) -> SessionReadGrant {
    mint_read_grant(active, root, "test-session", 1, 1, 7, path, now).unwrap()
}

fn canonical_str(path: &Path) -> String {
    path.canonicalize().unwrap().to_string_lossy().into_owned()
}

#[test]
fn mint_binds_exactly_one_canonical_file_outside_root() {
    let fixture = Fixture::new();
    let mut active = Vec::new();
    let now = 1_000_000;
    let grant = mint(&mut active, &fixture.root, &canonical_str(&fixture.external_file()), now);

    assert_eq!(grant.schema, SESSION_READ_GRANT_SCHEMA);
    assert_eq!(grant.operation, "fs.read");
    assert_eq!(grant.generation, 1);
    assert_eq!(grant.request_id, 1);
    assert_eq!(grant.root, fixture.root.to_string_lossy());
    assert_eq!(grant.canonical_path, canonical_str(&fixture.external_file()));
    assert_eq!(grant.issued_at_unix_ms, now);
    assert_eq!(
        grant.expires_at_unix_ms,
        now.saturating_add(MAX_SESSION_READ_GRANT_LIFETIME_MS)
    );
    assert!(grant.grant_id.starts_with("read-grant-test-session-r1-"));
    assert_eq!(active.len(), 1);
}

#[test]
fn mint_rejects_in_root_missing_and_directory_targets() {
    let fixture = Fixture::new();
    let mut active = Vec::new();

    let error = mint_read_grant(
        &mut active,
        &fixture.root,
        "test-session",
        1,
        1,
        1,
        &canonical_str(&fixture.in_root_file()),
        1_000_000,
    )
    .unwrap_err();
    assert!(error.contains("inside the session root"), "{error}");

    let missing = fixture.external.join("missing.txt");
    let error = mint_read_grant(
        &mut active,
        &fixture.root,
        "test-session",
        1,
        1,
        2,
        &missing.to_string_lossy(),
        1_000_000,
    )
    .unwrap_err();
    assert!(error.contains("does not resolve"), "{error}");

    let error = mint_read_grant(
        &mut active,
        &fixture.root,
        "test-session",
        1,
        1,
        3,
        &fixture.external.to_string_lossy(),
        1_000_000,
    )
    .unwrap_err();
    assert!(error.contains("not a regular file"), "{error}");

    let error = mint_read_grant(
        &mut active,
        &fixture.root,
        "test-session",
        1,
        1,
        4,
        "relative/path.txt",
        1_000_000,
    )
    .unwrap_err();
    assert!(error.contains("requires an absolute path"), "{error}");

    assert!(active.is_empty());
}

#[cfg(unix)]
#[test]
fn mint_follows_symlink_to_canonical_target() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let mut active = Vec::new();
    let link = fixture.external.join("link.txt");
    symlink(fixture.external_file(), &link).unwrap();

    let grant = mint(&mut active, &fixture.root, &canonical_str(&link), 1_000_000);
    // The grant binds the RESOLVED identity, not the link spelling.
    assert_eq!(grant.canonical_path, canonical_str(&fixture.external_file()));
}

#[test]
fn mint_enforces_ledger_bound() {
    let fixture = Fixture::new();
    let mut active = Vec::new();
    let path = canonical_str(&fixture.external_file());
    for index in 0..MAX_SESSION_READ_GRANTS {
        mint_read_grant(
            &mut active,
            &fixture.root,
            "test-session",
            1,
            1,
            index as u64,
            &path,
            1_000_000,
        )
        .unwrap();
    }
    assert_eq!(active.len(), MAX_SESSION_READ_GRANTS);
    let error = mint_read_grant(
        &mut active,
        &fixture.root,
        "test-session",
        1,
        1,
        MAX_SESSION_READ_GRANTS as u64,
        &path,
        1_000_000,
    )
    .unwrap_err();
    assert!(error.contains("ledger is full"), "{error}");
}

#[test]
fn take_consumes_exact_canonical_match_once() {
    let fixture = Fixture::new();
    let mut active = Vec::new();
    mint(&mut active, &fixture.root, &canonical_str(&fixture.external_file()), 1_000_000);

    let entries = take_read_grants(
        &mut active,
        &fixture.root,
        "fs.read",
        &[canonical_str(&fixture.external_file())],
        1_000_001,
    )
    .unwrap();
    assert_eq!(entries.len(), 1);
    let grant = entries[0].grant.as_ref().expect("external path is granted");
    assert_eq!(grant.canonical_path, fixture.external_file().canonicalize().unwrap());
    assert!(active.is_empty(), "grant consumed once");

    // A second take of the same path has no active grant left.
    let error = take_read_grants(
        &mut active,
        &fixture.root,
        "fs.read",
        &[canonical_str(&fixture.external_file())],
        1_000_002,
    )
    .unwrap_err();
    assert!(error.contains("no active grant matches"), "{error}");
}

#[test]
fn take_fails_closed_on_substitution() {
    let fixture = Fixture::new();
    let mut active = Vec::new();
    mint(&mut active, &fixture.root, &canonical_str(&fixture.external_file()), 1_000_000);

    // Reading a different file with the same grant ledger fails closed.
    let other = canonical_str(&fixture.other_external_file());
    let error = take_read_grants(&mut active, &fixture.root, "fs.read", &[other], 1_000_001).unwrap_err();
    assert!(error.contains("no active grant matches"), "{error}");

    // The granted file's grant is still available (all-or-nothing).
    let entries = take_read_grants(
        &mut active,
        &fixture.root,
        "fs.read",
        &[canonical_str(&fixture.external_file())],
        1_000_002,
    )
    .unwrap();
    assert!(entries[0].grant.is_some());
}

#[test]
fn take_is_all_or_nothing() {
    let fixture = Fixture::new();
    let mut active = Vec::new();
    let first = canonical_str(&fixture.external_file());
    let second = canonical_str(&fixture.other_external_file());
    mint(&mut active, &fixture.root, &first, 1_000_000);

    // One granted, one not: the whole multiRead is rejected and NOTHING is
    // consumed.
    let error = take_read_grants(&mut active, &fixture.root, "fs.multiRead", &[first.clone(), second], 1_000_001)
        .unwrap_err();
    assert!(error.contains("no active grant matches"), "{error}");
    assert_eq!(active.len(), 1, "no grant consumed on partial match");

    let entries = take_read_grants(&mut active, &fixture.root, "fs.read", &[first], 1_000_002).unwrap();
    assert!(entries[0].grant.is_some());
}

#[test]
fn take_rejects_repeated_granted_path_without_separate_grant() {
    let fixture = Fixture::new();
    let mut active = Vec::new();
    let path = canonical_str(&fixture.external_file());
    mint(&mut active, &fixture.root, &path, 1_000_000);

    let error = take_read_grants(
        &mut active,
        &fixture.root,
        "fs.multiRead",
        &[path.clone(), path.clone()],
        1_000_001,
    )
    .unwrap_err();
    assert!(error.contains("one read grant per occurrence"), "{error}");
    assert_eq!(active.len(), 1, "nothing consumed on duplicate rejection");

    // Two separate grants for the same canonical file authorize both reads.
    mint(&mut active, &fixture.root, &path, 1_000_002);
    let entries =
        take_read_grants(&mut active, &fixture.root, "fs.multiRead", &[path.clone(), path], 1_000_003).unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries[0].grant.is_some() && entries[1].grant.is_some());
}

#[test]
fn take_requires_no_grant_inside_root() {
    let fixture = Fixture::new();
    let mut active = Vec::new();
    let inside = canonical_str(&fixture.in_root_file());

    let entries = take_read_grants(&mut active, &fixture.root, "fs.read", &[inside], 1_000_000).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].grant.is_none(), "in-root path needs no grant");
}

#[test]
fn take_prunes_expired_grants() {
    let fixture = Fixture::new();
    let mut active = Vec::new();
    mint(&mut active, &fixture.root, &canonical_str(&fixture.external_file()), 1_000_000);

    let error = take_read_grants(
        &mut active,
        &fixture.root,
        "fs.read",
        &[canonical_str(&fixture.external_file())],
        1_000_000 + MAX_SESSION_READ_GRANT_LIFETIME_MS + 1,
    )
    .unwrap_err();
    assert!(error.contains("no active grant matches"), "{error}");
}

#[test]
fn plan_roots_single_granted_read_at_file_parent() {
    let fixture = Fixture::new();
    let path = canonical_str(&fixture.external_file());
    let mut active = Vec::new();
    let grant = mint(&mut active, &fixture.root, &path, 1_000_000);
    let bindings = vec![GrantedReadFile {
        grant_id: grant.grant_id,
        canonical_path: grant.canonical_path.clone().into(),
        issued_at_unix_ms: grant.issued_at_unix_ms,
        expires_at_unix_ms: grant.expires_at_unix_ms,
    }];

    let plan = plan_granted_read(
        Some(&fixture.root),
        "fs.read",
        &[path.clone()],
        &bindings,
        1_000_001,
    )
    .unwrap()
    .expect("absolute external read plans");
    assert_eq!(plan.root, fixture.external);
    assert_eq!(plan.rewrites.len(), 1);
    assert_eq!(plan.rewrites[0].requested_path, path);
    assert_eq!(plan.rewrites[0].relative_path, "granted.txt");
    assert!(plan.rewrites[0].grant.is_some());

    // The plan re-verifies fresh: a binding for a different file fails.
    let other = canonical_str(&fixture.other_external_file());
    let error = plan_granted_read(
        Some(&fixture.root),
        "fs.read",
        &[other],
        &bindings,
        1_000_002,
    )
    .unwrap_err();
    assert!(error.contains("no matching read grant"), "{error}");
}

#[test]
fn plan_rejects_expired_and_missing_bindings() {
    let fixture = Fixture::new();
    let path = canonical_str(&fixture.external_file());
    let expired = vec![GrantedReadFile {
        grant_id: "read-grant-test-session-r1-1".into(),
        canonical_path: fixture.external_file().canonicalize().unwrap(),
        issued_at_unix_ms: 1_000_000,
        expires_at_unix_ms: 1_000_000 + MAX_SESSION_READ_GRANT_LIFETIME_MS,
    }];
    let error = plan_granted_read(
        Some(&fixture.root),
        "fs.read",
        &[path.clone()],
        &expired,
        1_000_000 + MAX_SESSION_READ_GRANT_LIFETIME_MS + 1,
    )
    .unwrap_err();
    assert!(error.contains("expired"), "{error}");

    // No binding at all: fail closed.
    let error = plan_granted_read(Some(&fixture.root), "fs.read", &[path], &[], 1_000_001)
        .unwrap_err();
    assert!(error.contains("no matching read grant"), "{error}");
}

#[test]
fn plan_uses_primary_root_for_in_root_absolute_reads() {
    let fixture = Fixture::new();
    let inside = canonical_str(&fixture.in_root_file());
    let plan = plan_granted_read(Some(&fixture.root), "fs.read", &[inside], &[], 1_000_000)
        .unwrap()
        .expect("in-root absolute read plans");
    assert_eq!(plan.root, fixture.root);
    assert_eq!(plan.rewrites[0].relative_path, "inside.txt");
    assert!(plan.rewrites[0].grant.is_none());
}

#[test]
fn plan_rejects_multi_read_mixing_in_root_and_granted() {
    let fixture = Fixture::new();
    let mut active = Vec::new();
    let external = canonical_str(&fixture.external_file());
    let grant = mint(&mut active, &fixture.root, &external, 1_000_000);
    let bindings = vec![GrantedReadFile {
        grant_id: grant.grant_id,
        canonical_path: grant.canonical_path.clone().into(),
        issued_at_unix_ms: grant.issued_at_unix_ms,
        expires_at_unix_ms: grant.expires_at_unix_ms,
    }];

    let error = plan_granted_read(
        Some(&fixture.root),
        "fs.multiRead",
        &[canonical_str(&fixture.in_root_file()), external],
        &bindings,
        1_000_001,
    )
    .unwrap_err();
    assert!(error.contains("cannot mix"), "{error}");
}

#[test]
fn plan_multi_read_requires_single_shared_parent() {
    let fixture = Fixture::new();
    let mut active = Vec::new();
    let first = canonical_str(&fixture.external_file());
    let second = canonical_str(&fixture.other_external_file());
    let grant_one = mint(&mut active, &fixture.root, &first, 1_000_000);
    let grant_two = mint(&mut active, &fixture.root, &second, 1_000_000);
    let bindings = vec![
        GrantedReadFile {
            grant_id: grant_one.grant_id,
            canonical_path: grant_one.canonical_path.clone().into(),
            issued_at_unix_ms: grant_one.issued_at_unix_ms,
            expires_at_unix_ms: grant_one.expires_at_unix_ms,
        },
        GrantedReadFile {
            grant_id: grant_two.grant_id,
            canonical_path: grant_two.canonical_path.clone().into(),
            issued_at_unix_ms: grant_two.issued_at_unix_ms,
            expires_at_unix_ms: grant_two.expires_at_unix_ms,
        },
    ];

    // Both files share the external parent: one temporary root, exactly the
    // granted file names rewritten.
    let plan = plan_granted_read(
        Some(&fixture.root),
        "fs.multiRead",
        &[first.clone(), second.clone()],
        &bindings,
        1_000_001,
    )
    .unwrap()
    .expect("same-parent multiRead plans");
    assert_eq!(plan.root, fixture.external);
    assert_eq!(plan.rewrites[0].relative_path, "granted.txt");
    assert_eq!(plan.rewrites[1].relative_path, "other.txt");

    // Files in different directories are rejected: the temporary root must
    // never widen beyond one granted file's own parent.
    let sub = fixture.external.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let nested = sub.join("nested.txt");
    fs::write(&nested, "nested").unwrap();
    let nested_path = canonical_str(&nested);
    let grant_three = mint(&mut active, &fixture.root, &nested_path, 1_000_002);
    let mut bindings = bindings;
    bindings.push(GrantedReadFile {
        grant_id: grant_three.grant_id,
        canonical_path: grant_three.canonical_path.clone().into(),
        issued_at_unix_ms: grant_three.issued_at_unix_ms,
        expires_at_unix_ms: grant_three.expires_at_unix_ms,
    });
    let error = plan_granted_read(
        Some(&fixture.root),
        "fs.multiRead",
        &[first, second, nested_path],
        &bindings,
        1_000_003,
    )
    .unwrap_err();
    assert!(error.contains("share one parent directory"), "{error}");
}

#[cfg(unix)]
#[test]
fn post_verify_fails_closed_on_swap_during_read() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let path = canonical_str(&fixture.external_file());
    let mut active = Vec::new();
    let grant = mint(&mut active, &fixture.root, &path, 1_000_000);
    let bindings = vec![GrantedReadFile {
        grant_id: grant.grant_id,
        canonical_path: grant.canonical_path.clone().into(),
        issued_at_unix_ms: grant.issued_at_unix_ms,
        expires_at_unix_ms: grant.expires_at_unix_ms,
    }];
    let plan = plan_granted_read(
        Some(&fixture.root),
        "fs.read",
        &[path],
        &bindings,
        1_000_001,
    )
    .unwrap()
    .unwrap();

    // Stable file: post-verification passes.
    post_verify_read(&plan).unwrap();

    // Swap the granted file for a symlink to a different file: the read
    // result must be discarded.
    let granted = fixture.external.join("granted.txt");
    fs::remove_file(&granted).unwrap();
    symlink(fixture.other_external_file(), &granted).unwrap();
    let error = post_verify_read(&plan).unwrap_err();
    assert!(error.contains("substituted during read"), "{error}");
}

#[test]
fn absolute_read_paths_extraction_and_mixing_rule() {
    let args = json!({"path": "/tmp/x.txt"});
    assert_eq!(
        absolute_read_paths("fs.read", &args).unwrap(),
        vec!["/tmp/x.txt".to_string()]
    );
    let args = json!({"paths": ["/tmp/x.txt", "/tmp/y.txt"]});
    assert_eq!(
        absolute_read_paths("fs.multiRead", &args).unwrap(),
        vec!["/tmp/x.txt".to_string(), "/tmp/y.txt".to_string()]
    );
    // Relative-only calls need no grants.
    assert!(absolute_read_paths("fs.read", &json!({"path": "rel.txt"})).unwrap().is_empty());
    // Mixing relative and absolute fails closed.
    let args = json!({"paths": ["rel.txt", "/tmp/x.txt"]});
    let error = absolute_read_paths("fs.multiRead", &args).unwrap_err();
    assert!(error.contains("cannot mix"), "{error}");
    // Non-read operations are untouched.
    assert!(absolute_read_paths("fs.ls", &json!({"path": "/tmp"})).unwrap().is_empty());
}

#[test]
fn lower_read_grant_surface() {
    let (engine, op, args) =
        lower("fs", "read_grant", json!({"path": "/tmp/x.txt"})).unwrap();
    assert_eq!(engine.as_str(), "fszero");
    assert_eq!(op, "fs.readGrant");
    assert_eq!(args, json!({"path": "/tmp/x.txt"}));

    let (engine, op, args) = lower(
        "fs",
        "compound",
        json!({"name": "readGrant", "args": {"path": "/tmp/x.txt"}}),
    )
    .unwrap();
    assert_eq!(engine.as_str(), "fszero");
    assert_eq!(op, "fs.readGrant");
    assert_eq!(args, json!({"path": "/tmp/x.txt"}));

    let (engine, op, args) = lower(
        "fs",
        "compound",
        json!(["readGrant", {"path": "/tmp/x.txt"}]),
    )
    .unwrap();
    assert_eq!(engine.as_str(), "fszero");
    assert_eq!(op, "fs.readGrant");
    assert_eq!(args, json!({"path": "/tmp/x.txt"}));
}
