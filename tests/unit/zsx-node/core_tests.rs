    use super::*;
    use std::time::Instant;

    #[test]
    fn host_ceiling_is_800ms_between_core_budget_and_host_deadline() {
        assert_eq!(HOST_SHUTDOWN_HOOK_CEILING_MS, 800);
        assert!(HOST_SHUTDOWN_HOOK_CEILING_MS > zsx_core::DEFAULT_SHUTDOWN_WAIT_MS);
        assert!(HOST_SHUTDOWN_HOOK_CEILING_MS < 2000);
    }

    #[test]
    fn host_ceiling_returns_fast_path_value() {
        let started = Instant::now();
        let outcome = run_with_host_ceiling(Duration::from_millis(HOST_SHUTDOWN_HOOK_CEILING_MS), || 42u64);
        assert_eq!(outcome, HostCeiling::Ready(42));
        assert!(started.elapsed() < Duration::from_millis(400));
    }

    #[test]
    fn host_ceiling_settles_when_op_stalls() {
        let (release_tx, release_rx) = mpsc::sync_channel::<()>(1);
        let started = Instant::now();
        let outcome = run_with_host_ceiling(
            Duration::from_millis(HOST_SHUTDOWN_HOOK_CEILING_MS),
            move || {
                let _ = release_rx.recv();
                1u64
            },
        );
        let elapsed = started.elapsed();
        drop(release_tx);
        assert_eq!(outcome, HostCeiling::TimedOut);
        assert!(
            elapsed >= Duration::from_millis(750),
            "ceiling fired too early: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_millis(1200),
            "hook stayed pending past 800ms slack: {elapsed:?}"
        );
    }

