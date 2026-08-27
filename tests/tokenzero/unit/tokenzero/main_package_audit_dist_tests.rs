    use super::collect_package_audit_artifacts;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn default_dot_dist_keeps_workspace_packaging_defaults() {
        let artifacts = collect_package_audit_artifacts(Path::new(".")).unwrap();
        assert!(
            artifacts.is_empty(),
            "'.' is the documented fallback to workspace packaging files: {artifacts:?}"
        );
    }

    #[test]
    fn missing_dist_fails_loud_instead_of_auditing_defaults() {
        let error = collect_package_audit_artifacts(Path::new(
            "/definitely/not/a/tokenzero-package-audit-dist",
        ))
        .expect_err("missing --dist must not fall through to Cargo.toml defaults");
        let message = error.to_string();
        assert!(message.contains("does not exist"), "{message}");
        assert!(message.contains("--dist"), "{message}");
    }

    #[test]
    fn empty_dist_directory_fails_loud_instead_of_auditing_defaults() {
        let temp = tempdir().unwrap();
        let error = collect_package_audit_artifacts(temp.path())
            .expect_err("empty --dist must not report ok against workspace defaults");
        let message = error.to_string();
        assert!(message.contains("contains no artifacts"), "{message}");
    }

    #[test]
    fn file_dist_is_collected() {
        let temp = tempdir().unwrap();
        let file = temp.path().join("tokenzero.tar.gz");
        std::fs::write(&file, b"not-a-real-archive").unwrap();
        let artifacts = collect_package_audit_artifacts(&file).unwrap();
        assert_eq!(artifacts, vec![file]);
    }

