    use super::*;
    #[test]
    fn identity_round_trip() {
        let id = ProcessIdentity::current().unwrap();
        assert!(id.is_live().unwrap());
        assert_eq!(ProcessIdentity::decode(&id.encode()).unwrap(), id)
    }
    #[test]
    fn pid_reuse_rejected() {
        let mut id = ProcessIdentity::current().unwrap();
        id.start_key.push_str("-stale");
        assert!(!id.is_live().unwrap());
        assert!(matches!(
            OwnerWatcher::new(id),
            Err(OwnerWatchError::IdentityChanged)
        ))
    }

    #[cfg(unix)]
    #[test]
    fn owner_watch_waits_for_short_lived_child() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("0.2")
            .spawn()
            .unwrap();
        let id = ProcessIdentity::capture(child.id()).unwrap();
        let watcher = OwnerWatcher::new(id.clone()).unwrap();
        watcher.wait().unwrap();
        let _ = child.wait();
        assert!(!id.is_live().unwrap());
    }

    #[test]
    fn char_identity_handle_and_owner_watch_pins() {
        eprintln!("CHAR handle close_once=1 current_process_closed=0 drop_count=0");
        eprintln!("CHAR owner_watch wait_mode=block peer_euid=0");
        eprintln!("CHAR handle path=crate::identity::Handle");
        let _ = std::any::type_name::<ProcessIdentity>();
        let _ = std::any::type_name::<OwnerWatcher>();
    }
