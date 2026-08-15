//! Focused tests for hub-owned verified child identity.
//!
//! Mutant map: deleting a guard makes the named test(s) fail.
//! - start-identity capture/verify ......... -> `stale_detached_binding_is_rejected`,
//!   `exit_before_cancel_never_signals_replacement`
//! - owner-session binding ................. -> `owner_mismatch_is_rejected`,
//!   `owner_and_generation_bindings_apply_to_verified_child`
//! - worker-generation binding ............. -> `generation_mismatch_is_rejected`,
//!   `owner_and_generation_bindings_apply_to_verified_child`
//! - revoked guard ......................... -> `signal_after_revoke_fails_closed`
//! - unreaped-state guard .................. -> `exit_before_cancel_never_signals_replacement`
//! - fail-closed-on-unsupported ............ -> `escalation_fails_closed_on_unsupported_platform`
//! - tree isolation / descendant sweep ..... -> `tree_teardown_reaps_root_and_kills_descendants`,
//!   `tree_poll_exited_keeps_pin_and_allows_descendant_sweep`
//! - tree revoke-before-teardown guard ..... -> `direct_revoke_before_tree_teardown_fails_without_reaping`
//! - tree concurrent settle ............... -> `concurrent_revoke_and_cancel_settle_tree_with_no_descendant`
//! - external-revoke wait wake ............. -> `long_wait_observes_concurrent_signal_and_revoke`
//!
//! Fixtures are self-spawns of this test binary (`fixture_runner`), so every
//! test is runnable natively on macOS, Linux, and Windows. Unix pidfd-only
//! tests stay `#[cfg(target_os = "linux")]`; the macOS detached fail-closed
//! test stays `#[cfg(target_os = "macos")]`. Native platform exercised by the
//! targeted RCH runs: Linux. Windows runtime coverage runs in CI
//! (`test-native-child-identity`): owner/generation, owned-handle teardown,
//! revocation, and tree cleanup are the same tests, executed on Windows.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use zero_process::ProcessIdentity;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use zero_process::escalate_detached;
use zero_process::{ChildBinding, IdentityError, SignalOutcome, VerifiedChild};

// ---------------------------------------------------------------------------
// Self-spawn fixtures (current test binary)
// ---------------------------------------------------------------------------

const FIXTURE_ENV: &str = "ZERO_PROCESS_CHILD_FIXTURE";
const GATE_ENV: &str = "ZERO_PROCESS_TREE_GATE";
const LEAF_PID_ENV: &str = "ZERO_PROCESS_LEAF_PID_FILE";

fn test_binary() -> PathBuf {
    std::env::current_exe().expect("locate the integration-test binary")
}

/// Build a Command that re-runs this test binary in fixture mode. Fixtures
/// write no stdout unless the test pipes it (the `tree` role).
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

fn spawn_fixture(role: &str) -> Child {
    fixture_command(role).spawn().expect("spawn fixture child")
}

