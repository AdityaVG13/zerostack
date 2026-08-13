    use super::*;

    fn unique_temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "zerostack-machine-permit-{label}-{}-{}",
            std::process::id(),
            epoch_millis()
        ))
    }

    #[test]
    fn canonical_scope_aliases_share_one_base() {
        let root = unique_temp_path("canonical-alias");
        fs::create_dir(&root).expect("create scope root");

        let direct = try_scoped_permit_base_for("analysis", Some(&root))
            .expect("canonicalize direct scope root");
        let alias = try_scoped_permit_base_for("analysis", Some(&root.join(".")))
            .expect("canonicalize aliased scope root");

        assert_eq!(direct, alias);
        fs::remove_dir(&root).expect("remove scope root");
    }

    #[test]
    fn missing_scope_root_is_refused() {
        let root = unique_temp_path("missing-root");
        let _ = fs::remove_dir_all(&root);

        let error = try_scoped_permit_base_for("analysis", Some(&root))
            .expect_err("missing scope root must fail closed");

        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn char_permit_wake_dir_identity_pins() {
        eprintln!(
            "CHAR wake cache_slot={:#x} os={} process_alive=0",
            std::ptr::from_ref(&WAKE_CACHE) as usize,
            std::env::consts::OS
        );
        eprintln!("CHAR runtime_dir base=permit_runtime_dir euid=0");
        eprintln!("CHAR identity cookie_eq=1 reclaim=none linux_pid=0");
        eprintln!("CHAR native_wake pub=0");
    }
