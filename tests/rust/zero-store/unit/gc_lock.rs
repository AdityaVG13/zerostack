    use super::*;
    use std::sync::mpsc;
    use tempfile::TempDir;

    #[test]
    fn lock_file_lands_in_the_gc_namespace() {
        let root = TempDir::new().unwrap();
        let guard = StoreLock::publish(root.path(), LOCK_DEADLINE).unwrap();
        assert_eq!(guard.path(), coordinator_lock_path(root.path()));
        assert!(guard.path().is_file());
        assert_eq!(guard.mode(), LockMode::Shared);
        assert!(!guard.is_exclusive());
    }

    #[test]
    fn many_publishers_share_the_lock() {
        let root = TempDir::new().unwrap();
        let a = StoreLock::publish(root.path(), LOCK_DEADLINE).unwrap();
        let b = StoreLock::publish(root.path(), LOCK_DEADLINE).unwrap();
        let c = StoreLock::try_publish(root.path()).unwrap();
        assert!(c.is_some(), "shared holders must not exclude each other");
        drop((a, b, c));
    }

    /// A sweep in progress must exclude a publisher. This is the property the
    /// TOCTOU depended on being absent.
    #[test]
    fn a_sweep_excludes_publishers() {
        let root = TempDir::new().unwrap();
        let sweep = StoreLock::try_sweep(root.path()).unwrap().expect("sweep");
        assert!(sweep.is_exclusive());
        assert!(
            StoreLock::try_publish(root.path()).unwrap().is_none(),
            "publish must not proceed during a sweep"
        );
        assert!(
            StoreLock::try_sweep(root.path()).unwrap().is_none(),
            "two sweeps must not run at once"
        );
        drop(sweep);
        assert!(StoreLock::try_publish(root.path()).unwrap().is_some());
    }

    #[test]
    fn a_publisher_excludes_a_sweep() {
        let root = TempDir::new().unwrap();
        let publish = StoreLock::publish(root.path(), LOCK_DEADLINE).unwrap();
        assert!(StoreLock::try_sweep(root.path()).unwrap().is_none());
        drop(publish);
        assert!(StoreLock::try_sweep(root.path()).unwrap().is_some());
    }

    /// Waiting is bounded, so a wedged holder surfaces as a typed timeout
    /// instead of hanging the caller forever.
    #[test]
    fn acquisition_is_deadline_bounded() {
        let root = TempDir::new().unwrap();
        let _sweep = StoreLock::try_sweep(root.path()).unwrap().expect("sweep");
        let started = Instant::now();
        let err = StoreLock::publish(root.path(), Duration::from_millis(120))
            .expect_err("must time out while a sweep holds the lock");
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
        assert!(started.elapsed() >= Duration::from_millis(120));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// Deterministic hand-off: no timing assumptions, only channel rendezvous.
    #[test]
    fn publisher_proceeds_exactly_when_the_sweep_releases() {
        let root = TempDir::new().unwrap();
        let path = root.path().to_path_buf();
        let (holding_tx, holding_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();

        let sweeper = std::thread::spawn(move || {
            let guard = StoreLock::sweep(&path, LOCK_DEADLINE).unwrap();
            holding_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(guard);
        });

        holding_rx.recv().unwrap();
        assert!(
            StoreLock::try_publish(root.path()).unwrap().is_none(),
            "sweep is holding the lock, so publish must be excluded"
        );
        release_tx.send(()).unwrap();
        sweeper.join().unwrap();
        assert!(
            StoreLock::publish(root.path(), LOCK_DEADLINE).is_ok(),
            "publish must succeed once the sweep releases"
        );
    }

    #[test]
    fn a_guard_remembers_its_store_root() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        let guard = StoreLock::publish(a.path(), LOCK_DEADLINE).unwrap();
        assert!(guard.is_for_store_root(a.path()));
        assert!(!guard.is_for_store_root(b.path()));
        let alias = a.path().join("..").join(a.path().file_name().unwrap());
        assert!(
            guard.is_for_store_root(&alias),
            "binding is spelling-independent"
        );
        assert_eq!(guard.store_root(), crate::store_root::absolutize(a.path()));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_gc_namespace_is_refused() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        symlink(outside.path(), root.path().join(GC_DIR)).unwrap();

        let error = StoreLock::publish(root.path(), LOCK_DEADLINE).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!outside.path().join(COORDINATOR_LOCK).exists());
    }