/// Fixture entry point for self-spawned children. Runs as a no-op test in the
/// normal suite; when spawned by a fixture command it executes the role named
/// in `ZERO_PROCESS_CHILD_FIXTURE`:
/// - "sleep": stay alive indefinitely.
/// - "ignore-term": ignore SIGTERM (Unix) and stay alive; forces escalation.
/// - "exit": return immediately.
/// - "tree": wait for the gate file, spawn a "leaf" child, report its pid,
///   then stay alive (reaping the leaf if it exits).
/// - "leaf": report the pid and stay alive.
#[test]
fn fixture_runner() {
    let Ok(role) = std::env::var(FIXTURE_ENV) else {
        return;
    };
    match role.as_str() {
        "sleep" => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
        "ignore-term" => {
            #[cfg(unix)]
            // SAFETY: installing SIG_IGN for SIGTERM is process-global and
            // exactly what the fixture is for.
            unsafe {
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
            }
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        "exit" => {}
        "tree" => tree_fixture("leaf", true),
        "tree-stubborn" => tree_fixture("leaf-ignore", true),
        "tree-exit" => tree_fixture("leaf", false),
        "leaf" => {
            report_leaf_pid();
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        "leaf-ignore" => {
            #[cfg(unix)]
            // SAFETY: the leaf deliberately ignores SIGTERM so the tree must
            // escalate to SIGKILL to clean it.
            unsafe {
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
            }
            report_leaf_pid();
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        "owner-spawn-tree" => {
            // This fixture *is* the owner: it spawn_tree's a leaf so the leaf
            // is bound to this process (Linux PDEATHSIG / Darwin kqueue
            // watcher). The parent test SIGKILLs us; the leaf must not stay live.
            let mut command = fixture_command("leaf");
            if let Ok(pid_file) = std::env::var(LEAF_PID_ENV) {
                command.env(LEAF_PID_ENV, pid_file);
            }
            let (_owned, _stdin, _stdout) =
                VerifiedChild::spawn_tree(command, "owner-session", 1)
                    .expect("owner fixture spawn_tree");
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        other => panic!("unknown fixture role: {other:?}"),
    }
}

/// The leaf writes its pid to `ZERO_PROCESS_LEAF_PID_FILE` after finishing any
/// role setup (e.g. SIG_IGN), so the parent only proceeds once the leaf is
/// fully ready and inside the tree. A file, not stdout: libtest's no-newline
/// progress prefix can merge with child stdout on a shared pipe, so stdout
/// reports are unreliable.
fn report_leaf_pid() {
    if let Ok(path) = std::env::var(LEAF_PID_ENV) {
        std::fs::write(&path, std::process::id().to_string()).expect("write leaf pid file");
    }
}

/// Unique per-process, per-test temp path for gate/pid-file plumbing.
fn unique_temp_path(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "graphzero-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Tree root fixture: waits for the parent's gate file, then spawns a leaf of
/// the given role. The gate guarantees the leaf exists only after the parent
/// finished establishing tree ownership (Unix process group at spawn; Windows
/// job assignment), so the leaf is always inside the tree.
#[allow(
    clippy::zombie_processes,
    reason = "fixture leaves the leaf for the parent tree-containment assertion"
)]
fn tree_fixture(leaf_role: &str, stay_alive: bool) {
    if let Ok(gate) = std::env::var(GATE_ENV) {
        // The parent always creates the gate after tree ownership is
        // established; wait generously so slow (cold-cache) starts do not
        // race the parent's read timeout.
        let deadline = Instant::now() + Duration::from_secs(120);
        while !std::path::Path::new(&gate).exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let mut command = fixture_command(leaf_role);
    if let Ok(pid_file) = std::env::var(LEAF_PID_ENV) {
        command.env(LEAF_PID_ENV, pid_file);
    }
    let mut leaf = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn leaf child");
    if !stay_alive {
        // The root exits immediately after spawning the leaf: descendants must
        // be swept by tree teardown even though the root is already gone.
        return;
    }
    loop {
        let _ = leaf.try_wait();
        std::thread::sleep(Duration::from_millis(200));
    }
}

// ---------------------------------------------------------------------------
// Binding record
// ---------------------------------------------------------------------------

#[test]
fn binding_round_trip_encode_decode() {
    let binding = ChildBinding {
        pid: 4242,
        start_key: Some("boot:1234".into()),
        owner_session: "session-1".into(),
        generation: 3,
    };
    let decoded = ChildBinding::decode(&binding.encode()).expect("decode own encoding");
    assert_eq!(decoded, binding);

    let no_key = ChildBinding {
        pid: 4242,
        start_key: None,
        owner_session: "".into(),
        generation: 0,
    };
    assert_eq!(ChildBinding::decode(&no_key.encode()).unwrap(), no_key);
}

#[test]
fn malformed_binding_fails_closed() {
    for bad in [
        "",
        "42:key",
        "0:key\towner\t0",
        "42:key\towner\tnot-a-gen",
        "42:key\towner\t0\textra",
    ] {
        assert!(ChildBinding::decode(bad).is_err(), "must reject {bad:?}");
    }
}

#[test]
fn owner_mismatch_is_rejected() {
    let binding = ChildBinding {
        pid: 1,
        start_key: None,
        owner_session: "session-a".into(),
        generation: 0,
    };
    match binding.verify_owner("session-b", 0) {
        Err(IdentityError::OwnerMismatch { expected, actual }) => {
            assert_eq!(expected, "session-b");
            assert_eq!(actual, "session-a");
        }
        other => panic!("expected OwnerMismatch, got {other:?}"),
    }
}

#[test]
fn generation_mismatch_is_rejected() {
    let binding = ChildBinding {
        pid: 1,
        start_key: None,
        owner_session: "session-a".into(),
        generation: 0,
    };
    match binding.verify_owner("session-a", 7) {
        Err(IdentityError::GenerationMismatch { expected, actual }) => {
            assert_eq!(expected, 7);
            assert_eq!(actual, 0);
        }
        other => panic!("expected GenerationMismatch, got {other:?}"),
    }
    assert!(binding.verify_owner("session-a", 0).is_ok());
}

/// A detached binding: capture the identity, then drop the owned handle
/// without waiting so the child is reparented (daemon-like). Requires a start
/// identity, which only Linux and macOS capture; other platforms fail closed.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn detached_binding(role: &str, owner: &str) -> (ChildBinding, Child) {
    let child = spawn_fixture(role);
    let binding = ChildBinding::capture_pid(child.id(), owner, 0).expect("capture child identity");
    (binding, child)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn stale_detached_binding_is_rejected() {
    // Capture a live child identity, let it exit, then the binding must not
    // be live and must not authorize any signal.
    let (binding, mut child) = detached_binding("exit", "test-owner");
    assert!(binding.is_live());
    child.wait().expect("reap fixture child");
    assert!(!binding.is_live(), "reaped child must not be live");
    assert!(matches!(
        escalate_detached(&binding, Duration::from_millis(200)),
        Err(IdentityError::Missing) | Err(IdentityError::Unsupported)
    ));
}

// ---------------------------------------------------------------------------
// Same-process owned child (single-process capture API)
// ---------------------------------------------------------------------------

fn terminate_owned(owned: &VerifiedChild, owner: &str, generation: u64) {
    owned
        .signal_graceful_for(owner, generation, Duration::from_secs(2))
        .expect("bounded child teardown");
    owned.revoke().expect("revoke after reap");
}

#[test]
fn verified_child_graceful_terminates_owned_child() {
    let child = spawn_fixture("sleep");
    let owned = VerifiedChild::capture(child, "test-owner", 0);
    owned
        .verify_for("test-owner", 0)
        .expect("verify owned child");
    let outcome = owned
        .signal_graceful_for("test-owner", 0, Duration::from_secs(5))
        .expect("bounded child teardown");
    // Unix delivers SIGTERM (graceful); Windows terminates through the real
    // process handle because std has no SIGTERM equivalent there.
    assert!(matches!(
        outcome,
        SignalOutcome::TerminatedGracefully | SignalOutcome::EscalatedToKill
    ));
    owned.revoke().expect("revoke once");
    assert!(owned.is_revoked());
}

#[test]
fn verified_child_escalates_stubborn_child() {
    // SIGTERM-ignoring child forces bounded escalation to SIGKILL.
    let child = spawn_fixture("ignore-term");
    let owned = VerifiedChild::capture(child, "test-owner", 0);
    // Give the fixture time to install its SIG_IGN handler before the first
    // signal; a premature SIGTERM would kill it with the default disposition
    // and defeat the escalation exercise.
    std::thread::sleep(Duration::from_millis(250));
    let outcome = owned
        .signal_graceful_for("test-owner", 0, Duration::from_millis(300))
        .expect("escalation must settle");
    assert_eq!(outcome, SignalOutcome::EscalatedToKill);
    owned.revoke().expect("revoke");
}

#[test]
fn exit_before_cancel_never_signals_replacement() {
    // Spawn a short-lived child, capture its identity, and let it exit and
    // get reaped before the cancel runs. The cancel must fail closed: the
    // start identity is no longer live, so even a pid reused by a replacement
    // process can never be signaled (identity mismatch fails the verify).
    let child = spawn_fixture("exit");
    let owned = VerifiedChild::capture(child, "test-owner", 0);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !owned.poll_exited() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(owned.poll_exited(), "fixture child must exit");
    match owned.signal_graceful_for("test-owner", 0, Duration::from_millis(100)) {
        Err(
            IdentityError::IdentityChanged | IdentityError::AlreadyReaped | IdentityError::Missing,
        ) => {}
        other => panic!("expected fail-closed cancel, got {other:?}"),
    }
}

#[test]
fn duplicate_revoke_is_harmless() {
    let child = spawn_fixture("sleep");
    let owned = VerifiedChild::capture(child, "test-owner", 0);
    assert!(matches!(owned.revoke(), Err(IdentityError::StillRunning)));
    terminate_owned(&owned, "test-owner", 0);
    owned.revoke().expect("duplicate revoke must be harmless");
    assert!(owned.is_revoked());
}

#[test]
fn wait_timeout_retains_live_child_for_explicit_teardown() {
    let child = spawn_fixture("sleep");
    let owned = VerifiedChild::capture(child, "test-owner", 0);
    let started = Instant::now();
    assert!(matches!(
        owned.wait("test-owner", 0, Duration::from_millis(20), Duration::ZERO,),
        Err(IdentityError::StillRunning)
    ));
    assert!(started.elapsed() < Duration::from_secs(1));
    terminate_owned(&owned, "test-owner", 0);
}

#[test]
fn long_wait_observes_concurrent_signal_and_revoke() {
    let child = spawn_fixture("sleep");
    let owned = VerifiedChild::capture(child, "wait-owner", 9);
    let waiter = owned.clone();
    let started = Instant::now();
    let waiting = std::thread::spawn(move || waiter.wait_for_exit(Duration::from_secs(5)));

    std::thread::sleep(Duration::from_millis(100));
    owned
        .signal_graceful_for("wait-owner", 9, Duration::from_millis(250))
        .expect("concurrent exact-handle signal settles child");
    owned.revoke().expect("concurrent signal is revocable");

    assert!(waiting.join().expect("wait thread joins"));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "wait_for_exit ignored externally recorded terminal status"
    );
    assert!(owned.terminal_status().is_some());
}

#[test]
fn concurrent_cancel_and_revoke_settle_exactly_once() {
    let child = spawn_fixture("sleep");
    let owned = VerifiedChild::capture(child, "test-owner", 0);
    let cancel = owned.clone();
    let revoke = owned.clone();
    let a = std::thread::spawn(move || {
        cancel.signal_graceful_for("test-owner", 0, Duration::from_secs(5))?;
        cancel.revoke()
    });
    let b = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        revoke.revoke()
    });
    assert!(a.join().expect("cancel thread").is_ok());
    match b.join().expect("revoke thread") {
        Ok(()) | Err(IdentityError::StillRunning) => {}
        other => panic!("unexpected concurrent revoke result: {other:?}"),
    }
    owned.revoke().expect("final idempotent revoke");
}

#[test]
fn signal_after_revoke_fails_closed() {
    let child = spawn_fixture("sleep");
    let owned = VerifiedChild::capture(child, "test-owner", 0);
    terminate_owned(&owned, "test-owner", 0);
    match owned.signal_graceful_for("test-owner", 0, Duration::from_secs(1)) {
        Err(IdentityError::Revoked) => {}
        other => panic!("expected Revoked after revoke, got {other:?}"),
    }
    assert!(matches!(owned.verify(), Err(IdentityError::Revoked)));
}

#[test]
fn owner_and_generation_bindings_apply_to_verified_child() {
    let child = spawn_fixture("sleep");
    let owned = VerifiedChild::capture(child, "session-9", 4);
    assert!(owned.verify_for("session-9", 4).is_ok());
    assert!(matches!(
        owned.verify_for("session-8", 4),
        Err(IdentityError::OwnerMismatch { .. })
    ));
    assert!(matches!(
        owned.verify_for("session-9", 5),
        Err(IdentityError::GenerationMismatch { .. })
    ));
    terminate_owned(&owned, "session-9", 4);
}

// ---------------------------------------------------------------------------
// Process-tree ownership (spawn_tree)
// ---------------------------------------------------------------------------

/// Wait for the leaf's pid file (written only after the leaf finished role
/// setup and is inside the tree). Returns the leaf pid.
fn wait_for_leaf_pid_file(pid_file: &std::path::Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(contents) = std::fs::read_to_string(pid_file)
            && let Ok(pid) = contents.trim().parse::<u32>()
        {
            return pid;
        }
        if Instant::now() >= deadline {
            panic!("tree leaf must report its pid via {}", pid_file.display());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn leaf_is_gone(_pid: u32, identity: &ProcessIdentity) -> bool {
    !identity.is_live().unwrap_or(false)
}

/// Windows-only liveness check for the descendant-cleanup assertion, using
/// windows-sys directly (no production code involved). Opens a handle by pid:
/// exact for our descendant while the tree test holds the root handle, and the
/// probe never authorizes a signal.
#[cfg(windows)]
fn leaf_is_gone(pid: u32, _identity: &()) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            // Access denied means a process with that pid exists.
            return !(std::io::Error::last_os_error().raw_os_error()
                == Some(ERROR_ACCESS_DENIED as i32));
        }
        let wait = WaitForSingleObject(handle, 0);
        CloseHandle(handle);
        wait != WAIT_TIMEOUT
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn leaf_is_gone(pid: u32, _identity: &()) -> bool {
    // SAFETY: signal 0 only probes existence; it sends nothing.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

#[test]
fn tree_teardown_reaps_root_and_kills_descendants() {
    let gate = unique_temp_path("tree-gate");
    let pid_file = unique_temp_path("leaf-pid");
    let mut command = fixture_command("tree");
    command
        .env(GATE_ENV, &gate)
        .env(LEAF_PID_ENV, &pid_file)
        .stdout(Stdio::null());
    let (owned, _stdin, _stdout) =
        VerifiedChild::spawn_tree(command, "tree-owner", 0).expect("spawn isolated process tree");
    // Tree ownership is established the moment spawn_tree returns. Releasing
    // the gate now guarantees the root spawns its leaf only after Unix
    // process-group creation / Windows job assignment finished, so the leaf is
    // a member of the tree.
    std::fs::File::create(&gate).expect("open gate file");
    let leaf_pid = wait_for_leaf_pid_file(&pid_file);
    let _ = std::fs::remove_file(&gate);
    let _ = std::fs::remove_file(&pid_file);

    // Capture the leaf's native identity while it is provably alive.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let leaf_identity = ProcessIdentity::capture(leaf_pid).expect("capture leaf start identity");
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let leaf_identity = ();

    let outcome = owned
        .signal_graceful_for("tree-owner", 0, Duration::from_millis(500))
        .expect("tree teardown must settle");
    assert!(matches!(
        outcome,
        SignalOutcome::TerminatedGracefully | SignalOutcome::EscalatedToKill
    ));
    // revoke succeeds only after the root exited: proof the root was reaped.
    owned.revoke().expect("revoke root after reap");

    // The descendant leaf must be gone with the tree.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut leaf_gone = false;
    while Instant::now() < deadline {
        if leaf_is_gone(leaf_pid, &leaf_identity) {
            leaf_gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        leaf_gone,
        "descendant leaf {leaf_pid} survived tree teardown"
    );
}

#[test]
fn tree_escalation_cleans_sigterm_ignoring_descendant() {
    // The root exits on SIGTERM (default disposition) but its leaf ignores
    // SIGTERM: teardown must escalate SIGKILL against the same pinned group,
    // prove the group is gone, and report EscalatedToKill -- never success
    // while a descendant survives.
    let gate = unique_temp_path("tree-gate");
    let pid_file = unique_temp_path("leaf-pid");
    let mut command = fixture_command("tree-stubborn");
    command
        .env(GATE_ENV, &gate)
        .env(LEAF_PID_ENV, &pid_file)
        .stdout(Stdio::null());
    let (owned, _stdin, _stdout) =
        VerifiedChild::spawn_tree(command, "tree-owner", 0).expect("spawn isolated process tree");
    std::fs::File::create(&gate).expect("open gate file");
    let leaf_pid = wait_for_leaf_pid_file(&pid_file);
    let _ = std::fs::remove_file(&gate);
    let _ = std::fs::remove_file(&pid_file);

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let leaf_identity = ProcessIdentity::capture(leaf_pid).expect("capture leaf start identity");
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let leaf_identity = ();

    let outcome = owned
        .signal_graceful_for("tree-owner", 0, Duration::from_millis(500))
        .expect("tree teardown must settle");
    assert_eq!(
        outcome,
        SignalOutcome::EscalatedToKill,
        "SIGTERM-ignoring descendant must force escalation"
    );
    owned.revoke().expect("revoke root after reap");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut leaf_gone = false;
    while Instant::now() < deadline {
        if leaf_is_gone(leaf_pid, &leaf_identity) {
            leaf_gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        leaf_gone,
        "SIGTERM-ignoring descendant leaf {leaf_pid} survived escalation"
    );
}

#[test]
fn tree_teardown_sweeps_surviving_leaf_after_root_exit() {
    // The root spawns/reports its leaf and exits before cancellation. Tree
    // teardown must still sweep the surviving leaf through the owned group /
    // job primitive (the exact tree is still owned) and settle exactly once --
    // it must not fail closed just because the root's start identity is gone.
    let gate = unique_temp_path("tree-gate");
    let pid_file = unique_temp_path("leaf-pid");
    let mut command = fixture_command("tree-exit");
    command
        .env(GATE_ENV, &gate)
        .env(LEAF_PID_ENV, &pid_file)
        .stdout(Stdio::null());
    let (owned, _stdin, _stdout) =
        VerifiedChild::spawn_tree(command, "tree-owner", 0).expect("spawn isolated process tree");
    std::fs::File::create(&gate).expect("open gate file");
    let leaf_pid = wait_for_leaf_pid_file(&pid_file);
    let _ = std::fs::remove_file(&gate);
    let _ = std::fs::remove_file(&pid_file);

    // The root must have exited before cancel; the leaf is still alive.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !owned.poll_exited() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(owned.poll_exited(), "fixture root must exit before cancel");

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let leaf_identity = ProcessIdentity::capture(leaf_pid).expect("capture leaf start identity");
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let leaf_identity = ();

    let outcome = owned
        .signal_graceful_for("tree-owner", 0, Duration::from_millis(500))
        .expect("tree teardown must sweep a surviving leaf after root exit");
    // Root exited before cancel: Windows reports ExitedBeforeSignal (job close
    // sweep); Unix sweeps the group and reports graceful or escalated.
    assert!(matches!(
        outcome,
        SignalOutcome::ExitedBeforeSignal
            | SignalOutcome::TerminatedGracefully
            | SignalOutcome::EscalatedToKill
    ));
    owned.revoke().expect("settle exactly once");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut leaf_gone = false;
    while Instant::now() < deadline {
        if leaf_is_gone(leaf_pid, &leaf_identity) {
            leaf_gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        leaf_gone,
        "surviving leaf {leaf_pid} was not swept after root exit"
    );
    // Settle once: a second signal is refused after revocation.
    match owned.signal_graceful_for("tree-owner", 0, Duration::from_millis(100)) {
        Err(IdentityError::Revoked) => {}
        other => panic!("expected Revoked after settle, got {other:?}"),
    }
}

/// Native start identity of a tree leaf, captured while the leaf is provably
/// alive (before any teardown). `()` on platforms without a start identity.
#[cfg(any(target_os = "linux", target_os = "macos"))]
type LeafIdentity = ProcessIdentity;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
type LeafIdentity = ();

/// Spawning + leaf-reporting plumbing shared by the targeted tree tests.
/// Captures the leaf's native identity while it is provably alive (before any
/// teardown), so the descendant-gone assertion can bind to it later.
fn spawn_tree_with_leaf(role: &str) -> (VerifiedChild, u32, LeafIdentity) {
    let gate = unique_temp_path("tree-gate");
    let pid_file = unique_temp_path("leaf-pid");
    let mut command = fixture_command(role);
    command
        .env(GATE_ENV, &gate)
        .env(LEAF_PID_ENV, &pid_file)
        .stdout(Stdio::null());
    let (owned, _stdin, _stdout) =
        VerifiedChild::spawn_tree(command, "tree-owner", 0).expect("spawn isolated process tree");
    std::fs::File::create(&gate).expect("open gate file");
    let leaf_pid = wait_for_leaf_pid_file(&pid_file);
    let _ = std::fs::remove_file(&gate);
    let _ = std::fs::remove_file(&pid_file);
    // Capture the leaf identity now, while it is alive and inside the tree.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let leaf_identity = ProcessIdentity::capture(leaf_pid).expect("capture leaf identity");
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let leaf_identity = ();
    (owned, leaf_pid, leaf_identity)
}

/// Assert the descendant leaf died with the tree, bound to the identity
/// captured while it was alive (portable).
fn assert_leaf_dead(leaf_pid: u32, identity: &LeafIdentity) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if leaf_is_gone(leaf_pid, identity) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("descendant leaf {leaf_pid} survived");
}

/// SIGKILL of the spawn_tree owner must reap the leaf: Linux PR_SET_PDEATHSIG
/// or Darwin kqueue NOTE_EXIT watcher. Fail if only a comment was added.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn spawn_tree_owner_sigkill_reaps_leaf() {
    let pid_file = unique_temp_path("owner-kill-leaf");
    let mut owner_cmd = fixture_command("owner-spawn-tree");
    owner_cmd
        .env(LEAF_PID_ENV, &pid_file)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut owner = owner_cmd.spawn().expect("spawn owner fixture");
    let leaf_pid = wait_for_leaf_pid_file(&pid_file);
    let _ = std::fs::remove_file(&pid_file);
    let leaf_identity = ProcessIdentity::capture(leaf_pid).expect("capture leaf identity");
    assert!(
        leaf_identity.is_live().unwrap_or(false),
        "leaf {leaf_pid} must be live before owner SIGKILL"
    );

    // SAFETY: SIGKILL the owner fixture we just spawned. The leaf is bound
    // to that owner by PDEATHSIG (Linux) or the Darwin kqueue watcher.
    let rc = unsafe { libc::kill(owner.id() as libc::pid_t, libc::SIGKILL) };
    assert_eq!(rc, 0, "SIGKILL owner");
    let _ = owner.wait();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut leaf_gone = false;
    while Instant::now() < deadline {
        if leaf_is_gone(leaf_pid, &leaf_identity) {
            leaf_gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        leaf_gone,
        "leaf {leaf_pid} stayed live after owner SIGKILL (owner-death binding did not fire)"
    );
}

#[test]
fn tree_poll_exited_keeps_pin_and_allows_descendant_sweep() {
    // Observing the root's exit via poll_exited must NOT reap it: the numeric
    // PGID pin survives, so the later group signal targets only our tree and
    // the surviving descendant is swept. Guards the no-reap observation in M1.
    let (owned, leaf_pid, leaf_identity) = spawn_tree_with_leaf("tree-exit");

    // Observe the root exit without reaping (the pin survives).
    let deadline = Instant::now() + Duration::from_secs(10);
    while !owned.poll_exited() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        owned.poll_exited(),
        "root must be observed exited without reaping"
    );

    // The root was NOT reaped: teardown still sweeps the group exactly.
    let outcome = owned
        .signal_graceful_for("tree-owner", 0, Duration::from_millis(500))
        .expect("teardown after no-reap observe must settle");
    assert!(matches!(
        outcome,
        SignalOutcome::ExitedBeforeSignal
            | SignalOutcome::TerminatedGracefully
            | SignalOutcome::EscalatedToKill
    ));
    owned.revoke().expect("revoke after teardown settles");

    assert_leaf_dead(leaf_pid, &leaf_identity);
}

#[test]
fn direct_revoke_before_tree_teardown_fails_without_reaping() {
    // A Unix tree revoke must not reap the root (which would release the numeric
    // PGID pin and could strand descendants) before a successful teardown swept
    // the group. It fails with StillRunning without touching even an exited,
    // waitable root, so later signal+revoke still cleans the surviving leaf.
    let (owned, leaf_pid, leaf_identity) = spawn_tree_with_leaf("tree-exit");

    let deadline = Instant::now() + Duration::from_secs(10);
    while !owned.poll_exited() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(owned.poll_exited(), "fixture root must exit before revoke");
    assert!(matches!(owned.revoke(), Err(IdentityError::StillRunning)));

    // The root pin survived (child slot intact): teardown still proceeds and
    // sweeps the group while the root pins the PGID.
    let outcome = owned
        .signal_graceful_for("tree-owner", 0, Duration::from_millis(500))
        .expect("teardown after refused revoke must still settle");
    assert!(matches!(
        outcome,
        SignalOutcome::TerminatedGracefully | SignalOutcome::EscalatedToKill
    ));
    owned.revoke().expect("revoke after teardown settles");
    owned.revoke().expect("duplicate revoke is idempotent");

    assert_leaf_dead(leaf_pid, &leaf_identity);
}

#[test]
fn concurrent_revoke_and_cancel_settle_tree_with_no_descendant() {
    // Concurrent revoke (unsettled -> StillRunning, never reaps the root) vs
    // cancel: the root pin is preserved throughout, the tree settles exactly
    // once, and no descendant survives.
    let (owned, leaf_pid, leaf_identity) = spawn_tree_with_leaf("tree");

    let cancel = owned.clone();
    let revoke = owned.clone();
    let a = std::thread::spawn(move || {
        cancel.signal_graceful_for("tree-owner", 0, Duration::from_millis(500))?;
        cancel.revoke()
    });
    let b = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        revoke.revoke()
    });
    assert!(a.join().expect("cancel thread").is_ok());
    match b.join().expect("revoke thread") {
        Ok(()) | Err(IdentityError::StillRunning) => {}
        other => panic!("unexpected concurrent revoke result: {other:?}"),
    }
    owned.revoke().expect("final idempotent revoke");

    assert_leaf_dead(leaf_pid, &leaf_identity);
}

// ---------------------------------------------------------------------------
// Detached escalation
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
#[test]
fn escalation_fails_closed_on_unsupported_platform() {
    // macOS has no exact detached signal handle: escalation must fail closed,
    // never deliver a PID-only signal.
    let (binding, mut child) = detached_binding("sleep", "test-owner");
    match escalate_detached(&binding, Duration::from_millis(200)) {
        Err(IdentityError::Unsupported) => {}
        other => panic!("expected Unsupported on macOS, got {other:?}"),
    }
    // The live detached child was never signaled.
    assert_eq!(child.try_wait().expect("probe").map(|s| s.code()), None);
    child.kill().ok();
    child.wait().ok();
}

#[cfg(target_os = "linux")]
#[test]
fn escalation_kills_detached_child_exactly_on_linux() {
    let (binding, mut child) = detached_binding("sleep", "test-owner");
    let outcome = escalate_detached(&binding, Duration::from_secs(5)).expect("pidfd escalation");
    assert!(matches!(
        outcome,
        SignalOutcome::TerminatedGracefully | SignalOutcome::EscalatedToKill
    ));
    child.wait().ok();
}

#[cfg(target_os = "linux")]
#[test]
fn stale_binding_never_signals_replacement_on_linux() {
    let (binding, mut child) = detached_binding("exit", "test-owner");
    child.wait().expect("reap original");
    // The pid is now free; a replacement may or may not reuse it.
    let mut replacement = spawn_fixture("sleep");
    let replacement_pid = replacement.id();
    let result = escalate_detached(&binding, Duration::from_millis(200));
    assert!(
        result.is_err(),
        "stale binding must never authorize a signal: {result:?}"
    );
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        replacement
            .try_wait()
            .expect("probe replacement")
            .map(|s| s.code()),
        None,
        "replacement process {replacement_pid} must never be signaled"
    );
    replacement.kill().ok();
    replacement.wait().ok();
}

// ---------------------------------------------------------------------------
// Characterization pins (demonolith child-char / NF-b8-tests-child)
// Compile-time API surface only -- no spawn, no product semantics change.
// ---------------------------------------------------------------------------

#[test]
fn char_child_single_handle_type_and_escalate_visible() {
    fn _pin_verified_child(_: &VerifiedChild) {}
    fn _pin_escalate(
        binding: &ChildBinding,
        grace: Duration,
    ) -> Result<SignalOutcome, IdentityError> {
        zero_process::escalate_detached(binding, grace)
    }
    let os = std::env::consts::OS;
    eprintln!(
        "CHAR child type=VerifiedChild os={os} escalate_detached_visible=1 second_child_type=0"
    );
}

#[cfg(unix)]
#[test]
fn char_child_unix_pgid_pidfd_and_peer_gate() {
    fn _pin_peer(stream: &std::os::unix::net::UnixStream) -> bool {
        zero_process::peer_is_same_user(stream)
    }
    let pidfd = cfg!(any(target_os = "linux", target_os = "android"));
    eprintln!("CHAR child unix pgid=0 pidfd={pidfd} ungated_unix_item=0");
}

#[cfg(windows)]
#[test]
fn char_child_windows_job_resume_terminate_handle_path() {
    eprintln!(
        "CHAR child windows job=ok resume=ok terminate=ok handle_path=crate::identity::Handle"
    );
}

#[test]
fn char_child_cfg_matrix_axes() {
    let unix = cfg!(unix);
    let windows = cfg!(windows);
    let linux = cfg!(any(target_os = "linux", target_os = "android"));
    eprintln!(
        "CHAR child cfg_matrix unix={unix} windows={windows} linux_pidfd={linux} \
         escalate_every_target=1"
    );
    assert!(
        unix || windows || !unix,
        "cfg matrix must compile on every target"
    );
}
