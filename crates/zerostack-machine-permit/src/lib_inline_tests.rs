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
            assert!(Instant::now() < deadline, "expected {expected} waiter intents");
            thread::sleep(Duration::from_millis(1));
        }
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

    #[cfg(unix)]
    #[test]
    fn native_pid_liveness_handles_alive_dead_and_conservative_errors() {
        assert!(process_alive(std::process::id()), "current process must be alive");

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
        let a = MachinePermit::acquire_slots(
            &base,
            2,
            Instant::now() + Duration::from_secs(2),
            "slot-a",
        )
        .expect("first slot");
        let b = MachinePermit::acquire_slots(
            &base,
            2,
            Instant::now() + Duration::from_secs(2),
            "slot-b",
        )
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

    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos", windows))]
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
        assert!(after.is_ok(), "slots acquire after legacy release: {after:?}");
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
                    message.contains("create codemode permit"),
                    "unexpected Fatal message: {message}"
                );
            }
            AcquireError::Busy(message) => {
                panic!("I/O failure must be Fatal, not Busy: {message}")
            }
        }
    }
