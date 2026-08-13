    use super::*;
    use tempfile::tempdir;

    fn temps_in(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp-"))
            .collect()
    }

    #[test]
    fn atomic_write_publishes_bytes_and_leaves_no_temp() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("artifact.json");
        atomic_write_file(&dest, b"first").unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"first");
        assert!(temps_in(dir.path()).is_empty(), "temp must not survive");
    }

    #[test]
    fn atomic_write_replaces_existing_contents_whole() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("artifact.json");
        atomic_write_file(&dest, b"first").unwrap();
        atomic_write_file(&dest, b"second-and-longer").unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"second-and-longer");
        assert!(temps_in(dir.path()).is_empty());
    }

    #[test]
    fn atomic_write_creates_missing_parents() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("a").join("b").join("artifact.json");
        atomic_write_file(&dest, b"nested").unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"nested");
    }

    #[test]
    fn a_failed_publish_removes_our_temp() {
        let dir = tempdir().unwrap();
        // A directory at dest makes the rename fail on every platform.
        let dest = dir.path().join("artifact.json");
        fs::create_dir(&dest).unwrap();
        assert!(atomic_write_file(&dest, b"bytes").is_err());
        assert!(
            temps_in(dir.path()).is_empty(),
            "the temp we created must be cleaned up on failure"
        );
    }

    #[test]
    fn a_blocked_parent_is_reported_and_writes_nothing() {
        let dir = tempdir().unwrap();
        let blocker = dir.path().join("parent");
        fs::write(&blocker, b"not a directory").unwrap();
        let dest = blocker.join("artifact.json");
        assert!(atomic_write_file(&dest, b"bytes").is_err());
        assert_eq!(fs::read(&blocker).unwrap(), b"not a directory");
    }

    #[test]
    fn replace_file_moves_the_temp_exactly_once() {
        let dir = tempdir().unwrap();
        let tmp = dir.path().join("tmp");
        let dest = dir.path().join("dest");
        fs::write(&tmp, b"payload").unwrap();
        replace_file(&tmp, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"payload");
        assert!(!tmp.exists());
        assert!(replace_file(&tmp, &dest).is_err(), "temp is gone");
    }

    #[test]
    fn sync_dir_reports_a_bad_directory() {
        let dir = tempdir().unwrap();
        assert!(sync_dir(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn never_policy_still_publishes_whole_bytes() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("artifact.json");
        atomic_write_file_with_sync(&dest, b"unsynced", SyncPolicy::Never).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"unsynced");
        assert!(temps_in(dir.path()).is_empty());
    }

    #[test]
    fn tolerate_unsupported_absorbs_permission_denied_only() {
        let denied = io::Error::new(io::ErrorKind::PermissionDenied, "eperm");
        assert!(sync_unsupported(&denied));
        assert!(tolerate_unsupported_sync(Err(denied)).is_ok());

        let nospace = io::Error::new(io::ErrorKind::StorageFull, "enospc");
        assert!(!sync_unsupported(&nospace));
        assert!(tolerate_unsupported_sync(Err(nospace)).is_err());
    }
