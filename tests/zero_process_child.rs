//! Focused tests for hub-owned verified child identity and tree ownership. Mutant map: deleting a
//! guard makes the named test(s) fail.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use zero_process::ProcessIdentity;
use zero_process::VerifiedChild;

// Self-spawn fixtures (current test binary)

const FIXTURE_ENV: &str = "ZERO_PROCESS_CHILD_FIXTURE";
const ROOT_PID_ENV: &str = "ZERO_PROCESS_ROOT_PID_FILE";
const LEAF_PID_ENV: &str = "ZERO_PROCESS_LEAF_PID_FILE";

fn test_binary() -> PathBuf {
    std::env::current_exe().expect("locate the integration-test binary")
}

/// Build a Command that re-runs this test binary in fixture mode. Fixtures
/// write no stdout unless the test pipes it.
fn fixture_command(role: &str) -> Command {
    let mut command = Command::new(test_binary());
    command
        .env(FIXTURE_ENV, role)
        .arg("--exact")
        .arg("fixture_runner")
        .arg("--test-threads")
        .arg("1")
        .arg("--nocapture")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Fixture entry point for self-spawned children.
#[test]
fn fixture_runner() {
    let Ok(role) = std::env::var(FIXTURE_ENV) else {
        return;
    };
    match role.as_str() {
        "leaf" => {
            report_own_pid();
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        "tree-root" => {
            let mut command = fixture_command("leaf");
            if let Ok(pid_file) = std::env::var(LEAF_PID_ENV) {
                command.env(LEAF_PID_ENV, pid_file);
            }
            let (_owned, _stdin, _stdout) =
                VerifiedChild::spawn_tree(command, "tree-owner", 1).expect("root spawn_tree leaf");
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        "owner" => {
            // This fixture owns the spawned tree, binding root lifetime to this process.
            // Killing the fixture must also terminate the root and descendant leaf.
            let mut command = fixture_command("tree-root");
            if let Ok(pid_file) = std::env::var(LEAF_PID_ENV) {
                command.env(LEAF_PID_ENV, pid_file);
            }
            let (owned, _stdin, _stdout) = VerifiedChild::spawn_tree(command, "owner-session", 1)
                .expect("owner fixture spawn_tree");
            if let Ok(pid_file) = std::env::var(ROOT_PID_ENV) {
                std::fs::write(&pid_file, owned.child_id().to_string())
                    .expect("write root pid file");
            }
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        other => panic!("unknown fixture role: {other:?}"),
    }
}

/// The leaf writes its pid to `ZERO_PROCESS_LEAF_PID_FILE` after finishing role setup, so
/// the parent only proceeds once the leaf is fully ready and inside the tree. A file, not
/// stdout: libtest's no-newline progress prefix can merge with child stdout on a shared pipe.
fn report_own_pid() {
    if let Ok(path) = std::env::var(LEAF_PID_ENV) {
        std::fs::write(&path, std::process::id().to_string()).expect("write leaf pid file");
    }
}

/// Unique per-process, per-test temp path for pid-file plumbing.
fn unique_temp_path(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "zerostack-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Wait for a pid file (written only after the process finished role setup and
/// is inside the tree). Returns the pid.
fn wait_for_pid_file(pid_file: &std::path::Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(contents) = std::fs::read_to_string(pid_file) {
            if let Ok(pid) = contents.trim().parse::<u32>() {
                return pid;
            }
        }
        if Instant::now() >= deadline {
            panic!("process must report its pid via {}", pid_file.display());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Identity-bound liveness: the process is gone only when the captured identity no
/// longer matches a live process at the pid. A recycled pid carries a different
/// start identity, so it can never be reported as the same process (no false green).
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn process_is_gone(identity: &ProcessIdentity) -> bool {
    !identity.is_live().unwrap_or(false)
}

/// Poll up to 15s for the captured process to be gone, panicking otherwise.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn assert_process_gone(pid: u32, identity: &ProcessIdentity, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if process_is_gone(identity) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{what} {pid} stayed live after owner SIGKILL");
}

#[cfg(unix)]
#[test]
fn wait_for_exit_reports_an_exited_tree_root() {
    let command = Command::new("true");
    let (child, _, _) = VerifiedChild::spawn_tree(command, "wait-exited", 1).expect("spawn true");
    assert!(
        child.wait_for_exit(Duration::from_secs(2)),
        "an exited waitable root must not be mistaken for a live process"
    );
    child
        .wait(
            "wait-exited",
            1,
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect("settle exited tree");
}

#[test]
fn wait_for_exit_does_not_report_a_live_child_as_exited() {
    let mut command = Command::new("sleep");
    command.arg("2");
    let (child, _, _) = VerifiedChild::spawn_tree(command, "wait-timeout", 1).expect("spawn sleep");
    let started = Instant::now();
    assert!(!child.wait_for_exit(Duration::from_millis(50)));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "bounded wait must return before the child exits"
    );
}

/// Linux/Darwin: SIGKILL of the spawn_tree owner must reap the tree root AND the descendant leaf --
/// a grandchild that is not a direct child of the owner.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn spawn_tree_owner_sigkill_reaps_descendant_leaf() {
    let root_pid_file = unique_temp_path("owner-kill-root");
    let leaf_pid_file = unique_temp_path("owner-kill-leaf");
    let mut owner_cmd = fixture_command("owner");
    owner_cmd
        .env(ROOT_PID_ENV, &root_pid_file)
        .env(LEAF_PID_ENV, &leaf_pid_file)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut owner = owner_cmd.spawn().expect("spawn owner fixture");
    // The leaf pid file exists only after the whole chain exec'd: the root is
    // past its pre-exec binding and the leaf is fully set up inside the tree.
    let root_pid = wait_for_pid_file(&root_pid_file);
    let leaf_pid = wait_for_pid_file(&leaf_pid_file);
    let _ = std::fs::remove_file(&root_pid_file);
    let _ = std::fs::remove_file(&leaf_pid_file);

    let root_identity = ProcessIdentity::capture(root_pid).expect("capture root identity");
    let leaf_identity = ProcessIdentity::capture(leaf_pid).expect("capture leaf identity");
    assert!(
        root_identity.is_live().unwrap_or(false),
        "root {root_pid} must be live before owner SIGKILL"
    );
    assert!(
        leaf_identity.is_live().unwrap_or(false),
        "descendant leaf {leaf_pid} must be live before owner SIGKILL"
    );

    // SAFETY: the owner is an unreaped direct child with a pinned pid. The root
    // and leaf identities are bound through the same ownership chain.
    let rc = unsafe { libc::kill(owner.id() as libc::pid_t, libc::SIGKILL) };
    assert_eq!(rc, 0, "SIGKILL owner");
    let _ = owner.wait();

    assert_process_gone(root_pid, &root_identity, "spawn_tree root");
    assert_process_gone(leaf_pid, &leaf_identity, "descendant leaf");
}
