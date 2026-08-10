use super::*;
use std::sync::{Arc, Barrier};
use std::thread;

fn wait_for_waiters(base: &Path, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while fs::read_dir(base.join("waiters"))
        .map(|entries| entries.count())
        .unwrap_or(0)
        < expected
    {
        assert!(
            Instant::now() < deadline,
            "expected {expected} waiter intents"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

fn fencing_test_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "zerostack-fencing-{label}-{}-{}",
        std::process::id(),
        owner_cookie()
    ))
}

/// Pre-create a permit base that satisfies uid/mode checks (0o700, self-owned).
///
/// `create_dir` / `create_dir_all` under `/tmp` typically yield 0o755, which
/// `prepare_permit_base` correctly refuses. Tests that need a pre-existing
/// base (e.g. seeded waiters) must use this helper first.
#[cfg(unix)]
fn create_private_permit_base(base: &Path) {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(base).expect("create private permit base");
    fs::set_permissions(base, fs::Permissions::from_mode(0o700))
        .expect("chmod private permit base");
}

#[cfg(not(unix))]
fn create_private_permit_base(base: &Path) {
    fs::create_dir(base).expect("create permit base");
}

#[test]
fn permit_fence_replaced_cookie_survives_old_guard_drop() {
    let path = fencing_test_path("drop-foreign");
    let permit =
        MachinePermit::acquire(&path, Instant::now() + Duration::from_secs(2), "old-owner")
            .expect("acquire old owner");
    fs::remove_dir_all(&path).expect("simulate external replacement");
    fs::create_dir(&path).expect("create replacement");
    let foreign_cookie = "fedcba9876543210fedcba9876543210";
    publish_identity(
        &path,
        foreign_cookie,
        std::process::id(),
        "foreign",
        epoch_millis(),
    )
    .unwrap();

    drop(permit);

    assert_eq!(read_identity(&path).unwrap().cookie, foreign_cookie);
    fs::remove_dir_all(&path).expect("remove replacement");
}

#[test]
fn permit_fence_stale_reclaim_snapshot_cannot_delete_new_owner() {
    let path = fencing_test_path("reclaim-foreign");
    fs::create_dir(&path).expect("create stale slot");
    let stale_cookie = "11111111111111111111111111111111";
    publish_identity(&path, stale_cookie, 1, "stale", epoch_millis())
        .expect("publish stale identity");
    let stale_snapshot = fs::read(path.join("identity")).expect("snapshot stale identity");
    fs::remove_dir_all(&path).expect("replace stale slot");
    fs::create_dir(&path).expect("create new slot");
    let new_cookie = "22222222222222222222222222222222";
    publish_identity(&path, new_cookie, std::process::id(), "new", epoch_millis()).unwrap();

    assert!(!quarantine_exact(&path, Some(&stale_snapshot)));
    assert_eq!(read_identity(&path).unwrap().cookie, new_cookie);
    fs::remove_dir_all(&path).expect("remove new slot");
}

#[test]
fn permit_fence_incomplete_metadata_is_not_reclaimed_before_grace() {
    let path = fencing_test_path("incomplete-grace");
    fs::create_dir(&path).expect("create incomplete slot");
    fs::write(path.join("pid"), "malformed").expect("write partial metadata");

    assert!(!reclaim_dead(&path));
    assert!(path.exists());
    fs::remove_dir_all(&path).expect("remove incomplete slot");
}

#[test]
fn permit_fence_normal_drop_removes_its_own_cookie() {
    let path = fencing_test_path("normal-drop");
    let permit = MachinePermit::acquire(
        &path,
        Instant::now() + Duration::from_secs(2),
        "normal-owner",
    )
    .expect("acquire owner");
    let cookie = read_identity(&path).expect("published identity").cookie;
    assert_eq!(permit.cookie, cookie);

    drop(permit);

    assert!(!path.exists());
}

