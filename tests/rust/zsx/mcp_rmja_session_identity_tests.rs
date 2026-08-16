    use super::{load_next_request_id, mint_mcp_session_id, store_next_request_id};
    use std::path::PathBuf;

    #[test]
    fn mint_mcp_session_id_changes_across_calls() {
        let root = PathBuf::from("/tmp/zerostack-rmja-root");
        let first = mint_mcp_session_id(&root);
        let second = mint_mcp_session_id(&root);
        assert_ne!(
            first, second,
            "evicted MCP sessions must not reuse a session id"
        );
        assert!(first.starts_with("zsx-mcp-"), "{first}");
        assert!(second.starts_with("zsx-mcp-"), "{second}");
    }

    #[test]
    fn next_request_id_survives_recreate() {
        let dir = std::env::temp_dir().join(format!(
            "zerostack-rmja-req-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp state root");
        assert_eq!(load_next_request_id(&dir), 1);
        store_next_request_id(&dir, 7);
        assert_eq!(load_next_request_id(&dir), 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn persist_file_symlink_is_not_followed() {
        let dir = std::env::temp_dir().join(format!(
            "zerostack-volv-persist-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp state root");
        let victim = dir.join("victim");
        std::fs::write(&victim, b"secret\n").expect("victim");
        std::os::unix::fs::symlink(&victim, dir.join(super::NEXT_REQUEST_ID_FILE))
            .expect("persist symlink");

        assert_eq!(load_next_request_id(&dir), 1, "must not read through symlink");
        store_next_request_id(&dir, 9);
        assert_eq!(
            std::fs::read_to_string(&victim).expect("victim intact"),
            "secret\n",
            "store must not write through persist symlink"
        );
        assert!(
            std::fs::symlink_metadata(dir.join(super::NEXT_REQUEST_ID_FILE))
                .expect("persist meta")
                .file_type()
                .is_symlink()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn persist_state_root_symlink_is_not_followed() {
        let base = std::env::temp_dir().join(format!(
            "zerostack-volv-root-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let outside = base.join("outside");
        let workspace = base.join("workspace");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let planted = outside.join(super::NEXT_REQUEST_ID_FILE);
        std::fs::write(&planted, b"99\n").expect("planted persist");
        std::os::unix::fs::symlink(&outside, workspace.join(".zerostack"))
            .expect("state root symlink");

        let state_root = workspace.join(".zerostack");
        assert_eq!(load_next_request_id(&state_root), 1);
        store_next_request_id(&state_root, 3);
        assert_eq!(
            std::fs::read_to_string(&planted).expect("planted intact"),
            "99\n",
            "store must not follow a symlinked .zerostack"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

