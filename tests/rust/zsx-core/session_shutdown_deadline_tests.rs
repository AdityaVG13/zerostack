    use super::*;
    use crate::{AdapterBinding, AdapterCall, AdapterError, AdapterResponse, DomainAdapter};
    use std::sync::Arc;
    use std::time::Instant;
    use zero_abi::EngineIdentity;

    struct StubAdapter {
        engine: EngineIdentity,
        scheme: &'static str,
    }

    impl DomainAdapter for StubAdapter {
        fn engine(&self) -> EngineIdentity {
            self.engine
        }

        fn binding(&self) -> AdapterBinding {
            AdapterBinding::new(
                self.engine,
                "test",
                "test.v1",
                "a".repeat(64),
                "b".repeat(64),
                self.scheme,
            )
            .expect("stub binding")
        }

        fn call(&self, _call: AdapterCall<'_>) -> Result<AdapterResponse, AdapterError> {
            Err(AdapterError::new("internal", "stub unused", false, None))
        }
    }

    fn temp_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "zsx-1i7h-{}-{}",
            std::process::id(),
            SESSION_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        root
    }

    fn stub_session(root: &PathBuf) -> ZsxSession {
        ZsxSession::builder(root)
            .with_session_id("zsx-1i7h-deadline")
            .fszero(Arc::new(StubAdapter {
                engine: EngineIdentity::FsZero,
                scheme: "fz://",
            }))
            .graphzero(Arc::new(StubAdapter {
                engine: EngineIdentity::GraphZero,
                scheme: "gz://",
            }))
            .tokenzero(Arc::new(StubAdapter {
                engine: EngineIdentity::TokenZero,
                scheme: "tz://",
            }))
            .build()
            .expect("stub session")
    }

    #[test]
    fn shutdown_wait_is_500ms_and_below_host_deadlines() {
        assert_eq!(DEFAULT_SHUTDOWN_WAIT_MS, 500);
        assert_eq!(
            SESSION_SHUTDOWN_SETTLE_TIMEOUT,
            Duration::from_millis(DEFAULT_SHUTDOWN_WAIT_MS)
        );
        const HOST_HOOK_CEILING_MS: u64 = 800;
        const HOST_EXIT_DEADLINE_MS: u64 = 2000;
        assert!(DEFAULT_SHUTDOWN_WAIT_MS < HOST_HOOK_CEILING_MS);
        assert!(HOST_HOOK_CEILING_MS < HOST_EXIT_DEADLINE_MS);
        assert!(DEFAULT_SHUTDOWN_WAIT_MS < HOST_EXIT_DEADLINE_MS);
    }

    #[test]
    fn shutdown_returns_generation_within_shared_budget_and_is_idempotent() {
        let root = temp_root();
        let session = stub_session(&root);
        let started = Instant::now();
        let generation = session.shutdown().expect("shutdown");
        let elapsed = started.elapsed();
        assert_eq!(generation, 1);
        assert!(
            elapsed < Duration::from_millis(DEFAULT_SHUTDOWN_WAIT_MS),
            "core shutdown exceeded shared {DEFAULT_SHUTDOWN_WAIT_MS}ms budget: {elapsed:?}"
        );
        let again = session.shutdown().expect("idempotent shutdown");
        assert_eq!(again, generation);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn join_thread_within_zero_budget_does_not_block() {
        let (release_tx, release_rx) = mpsc::sync_channel::<()>(1);
        let handle = thread::spawn(move || {
            let _ = release_rx.recv();
        });
        let started = Instant::now();
        let result = join_thread_within(handle, Duration::ZERO);
        let elapsed = started.elapsed();
        drop(release_tx);
        assert!(result.is_err(), "zero budget must park, not join: {result:?}");
        assert!(
            elapsed < Duration::from_millis(80),
            "zero-budget join blocked the host: {elapsed:?}"
        );
    }

    #[test]
    fn second_shutdown_while_joining_is_error_not_ok() {
        let (release_tx, release_rx) = mpsc::sync_channel::<()>(1);
        let handle = thread::spawn(move || {
            let _ = release_rx.recv();
        });
        let err = join_thread_within(handle, Duration::ZERO).expect_err("parked");
        assert!(
            err.contains("did not join"),
            "expected park timeout, got {err}"
        );
        drop(release_tx);
    }

    #[test]
    fn parked_join_completion_is_observable() {
        let (release_tx, release_rx) = mpsc::sync_channel::<()>(1);
        let handle = thread::spawn(move || {
            let _ = release_rx.recv();
        });
        let parked = join_thread_within_parked(handle, Duration::ZERO).expect_err("park");
        drop(release_tx);
        parked
            .done
            .recv_timeout(Duration::from_millis(200))
            .expect("watcher must report join after worker unblocks");
    }

    #[test]
    fn parked_join_second_waiter_sees_consumed_watch() {
        let root = temp_root();
        let session = stub_session(&root);
        let (release_tx, release_rx) = mpsc::sync_channel::<()>(1);
        let handle = thread::spawn(move || {
            let _ = release_rx.recv();
        });
        let parked = join_thread_within_parked(handle, Duration::ZERO).expect_err("park");
        session.test_install_parked_watch(parked.done);
        drop(release_tx);
        let deadline = Instant::now() + Duration::from_millis(200);
        let mut first = false;
        while Instant::now() < deadline {
            first = session.finish_parked_join();
            if first {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(first, "watcher must become readable");
        assert!(
            session.finish_parked_join(),
            "second waiter must observe join_done after try_recv is consumed"
        );
        let again = session.shutdown().expect("idempotent after join_done");
        assert_eq!(again, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn parked_join_does_not_steal_in_flight_handle() {
        let root = temp_root();
        let session = stub_session(&root);
        assert!(session.test_has_worker(), "stub starts with a worker");
        session.test_mark_join_in_flight();
        assert!(
            !session.finish_parked_join(),
            "in-flight first shutdown still owns the join"
        );
        assert!(
            session.test_has_worker(),
            "second waiter must not take the JoinHandle"
        );
        let _ = session.shutdown();
        let _ = std::fs::remove_dir_all(&root);
    }