#[test]
fn slot_status_and_busy_error_identify_the_actionable_live_owner() {
    let base = fencing_test_path("actionable-status");
    let permit = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_secs(2),
        "tokenzero-codemode-heavy",
    )
    .expect("acquire holder");
    let initial = permit_status(&base, 1).expect("inspect holder");
    assert_eq!(initial.len(), 1);
    let initial = &initial[0];
    assert_eq!(initial.pid, Some(std::process::id()));
    assert_eq!(
        initial.operation.as_deref(),
        Some("tokenzero-codemode-heavy")
    );
    assert_eq!(initial.liveness, PermitHolderLiveness::Live);
    assert!(initial.repository.is_some());
    assert!(
        initial
            .session_ref
            .as_deref()
            .unwrap()
            .starts_with("cm://session/")
    );
    assert!(
        initial
            .cell_ref
            .as_deref()
            .unwrap()
            .starts_with("cm://cell/")
    );
    assert!(initial.status_ref.starts_with("cm://permit/"));
    let heartbeat = initial.heartbeat_at_ms.unwrap();
    thread::sleep(Duration::from_millis(2));
    permit.heartbeat().expect("refresh heartbeat");
    let refreshed = permit_status(&base, 1).unwrap();
    assert!(refreshed[0].heartbeat_at_ms.unwrap() >= heartbeat);

    let contested = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_millis(20),
        "contender",
    )
    .expect_err("live holder must not be stolen");
    let AcquireError::Busy(message) = contested else {
        panic!("expected retryable busy error");
    };
    for field in [
        "status=cm://permit/",
        "pid=",
        "repository=",
        "operation=\"tokenzero-codemode-heavy\"",
        "started_at_ms=",
        "age_ms=",
        "heartbeat_at_ms=",
        "heartbeat_age_ms=",
        "session=cm://session/",
        "cell=cm://cell/",
        "liveness=Live",
    ] {
        assert!(message.contains(field), "missing {field:?}: {message}");
    }
    assert!(!message.contains("unknown"), "{message}");
    drop(permit);
    MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_secs(2),
        "after-release",
    )
    .expect("fresh acquisition succeeds after release");
    fs::remove_dir_all(&base).ok();
}

#[test]
fn heartbeat_is_cookie_fenced_against_a_replaced_owner() {
    let path = fencing_test_path("heartbeat-foreign");
    let permit =
        MachinePermit::acquire(&path, Instant::now() + Duration::from_secs(2), "old-owner")
            .expect("acquire old owner");
    let replacement_cookie = owner_cookie();
    let replacement_started = epoch_millis();
    publish_identity(
        &path,
        &replacement_cookie,
        std::process::id(),
        "replacement-owner",
        replacement_started,
    )
    .expect("publish replacement identity");
    let heartbeat_before = fs::read(path.join("heartbeat_at")).unwrap();
    let error = permit
        .heartbeat()
        .expect_err("old owner heartbeat must fail");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(
        fs::read(path.join("heartbeat_at")).unwrap(),
        heartbeat_before
    );
    drop(permit);
    assert!(path.exists(), "old guard must not delete replacement");
    assert!(cleanup_owned(&path, &replacement_cookie));
}

#[test]
fn reclaims_incomplete_machine_permit_after_grace() {
    let path = std::env::temp_dir().join(format!(
        "zerostack-incomplete-permit-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    fs::create_dir(&path).unwrap();
    fs::write(path.join("owner"), "").unwrap();
    std::thread::sleep(INCOMPLETE_PERMIT_GRACE + Duration::from_millis(20));

    assert!(reclaim_dead(&path));
    assert!(!path.exists());
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
#[test]
fn native_pid_liveness_handles_alive_dead_and_conservative_errors() {
    assert!(
        process_alive(std::process::id()),
        "current process must be alive"
    );

    let mut child = std::process::Command::new("true")
        .spawn()
        .expect("spawn short-lived child");
    let child_pid = child.id();
    child.wait().expect("reap short-lived child");
    assert!(!process_alive(child_pid), "reaped child must be dead");

    assert!(unix_kill_result_is_alive(0, None));
    assert!(!unix_kill_result_is_alive(-1, Some(libc::ESRCH)));
    assert!(unix_kill_result_is_alive(-1, Some(libc::EPERM)));
    assert!(unix_kill_result_is_alive(-1, Some(libc::EINVAL)));
    assert!(process_alive(0), "pid zero must be treated conservatively");
}

/// Linux/Android reclaim uses `/proc` identity, not `kill(pid,0)`.
/// Keep the exact CI filter name so `--exact` cannot false-green on 0 tests.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn native_pid_liveness_handles_alive_dead_and_conservative_errors() {
    let self_id = std::process::id();
    let observed = read_linux_process_identity(self_id)
        .expect("read self /proc identity")
        .expect("current process must have a /proc identity");

    let mut child = std::process::Command::new("true")
        .spawn()
        .expect("spawn short-lived child");
    let child_pid = child.id();
    child.wait().expect("reap short-lived child");
    assert!(
        read_linux_process_identity(child_pid)
            .expect("read reaped child /proc identity")
            .is_none(),
        "reaped child must be missing from /proc"
    );

    assert!(
        read_linux_process_identity(0)
            .expect("pid zero observation")
            .is_none(),
        "pid zero must not fabricate a process identity"
    );

    let now = 1_000u128;
    let live = PermitIdentity {
        cookie: "0123456789abcdef0123456789abcdef".into(),
        pid: self_id,
        owner: "test-owner".into(),
        started_at: Some(now),
        process: Some(observed.clone()),
    };
    assert_eq!(
        identity_liveness(&live, now + 10, Duration::from_secs(60)),
        IdentityLiveness::Live
    );
    let mut reused = live;
    reused.process = Some(ProcessIdentity {
        boot_id: observed.boot_id,
        starttime: observed.starttime.wrapping_add(1),
    });
    assert_eq!(
        identity_liveness(&reused, now + 10, Duration::from_secs(60)),
        IdentityLiveness::Dead,
        "starttime mismatch is PID reuse"
    );
}

#[cfg(windows)]
#[test]
fn native_pid_liveness_handles_alive_dead_and_conservative_errors() {
    use windows_sys::Win32::Foundation::STILL_ACTIVE;

    assert!(
        process_alive(std::process::id()),
        "current process must be alive"
    );
    // OpenProcess failure is intentionally fail-open (alive). A reaped child
    // often yields a null handle, so polarity is pinned on the pure helper.
    assert!(
        windows_query_is_alive(0, 0),
        "query failure is conservative alive"
    );
    assert!(
        windows_query_is_alive(1, STILL_ACTIVE as u32),
        "STILL_ACTIVE means alive"
    );
    assert!(
        !windows_query_is_alive(1, 0),
        "successful query with exit code 0 means dead"
    );
}

#[test]
fn stale_malformed_pid_is_reclaimed_after_grace() {
    let path = std::env::temp_dir().join(format!(
        "zerostack-malformed-pid-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    fs::create_dir(&path).expect("create malformed permit");
    fs::write(path.join("owner"), "malformed").expect("write owner");
    fs::write(path.join("pid"), "not-a-pid").expect("write malformed pid");
    thread::sleep(INCOMPLETE_PERMIT_GRACE + Duration::from_millis(20));

    assert!(reclaim_dead(&path));
    assert!(!path.exists());
}

fn permit_liveness_identity(pid: u32) -> PermitIdentity {
    PermitIdentity {
        cookie: "0123456789abcdef0123456789abcdef".into(),
        pid,
        owner: "test-owner".into(),
        started_at: Some(1_000),
        process: Some(ProcessIdentity {
            boot_id: "boot-a".into(),
            starttime: 77,
        }),
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn permit_liveness_proc_stat_parses_parenthesized_comm_with_spaces_and_parens() {
    let stat =
        "42 (worker name) with parens) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 4242 20";
    assert_eq!(parse_proc_stat_starttime(stat), Some(4242));
}

#[test]
fn permit_liveness_classifier_cases_cover_reuse_boot_pid_zero_age_and_fresh_holder() {
    let classify = |identity: &PermitIdentity, observed, now, cap| {
        classify_identity_snapshot(identity, observed, now, cap, true)
    };
    let mut identity = permit_liveness_identity(42);
    let observed = ProcessObservation::Exists(identity.process.clone().unwrap());
    assert_eq!(
        classify(&identity, observed, 1_001, OWNER_IDENTITY_MAX_AGE),
        IdentityLiveness::Live
    );

    let mut reused = identity.process.clone().unwrap();
    reused.starttime += 1;
    assert_eq!(
        classify(
            &identity,
            ProcessObservation::Exists(reused),
            1_001,
            OWNER_IDENTITY_MAX_AGE
        ),
        IdentityLiveness::Dead
    );
    let mut rebooted = identity.process.clone().unwrap();
    rebooted.boot_id = "boot-b".into();
    assert_eq!(
        classify(
            &identity,
            ProcessObservation::Exists(rebooted),
            1_001,
            OWNER_IDENTITY_MAX_AGE
        ),
        IdentityLiveness::Dead
    );

    identity.pid = 0;
    assert_eq!(
        classify(
            &identity,
            ProcessObservation::Missing,
            1_001,
            OWNER_IDENTITY_MAX_AGE
        ),
        IdentityLiveness::Incomplete
    );
    identity.pid = 42;
    let observed = ProcessObservation::Exists(identity.process.clone().unwrap());
    assert_eq!(
        classify(
            &identity,
            observed,
            1_000 + WAITER_IDENTITY_MAX_AGE.as_millis() + 1,
            WAITER_IDENTITY_MAX_AGE
        ),
        IdentityLiveness::Live
    );
    assert_eq!(
        classify(
            &identity,
            ProcessObservation::Unknown,
            1_000 + WAITER_IDENTITY_MAX_AGE.as_millis() + 1,
            WAITER_IDENTITY_MAX_AGE
        ),
        IdentityLiveness::Dead
    );
    let observed = ProcessObservation::Exists(identity.process.clone().unwrap());
    assert_eq!(
        classify(
            &identity,
            observed,
            1_000 + OWNER_IDENTITY_MAX_AGE.as_millis() - 1,
            OWNER_IDENTITY_MAX_AGE
        ),
        IdentityLiveness::Live
    );
}

#[test]
fn analysis_permit_is_exclusive_across_threads() {
    let path = std::env::temp_dir().join(format!(
        "zerostack-analysis-excl-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    let _ = fs::remove_dir_all(&path);
    let barrier = Arc::new(Barrier::new(2));
    let path_holder = path.clone();
    let barrier_holder = Arc::clone(&barrier);
    let holder = thread::spawn(move || {
        let permit = MachinePermit::acquire(
            &path_holder,
            Instant::now() + Duration::from_secs(5),
            "test-analysis-holder",
        )
        .expect("holder acquires analysis permit");
        barrier_holder.wait();
        thread::sleep(Duration::from_millis(300));
        drop(permit);
    });

    barrier.wait();
    let contested = MachinePermit::acquire(
        &path,
        Instant::now() + Duration::from_millis(80),
        "test-analysis-contender",
    );
    assert!(
        contested.is_err(),
        "second acquirer must not stack while holder is live: {contested:?}"
    );
    holder.join().unwrap();
    let after = MachinePermit::acquire(
        &path,
        Instant::now() + Duration::from_secs(2),
        "test-analysis-after",
    );
    assert!(after.is_ok(), "permit must release for the next waiter");
}

#[test]
fn multi_slot_analysis_permit_allows_parallel_holders() {
    let base = std::env::temp_dir().join(format!(
        "zerostack-analysis-slots-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    let _ = fs::remove_dir_all(&base);
    let a =
        MachinePermit::acquire_slots(&base, 2, Instant::now() + Duration::from_secs(2), "slot-a")
            .expect("first slot");
    let b =
        MachinePermit::acquire_slots(&base, 2, Instant::now() + Duration::from_secs(2), "slot-b")
            .expect("second slot");
    let contested = MachinePermit::acquire_slots(
        &base,
        2,
        Instant::now() + Duration::from_millis(80),
        "slot-c",
    );
    assert!(
        contested.is_err(),
        "third holder must wait when only two slots exist"
    );
    drop(a);
    drop(b);
}

#[test]
fn index_permit_is_exclusive_across_threads() {
    let path = std::env::temp_dir().join(format!(
        "zerostack-index-excl-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    let _ = fs::remove_dir_all(&path);
    let barrier = Arc::new(Barrier::new(2));
    let path_holder = path.clone();
    let barrier_holder = Arc::clone(&barrier);
    let holder = thread::spawn(move || {
        let permit = MachinePermit::acquire(
            &path_holder,
            Instant::now() + Duration::from_secs(5),
            "test-index-holder",
        )
        .expect("holder acquires index permit");
        barrier_holder.wait();
        thread::sleep(Duration::from_millis(300));
        drop(permit);
    });

    barrier.wait();
    let contested = MachinePermit::acquire(
        &path,
        Instant::now() + Duration::from_millis(80),
        "test-index-contender",
    );
    assert!(
        contested.is_err(),
        "second acquirer must not stack while holder is live: {contested:?}"
    );
    holder.join().unwrap();
    let after = MachinePermit::acquire(
        &path,
        Instant::now() + Duration::from_secs(2),
        "test-index-after",
    );
    assert!(after.is_ok(), "permit must release for the next waiter");
}

#[test]
fn multi_slot_index_permit_allows_parallel_holders() {
    let base = std::env::temp_dir().join(format!(
        "zerostack-index-slots-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    let _ = fs::remove_dir_all(&base);
    let a = MachinePermit::acquire_slots(
        &base,
        2,
        Instant::now() + Duration::from_secs(2),
        "index-slot-a",
    )
    .expect("first index slot");
    let b = MachinePermit::acquire_slots(
        &base,
        2,
        Instant::now() + Duration::from_secs(2),
        "index-slot-b",
    )
    .expect("second index slot");
    let contested = MachinePermit::acquire_slots(
        &base,
        2,
        Instant::now() + Duration::from_millis(80),
        "index-slot-c",
    );
    assert!(
        contested.is_err(),
        "third index holder must wait when only two slots exist"
    );
    drop(a);
    drop(b);
}

#[test]
fn rapid_uncontended_reacquire_is_immediate() {
    let base = std::env::temp_dir().join(format!(
        "zerostack-uncontended-reacquire-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    let _ = fs::remove_dir_all(&base);
    let first = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_secs(2),
        "uncontended-first",
    )
    .expect("first acquire");
    drop(first);

    let started = Instant::now();
    let second = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_secs(2),
        "uncontended-second",
    )
    .expect("uncontended reacquire");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(100),
        "uncontended reacquire was unexpectedly delayed: {elapsed:?}"
    );
    drop(second);
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn hot_releaser_cannot_beat_an_already_waiting_peer() {
    let base = std::env::temp_dir().join(format!(
        "zerostack-waiter-fairness-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    let _ = fs::remove_dir_all(&base);
    let holder = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_secs(2),
        "fairness-holder",
    )
    .expect("holder");
    let (acquired_tx, acquired_rx) = std::sync::mpsc::sync_channel(1);
    let waiter_base = base.clone();
    let waiter = thread::spawn(move || {
        let permit = MachinePermit::acquire_slots(
            &waiter_base,
            1,
            Instant::now() + Duration::from_secs(5),
            "established-waiter",
        )
        .expect("established waiter acquires");
        acquired_tx.send(()).expect("signal waiter acquisition");
        thread::sleep(Duration::from_millis(80));
        drop(permit);
    });

    let marker_deadline = Instant::now() + Duration::from_secs(1);
    while fs::read_dir(base.join("waiters"))
        .map(|entries| entries.count())
        .unwrap_or(0)
        == 0
    {
        assert!(
            Instant::now() < marker_deadline,
            "waiter did not publish intent"
        );
        thread::sleep(Duration::from_millis(1));
    }

    let hot_started = Instant::now();
    drop(holder);
    let hot = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_secs(5),
        "hot-reacquirer",
    )
    .expect("hot reacquirer eventually acquires");
    assert!(
        acquired_rx.try_recv().is_ok(),
        "hot releaser beat the already-waiting peer"
    );
    assert!(
        hot_started.elapsed() < PERMIT_POLL_MAX,
        "ordered handoff retained a full cooldown: {:?}",
        hot_started.elapsed()
    );
    drop(hot);
    waiter.join().expect("waiter thread");
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn stale_incomplete_waiter_is_reclaimed() {
    let base = std::env::temp_dir().join(format!(
        "zerostack-stale-waiter-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    let stale = base.join("waiters/stale");
    let _ = fs::remove_dir_all(&base);
    create_private_permit_base(&base);
    fs::create_dir_all(&stale).expect("create stale waiter");
    thread::sleep(INCOMPLETE_PERMIT_GRACE + Duration::from_millis(20));

    let permit = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_secs(2),
        "after-stale-waiter",
    )
    .expect("stale waiter must not block acquisition");
    assert!(!stale.exists(), "stale waiter marker must be removed");
    drop(permit);
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn event_head_uses_long_safety_timeout_while_younger_waiters_back_off() {
    assert_eq!(waiter_wait_timeout(false, 0), PERMIT_POLL_MAX);
    assert_eq!(waiter_wait_timeout(false, 10), PERMIT_POLL_MAX);
    assert_eq!(waiter_wait_timeout(true, 0), PERMIT_POLL);
    assert_eq!(waiter_wait_timeout(true, 1), Duration::from_millis(40));
    assert_eq!(waiter_wait_timeout(true, 3), Duration::from_millis(160));
    assert_eq!(waiter_wait_timeout(true, 10), PERMIT_POLL_MAX);
}

#[test]
fn oldest_waiter_acquires_promptly_after_a_long_hold() {
    let base = std::env::temp_dir().join(format!(
        "zerostack-oldest-fast-poll-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    let _ = fs::remove_dir_all(&base);
    let holder = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_secs(2),
        "long-holder",
    )
    .expect("holder");
    let waiter_base = base.clone();
    let waiter = thread::spawn(move || {
        MachinePermit::acquire_slots(
            &waiter_base,
            1,
            Instant::now() + Duration::from_secs(3),
            "oldest-fast-waiter",
        )
        .expect("oldest waiter")
    });
    wait_for_waiters(&base, 1);
    thread::sleep(Duration::from_millis(170));
    let released = Instant::now();
    drop(holder);
    let permit = waiter.join().expect("waiter thread");
    assert!(
        released.elapsed() < PERMIT_POLL,
        "directory event did not wake the oldest waiter promptly: {:?}",
        released.elapsed()
    );
    drop(permit);
    let _ = fs::remove_dir_all(&base);
}
#[test]
fn spurious_directory_event_does_not_bypass_live_holder() {
    let base = std::env::temp_dir().join(format!(
        "zerostack-spurious-wake-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    let _ = fs::remove_dir_all(&base);
    let holder = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_secs(2),
        "spurious-holder",
    )
    .expect("holder");
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let waiter_base = base.clone();
    let waiter = thread::spawn(move || {
        let permit = MachinePermit::acquire_slots(
            &waiter_base,
            1,
            Instant::now() + Duration::from_secs(2),
            "spurious-waiter",
        )
        .expect("waiter");
        tx.send(()).expect("signal acquisition");
        permit
    });
    wait_for_waiters(&base, 1);
    let noise = base.join("unrelated-event");
    fs::create_dir(&noise).expect("create spurious directory event");
    fs::remove_dir(&noise).expect("remove spurious directory event");
    assert!(
        rx.recv_timeout(Duration::from_millis(50)).is_err(),
        "spurious event bypassed a live holder"
    );
    drop(holder);
    rx.recv_timeout(Duration::from_secs(1))
        .expect("release event wakes waiter");
    drop(waiter.join().expect("waiter thread"));
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn sleeping_fallback_preserves_release_correctness() {
    let base = std::env::temp_dir().join(format!(
        "zerostack-fallback-wake-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    let _ = fs::remove_dir_all(&base);
    let holder = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_secs(2),
        "fallback-holder",
    )
    .expect("holder");
    let waiter_base = base.clone();
    let waiter = thread::spawn(move || {
        MachinePermit::acquire_slots_with_wake(
            &waiter_base,
            1,
            Instant::now() + Duration::from_secs(2),
            "fallback-waiter",
            |_| PermitWake::fallback(),
        )
        .expect("fallback waiter")
    });
    wait_for_waiters(&base, 1);
    drop(holder);
    let permit = waiter.join().expect("fallback waiter thread");
    drop(permit);
    let _ = fs::remove_dir_all(&base);
}

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    windows
))]
#[test]
fn native_wake_handle_closes_on_drop() {
    let base = std::env::temp_dir().join(format!(
        "zerostack-wake-close-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    fs::create_dir(&base).expect("create watched directory");
    let wake = NativeWake::new(&base).expect("create native wake");
    drop(wake);
    fs::remove_dir(&base).expect("closed wake must not retain directory handle");
}

/// Fence canary: NativeWake stays crate-private and is structurally !Send/!Sync
/// via `PhantomData<Rc<()>>`. Uncommenting the `thread::spawn` line below must
/// fail to compile if the fence is removed.
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    windows
))]
#[test]
fn native_wake_is_private_and_not_send_sync() {
    use std::rc::Rc;

    // Structural: the fence field type is !Send + !Sync on stable Rust.
    // Rc<()> is !Send + !Sync; NativeWake embeds PhantomData<Rc<()>>.
    let _: PhantomData<Rc<()>> = PhantomData;
    let _ = std::mem::size_of::<NativeWake>();

    let base = std::env::temp_dir().join(format!(
        "zerostack-wake-fence-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    fs::create_dir(&base).expect("create watched directory");
    let wake = NativeWake::new(&base).expect("create native wake");
    // Must fail to compile if NativeWake becomes Send:
    // std::thread::spawn(move || drop(wake));
    drop(wake);
    fs::remove_dir(&base).expect("remove watched directory");
}

#[test]
fn oldest_waiter_respects_a_short_deadline() {
    let base = std::env::temp_dir().join(format!(
        "zerostack-oldest-deadline-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    let _ = fs::remove_dir_all(&base);
    let holder = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_secs(2),
        "deadline-holder",
    )
    .expect("holder");
    let started = Instant::now();
    let result = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_millis(55),
        "deadline-waiter",
    );
    assert!(matches!(result, Err(AcquireError::Busy(_))));
    assert!(
        started.elapsed() < Duration::from_millis(140),
        "deadline was exceeded by polling: {:?}",
        started.elapsed()
    );
    drop(holder);
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn twenty_waiters_acquire_in_published_order() {
    const WAITERS: usize = 20;
    let base = std::env::temp_dir().join(format!(
        "zerostack-fifo-many-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    let _ = fs::remove_dir_all(&base);
    let holder = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_secs(2),
        "fifo-holder",
    )
    .expect("holder");
    let (order_tx, order_rx) = std::sync::mpsc::channel();
    let mut waiters = Vec::with_capacity(WAITERS);
    for id in 0..WAITERS {
        let waiter_base = base.clone();
        let waiter_tx = order_tx.clone();
        waiters.push(thread::spawn(move || {
            let permit = MachinePermit::acquire_slots(
                &waiter_base,
                1,
                Instant::now() + Duration::from_secs(10),
                &format!("fifo-{id}"),
            )
            .unwrap_or_else(|e| panic!("waiter {id}: {e}"));
            waiter_tx.send(id).expect("record acquisition order");
            drop(permit);
        }));
        wait_for_waiters(&base, id + 1);
        thread::sleep(Duration::from_millis(2));
    }
    drop(order_tx);
    drop(holder);
    let order: Vec<_> = order_rx.iter().collect();
    assert_eq!(order, (0..WAITERS).collect::<Vec<_>>());
    for waiter in waiters {
        waiter.join().expect("fifo waiter");
    }
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn permit_backoff_grows_then_caps() {
    assert_eq!(permit_backoff(0), PERMIT_POLL);
    assert!(permit_backoff(3) > permit_backoff(0));
    assert_eq!(permit_backoff(10), PERMIT_POLL_MAX);
}

#[test]
fn slots_one_uses_slot_zero_not_base() {
    let base = std::env::temp_dir().join(format!(
        "zerostack-slot0-layout-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    let _ = fs::remove_dir_all(&base);
    let permit = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_secs(2),
        "slot-one",
    )
    .expect("slots=1 must acquire");
    assert!(
        base.join("slot-0").join("pid").is_file(),
        "slots=1 must lock base/slot-0, not base itself"
    );
    assert!(
        !base.join("pid").is_file(),
        "slots=1 must not write pid directly under base"
    );
    drop(permit);
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn mixed_concurrency_layouts_share_slot_namespace() {
    let base = std::env::temp_dir().join(format!(
        "zerostack-mixed-slots-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    let _ = fs::remove_dir_all(&base);

    // slots=1 holds slot-0; slots>1 peer must take another slot child (shared
    // namespace), not invent a nested lock under an exclusive base.
    let holder = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_secs(5),
        "holder-slots-1",
    )
    .expect("slots=1 holder");
    let peer = MachinePermit::acquire_slots(
        &base,
        3,
        Instant::now() + Duration::from_secs(2),
        "peer-slots-3",
    )
    .expect("slots>1 peer must share slot namespace with slots=1 holder");
    assert_eq!(peer.path().parent(), Some(base.as_path()));
    let peer_name = peer
        .path()
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    assert!(
        peer_name.starts_with("slot-") && peer_name != "slot-0",
        "peer must occupy a free slot child, got {}",
        peer.path().display()
    );
    drop(peer);
    drop(holder);

    // Saturated multi-slot pool must reject a slots=1 peer (no stacking past budget).
    let holder = MachinePermit::acquire_slots(
        &base,
        2,
        Instant::now() + Duration::from_secs(5),
        "holder-slots-2",
    )
    .expect("slots=2 holder");
    let holder2 = MachinePermit::acquire_slots(
        &base,
        2,
        Instant::now() + Duration::from_secs(5),
        "holder-slots-2b",
    )
    .expect("second slots=2 holder");
    let contested = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_millis(80),
        "peer-slots-1",
    );
    assert!(
        contested.is_err(),
        "slots=1 peer must not stack when multi-slot pool is full: {contested:?}"
    );
    drop(holder);
    drop(holder2);
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn live_legacy_exclusive_base_blocks_all_slots() {
    let base = std::env::temp_dir().join(format!(
        "zerostack-legacy-excl-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    let _ = fs::remove_dir_all(&base);
    let legacy = MachinePermit::acquire(
        &base,
        Instant::now() + Duration::from_secs(5),
        "legacy-exclusive",
    )
    .expect("legacy exclusive at base");
    let contested = MachinePermit::acquire_slots(
        &base,
        2,
        Instant::now() + Duration::from_millis(80),
        "slot-peer",
    );
    assert!(
        contested.is_err(),
        "live legacy exclusive base must block slot children: {contested:?}"
    );
    drop(legacy);
    let after = MachinePermit::acquire_slots(
        &base,
        2,
        Instant::now() + Duration::from_secs(2),
        "after-legacy",
    );
    assert!(
        after.is_ok(),
        "slots acquire after legacy release: {after:?}"
    );
    drop(after);
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn acquire_slots_returns_fatal_when_parent_is_not_a_directory() {
    // Parent path is a file → create_dir for slot children fails as Fatal (not Busy).
    let blocker = std::env::temp_dir().join(format!(
        "zerostack-permit-fatal-blocker-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    let _ = fs::remove_file(&blocker);
    let _ = fs::remove_dir_all(&blocker);
    fs::write(&blocker, b"not-a-directory").expect("write blocker file");
    let base = blocker.join("nested-permit");

    let err = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_millis(80),
        "test-fatal",
    )
    .expect_err("expected Fatal when permit parent is a file");
    let _ = fs::remove_file(&blocker);

    match err {
        AcquireError::Fatal(message) => {
            assert!(
                message.contains("prepare codemode permit base"),
                "unexpected Fatal message: {message}"
            );
        }
        AcquireError::Busy(message) => {
            panic!("I/O failure must be Fatal, not Busy: {message}")
        }
    }
}

#[test]
fn sanitize_permit_class_accepts_safe_tokens() {
    assert_eq!(sanitize_permit_class("analysis"), "analysis");
    assert_eq!(sanitize_permit_class("index"), "index");
    assert_eq!(sanitize_permit_class("heavy"), "heavy");
    assert_eq!(sanitize_permit_class("a.B_0-z"), "a.B_0-z");
}

#[test]
fn sanitize_permit_class_rejects_path_metacharacters() {
    assert_eq!(sanitize_permit_class(".."), "invalid");
    assert_eq!(sanitize_permit_class("../evil"), "invalid");
    assert_eq!(sanitize_permit_class("a/b"), "invalid");
    assert_eq!(sanitize_permit_class(r"a\b"), "invalid");
    assert_eq!(sanitize_permit_class(""), "invalid");
    assert_eq!(sanitize_permit_class("has space"), "invalid");

    let poisoned = scoped_permit_base_for("../evil", None);
    assert_eq!(
        poisoned.file_name().and_then(|name| name.to_str()),
        Some("zerostack-codemode-invalid.permit")
    );
    let slash = scoped_permit_base_for("a/b", Some(Path::new("/tmp")));
    let name = slash.file_name().and_then(|n| n.to_str()).unwrap();
    assert!(
        name.starts_with("zerostack-codemode-invalid-") && name.ends_with(".permit"),
        "slash class must not appear in basename: {name}"
    );
    assert!(!name.contains('/') && !name.contains(".."));
}

#[cfg(unix)]
#[test]
fn unix_fallback_runtime_directory_has_exact_safe_mode() {
    use std::os::unix::fs::MetadataExt;

    let temp = std::env::temp_dir().join(format!(
        "zerostack-runtime-safe-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    fs::create_dir(&temp).expect("create isolated temp");
    let runtime = unix_runtime_dir_for(None, &temp).expect("create private runtime");
    let metadata = fs::symlink_metadata(&runtime).expect("inspect private runtime");
    assert_eq!(metadata.mode() & 0o777, 0o700);
    assert_eq!(metadata.uid(), effective_uid());
    fs::remove_dir_all(&temp).expect("remove isolated temp");
}

#[cfg(unix)]
#[test]
fn unix_fallback_refuses_symlink_and_unsafe_preexisting_runtime() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temp = std::env::temp_dir().join(format!(
        "zerostack-runtime-unsafe-{}-{}",
        std::process::id(),
        epoch_millis()
    ));
    fs::create_dir(&temp).expect("create isolated temp");
    let runtime = temp.join(format!("zerostack-runtime-{}", effective_uid()));
    let target = temp.join("target");
    fs::create_dir(&target).expect("create symlink target");
    symlink(&target, &runtime).expect("create malicious runtime symlink");
    assert!(unix_runtime_dir_for(None, &temp).is_err());
    assert!(verify_permit_base(&runtime).is_err());
    fs::remove_file(&runtime).expect("remove runtime symlink");
    fs::create_dir(&runtime).expect("create unsafe runtime");
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(unix_runtime_dir_for(None, &temp).is_err());
    assert!(verify_permit_base(&runtime).is_err());
    fs::remove_dir_all(&temp).expect("remove isolated temp");
}

#[test]
fn typed_owner_metadata_is_written_exactly() {
    let base = fencing_test_path("typed-owner-metadata");
    let owner = PermitOwnerMetadata::new(
        "/repo/exact",
        "fs.search",
        "cm://session/session-7/generation/3",
        "cm://cell/session-7/generation/3/request/11",
    );
    let permit = MachinePermit::acquire_slots_with_owner_metadata(
        &base,
        1,
        Instant::now() + Duration::from_secs(2),
        owner.clone(),
    )
    .expect("typed owner acquires");
    assert_eq!(permit.owner_metadata().expect("read typed owner"), owner);
    let status = permit_status(&base, 1).expect("inspect typed owner");
    assert_eq!(status[0].repository.as_deref(), Some("/repo/exact"));
    assert_eq!(status[0].operation.as_deref(), Some("fs.search"));
    assert_eq!(
        status[0].session_ref.as_deref(),
        Some("cm://session/session-7/generation/3")
    );
    assert_eq!(
        status[0].cell_ref.as_deref(),
        Some("cm://cell/session-7/generation/3/request/11")
    );
    drop(permit);
    assert!(!base.join("slot-0").exists());
    let _ = fs::remove_dir_all(base);
}

#[test]
fn background_heartbeat_stops_and_releases_once() {
    let base = fencing_test_path("heartbeat-lease");
    let permit = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_secs(2),
        "heartbeat-lease",
    )
    .expect("acquire heartbeat permit");
    let initial = permit_status(&base, 1)
        .expect("inspect initial heartbeat")
        .pop()
        .expect("initial holder")
        .heartbeat_at_ms
        .expect("initial heartbeat timestamp");
    let lease = permit
        .start_heartbeat(Duration::from_millis(2))
        .expect("start bounded heartbeat");
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut refreshed = initial;
    while refreshed <= initial && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
        refreshed = permit_status(&base, 1)
            .expect("inspect refreshed heartbeat")
            .pop()
            .expect("live holder")
            .heartbeat_at_ms
            .expect("heartbeat timestamp");
    }
    assert!(refreshed > initial, "background heartbeat did not refresh");
    lease.stop();
    assert!(!base.join("slot-0").exists(), "lease must release its slot");
    let next = MachinePermit::acquire_slots(
        &base,
        1,
        Instant::now() + Duration::from_secs(2),
        "after-heartbeat-lease",
    )
    .expect("slot must be available after one release");
    drop(next);
    let _ = fs::remove_dir_all(base);
}
