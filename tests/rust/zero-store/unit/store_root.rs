    use super::*;
    use std::ffi::OsString;
    use tempfile::TempDir;

    #[cfg(unix)]
    #[test]
    fn lexical_normalize_keeps_absolute_paths_at_root() {
        assert_eq!(
            lexical_normalize(Path::new("/../../project")),
            Path::new("/project")
        );
        assert_eq!(
            lexical_normalize(Path::new("/project/../../..")),
            Path::new("/")
        );
    }

    #[test]
    fn lexical_normalize_collapses_interior_components() {
        assert_eq!(
            lexical_normalize(Path::new("alpha/beta/../gamma/./delta")),
            Path::new("alpha/gamma/delta")
        );
        assert_eq!(
            lexical_normalize(Path::new("../../alpha")),
            Path::new("../../alpha")
        );
    }

    #[cfg(unix)]
    #[test]
    fn lexical_normalize_equivalent_identity_paths_match() {
        assert_eq!(
            lexical_normalize(Path::new("/workspace/project/../project")),
            lexical_normalize(Path::new("/workspace/project"))
        );
    }

    const ENGINES: [Engine; 3] = [Engine::TokenZero, Engine::FsZero, Engine::GraphZero];

    fn env_of(pin: Option<&Path>, opt_in: bool) -> StoreEnv {
        StoreEnv::new(pin.map(OsString::from), opt_in)
    }

    fn env_raw(pin: Option<&str>, opt_in: bool) -> StoreEnv {
        StoreEnv::new(pin.map(OsString::from), opt_in)
    }

    /// Golden row 1: nothing set resolves to the legacy per-repo directory.
    #[test]
    fn no_store_resolves_legacy() {
        let repo = TempDir::new().unwrap();
        for engine in ENGINES {
            let r = ResolvedStore::resolve(repo.path(), engine, &StoreEnv::default());
            assert_eq!(r.mode(), StoreMode::Legacy);
            assert_eq!(r.engine_dir(), r.repo_root().join(engine.legacy_dir_name()));
            assert!(r.unified_root().is_none());
            assert!(r.project_key().is_none());
        }
    }

    /// Golden row 2: a project-local store wins with no environment at all.
    #[test]
    fn local_zerostack_resolves_unified() {
        let repo = TempDir::new().unwrap();
        std::fs::create_dir(repo.path().join(LOCAL_STORE_DIR)).unwrap();
        for engine in ENGINES {
            let r = ResolvedStore::resolve(repo.path(), engine, &StoreEnv::default());
            assert_eq!(r.mode(), StoreMode::LocalUnified);
            assert_eq!(
                r.engine_dir(),
                r.repo_root().join(LOCAL_STORE_DIR).join(engine.dir_name())
            );
        }
    }

    /// Golden rows 3 and 8: a pin without opt-in is ignored, absolute or
    /// relative. This is the FSZero divergence (zerostack-pi1).
    #[test]
    fn pin_without_opt_in_is_ignored() {
        let repo = TempDir::new().unwrap();
        let shared = TempDir::new().unwrap();
        for engine in ENGINES {
            let abs =
                ResolvedStore::resolve(repo.path(), engine, &env_of(Some(shared.path()), false));
            assert_eq!(abs.mode(), StoreMode::Legacy);
            assert_eq!(
                abs.engine_dir(),
                abs.repo_root().join(engine.legacy_dir_name())
            );
            assert!(abs.pin_set(), "pin is still reported, just not honored");

            let rel =
                ResolvedStore::resolve(repo.path(), engine, &env_raw(Some("sub-store"), false));
            assert_eq!(rel.mode(), StoreMode::Legacy);
        }
    }

    /// Golden row 4: an opted-in pin outside the project is namespaced.
    #[test]
    fn opted_in_external_pin_is_project_namespaced() {
        let repo = TempDir::new().unwrap();
        let shared = TempDir::new().unwrap();
        let key = project_key(repo.path());
        for engine in ENGINES {
            let r = ResolvedStore::resolve(repo.path(), engine, &env_of(Some(shared.path()), true));
            assert_eq!(r.mode(), StoreMode::SharedNamespaced);
            assert_eq!(r.project_key(), Some(key.as_str()));
            assert_eq!(
                r.engine_dir(),
                absolutize(shared.path())
                    .join(PROJECTS_DIR)
                    .join(&key)
                    .join(engine.dir_name())
            );
        }
    }

    /// Golden rows 5 and 6: a project-local store outranks any pin.
    #[test]
    fn local_store_outranks_pin_with_and_without_opt_in() {
        let repo = TempDir::new().unwrap();
        std::fs::create_dir(repo.path().join(LOCAL_STORE_DIR)).unwrap();
        let shared = TempDir::new().unwrap();
        for opt_in in [true, false] {
            for engine in ENGINES {
                let r = ResolvedStore::resolve(
                    repo.path(),
                    engine,
                    &env_of(Some(shared.path()), opt_in),
                );
                assert_eq!(r.mode(), StoreMode::LocalUnified);
                assert_eq!(
                    r.engine_dir(),
                    r.repo_root().join(LOCAL_STORE_DIR).join(engine.dir_name())
                );
            }
        }
    }

    /// Golden row 7: a pin inside the project is not namespaced, because
    /// there is exactly one project under it.
    #[test]
    fn opted_in_internal_pin_is_not_namespaced() {
        let repo = TempDir::new().unwrap();
        let r = ResolvedStore::resolve(
            repo.path(),
            Engine::FsZero,
            &env_raw(Some("sub-store"), true),
        );
        assert_eq!(r.mode(), StoreMode::PinnedInsideProject);
        assert!(r.project_key().is_none());
        assert_eq!(
            r.engine_dir(),
            r.repo_root()
                .join("sub-store")
                .join(Engine::FsZero.dir_name())
        );
    }

    /// Golden row 9: an exported-but-empty pin means unset, not the repo root.
    /// This is the FSZero empty-pin bug (fszero-44nj).
    #[test]
    fn empty_pin_is_unset() {
        let repo = TempDir::new().unwrap();
        for engine in ENGINES {
            let r = ResolvedStore::resolve(repo.path(), engine, &env_raw(Some(""), true));
            assert_eq!(r.mode(), StoreMode::Legacy);
            assert!(!r.pin_set());
            assert_eq!(r.engine_dir(), r.repo_root().join(engine.legacy_dir_name()));
            assert_ne!(r.engine_dir(), r.repo_root().join(engine.dir_name()));
        }
    }

    /// Golden row 10: a pin that does not exist still resolves. A store may
    /// be pinned before it is created.
    #[test]
    fn nonexistent_pin_resolves_without_error() {
        let repo = TempDir::new().unwrap();
        let shared = TempDir::new().unwrap();
        let missing = shared.path().join("does-not-exist").join("store");
        let r = ResolvedStore::resolve(
            repo.path(),
            Engine::GraphZero,
            &env_of(Some(&missing), true),
        );
        assert_eq!(r.mode(), StoreMode::SharedNamespaced);
        assert_eq!(
            r.engine_dir(),
            missing
                .join(PROJECTS_DIR)
                .join(project_key(repo.path()))
                .join("graphzero")
        );
    }

    /// Golden row 17: the anti-collision gate. Two distinct projects sharing
    /// one pin must never resolve to the same engine directory. This is what
    /// TokenZero gets wrong today (zerostack-ljx).
    #[test]
    fn distinct_projects_never_collide_under_one_pin() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        let shared = TempDir::new().unwrap();
        for engine in ENGINES {
            let ra = ResolvedStore::resolve(a.path(), engine, &env_of(Some(shared.path()), true));
            let rb = ResolvedStore::resolve(b.path(), engine, &env_of(Some(shared.path()), true));
            assert_ne!(ra.engine_dir(), rb.engine_dir());
            assert_ne!(ra.project_key(), rb.project_key());
        }
    }

    /// Golden row 18: all three engines must agree on the project key, or one
    /// cannot find another's shard. FSZero's `sid-` prefix broke this.
    #[test]
    fn all_engines_share_one_project_key() {
        let repo = TempDir::new().unwrap();
        let shared = TempDir::new().unwrap();
        let keys: Vec<String> = ENGINES
            .iter()
            .map(|e| {
                ResolvedStore::resolve(repo.path(), *e, &env_of(Some(shared.path()), true))
                    .project_key()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(keys[0], keys[1]);
        assert_eq!(keys[1], keys[2]);
        assert_eq!(keys[0].len(), PROJECT_KEY_HEX_LEN);
        assert!(
            keys[0]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    }

    /// Golden rows 19 and 20: the CAS root is shared per store root and is
    /// never project-namespaced, so a blob published by one engine is found
    /// by the others at the same path.
    #[test]
    fn cas_is_shared_per_store_root() {
        let repo = TempDir::new().unwrap();
        let shared = TempDir::new().unwrap();
        for engine in ENGINES {
            let r = ResolvedStore::resolve(repo.path(), engine, &env_of(Some(shared.path()), true));
            assert_eq!(r.cas_host(), absolutize(shared.path()));
            assert_eq!(r.blobs_dir(), absolutize(shared.path()).join(BLOBS_DIR));
            assert!(!r.blobs_dir().to_string_lossy().contains(PROJECTS_DIR));
        }
        std::fs::create_dir(repo.path().join(LOCAL_STORE_DIR)).unwrap();
        for engine in ENGINES {
            let r = ResolvedStore::resolve(repo.path(), engine, &StoreEnv::default());
            assert_eq!(r.cas_host(), r.repo_root().join(LOCAL_STORE_DIR));
            assert_eq!(
                r.blobs_dir(),
                r.repo_root().join(LOCAL_STORE_DIR).join(BLOBS_DIR)
            );
        }
    }

    /// Golden row 21: a root that does not exist yet must hash the same
    /// relative and absolute, or engines disagree about its shard.
    #[test]
    fn project_key_is_spelling_stable_for_missing_roots() {
        let cwd = std::env::current_dir().unwrap();
        let missing = "zero-store-does-not-exist-xyz";
        assert_eq!(
            project_key(Path::new(missing)),
            project_key(&cwd.join(missing))
        );
        assert_eq!(
            project_key(Path::new("./a/../b")),
            project_key(&cwd.join("b"))
        );
    }

    /// Golden row 22: `R` and `R/../<basename>` resolve identically.
    #[test]
    fn resolution_is_spelling_stable() {
        let repo = TempDir::new().unwrap();
        let base = repo.path().file_name().unwrap();
        let indirect = repo.path().join("..").join(base);
        let direct = ResolvedStore::resolve(repo.path(), Engine::TokenZero, &StoreEnv::default());
        let round = ResolvedStore::resolve(&indirect, Engine::TokenZero, &StoreEnv::default());
        assert_eq!(direct, round);
    }

    /// Golden rows 15 and 16: truthiness parity across every engine alias.
    #[test]
    fn opt_in_truthiness_parity() {
        for truthy in ["1", "on", "true", "yes", " 1 ", "TRUE", "Yes"] {
            assert!(
                StoreEnv::is_truthy(OsStr::new(truthy)),
                "expected truthy: {truthy:?}"
            );
        }
        for falsy in ["0", "off", "false", "no", "", "maybe", "2"] {
            assert!(
                !StoreEnv::is_truthy(OsStr::new(falsy)),
                "expected falsy: {falsy:?}"
            );
        }
    }

    /// Golden row 23: an ignored pin must be explained, not silently dropped.
    #[test]
    fn ignored_pin_is_reported() {
        let repo = TempDir::new().unwrap();
        let shared = TempDir::new().unwrap();
        let env = env_of(Some(shared.path()), false);
        let report = ResolvedStore::resolve(repo.path(), Engine::TokenZero, &env).report(&env);
        assert_eq!(report.mode, StoreMode::Legacy);
        assert_eq!(report.schema_version, STORE_RESOLUTION_SCHEMA);
        assert!(
            report.warnings.iter().any(|w| w.contains("pin ignored")),
            "warnings: {:?}",
            report.warnings
        );

        std::fs::create_dir(repo.path().join(LOCAL_STORE_DIR)).unwrap();
        let env = env_of(Some(shared.path()), true);
        let report = ResolvedStore::resolve(repo.path(), Engine::TokenZero, &env).report(&env);
        assert_eq!(report.mode, StoreMode::LocalUnified);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("takes precedence"))
        );
    }

    /// Opt-in with nothing pinned is a misconfiguration worth surfacing.
    #[test]
    fn opt_in_without_pin_is_reported() {
        let repo = TempDir::new().unwrap();
        let env = env_of(None, true);
        let report = ResolvedStore::resolve(repo.path(), Engine::FsZero, &env).report(&env);
        assert_eq!(report.mode, StoreMode::Legacy);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("no store root pinned"))
        );
    }

    /// Golden row 25: `.zerostack` as a regular file is not a store.
    #[test]
    fn zerostack_file_is_not_a_store() {
        let repo = TempDir::new().unwrap();
        std::fs::write(repo.path().join(LOCAL_STORE_DIR), b"not a dir").unwrap();
        let r = ResolvedStore::resolve(repo.path(), Engine::FsZero, &StoreEnv::default());
        assert_eq!(r.mode(), StoreMode::Legacy);
    }

    /// Golden row 26: a lookalike directory name is not a store.
    #[test]
    fn lookalike_directory_is_not_a_store() {
        let repo = TempDir::new().unwrap();
        std::fs::create_dir(repo.path().join(".zerostack-old")).unwrap();
        let r = ResolvedStore::resolve(repo.path(), Engine::GraphZero, &StoreEnv::default());
        assert_eq!(r.mode(), StoreMode::Legacy);
    }

    #[test]
    fn engine_dir_names_are_distinct_and_stable() {
        assert_eq!(Engine::TokenZero.dir_name(), "tokenzero");
        assert_eq!(Engine::FsZero.dir_name(), "fszero");
        assert_eq!(Engine::GraphZero.dir_name(), "graphzero");
        assert_eq!(Engine::TokenZero.legacy_dir_name(), ".tokenzero");
        assert_eq!(Engine::FsZero.legacy_dir_name(), ".fszero");
        assert_eq!(Engine::GraphZero.legacy_dir_name(), ".graphzero");
    }

    #[test]
    fn mode_labels_are_stable() {
        assert_eq!(StoreMode::LocalUnified.as_str(), "local_unified");
        assert_eq!(
            StoreMode::PinnedInsideProject.as_str(),
            "pinned_inside_project"
        );
        assert_eq!(StoreMode::SharedNamespaced.as_str(), "shared_namespaced");
        assert_eq!(StoreMode::Legacy.as_str(), "legacy");
    }

    #[test]
    fn ensure_layout_creates_engine_dir_only() {
        let repo = TempDir::new().unwrap();
        let shared = TempDir::new().unwrap();
        let r = ResolvedStore::resolve(
            repo.path(),
            Engine::FsZero,
            &env_of(Some(shared.path()), true),
        );
        ensure_layout(&r).unwrap();
        assert!(r.engine_dir().is_dir());
        assert!(
            !r.blobs_dir().exists(),
            "CAS directories are created on publish"
        );
    }

    #[test]
    fn engine_file_lands_in_engine_dir() {
        let repo = TempDir::new().unwrap();
        std::fs::create_dir(repo.path().join(LOCAL_STORE_DIR)).unwrap();
        let r = ResolvedStore::resolve(repo.path(), Engine::FsZero, &StoreEnv::default());
        assert_eq!(
            r.engine_file("store.sqlite3").unwrap(),
            r.repo_root()
                .join(LOCAL_STORE_DIR)
                .join("fszero")
                .join("store.sqlite3")
        );
    }

    #[test]
    fn containment_is_spelling_independent() {
        let repo = TempDir::new().unwrap();
        let inside = repo.path().join("sub");
        std::fs::create_dir(&inside).unwrap();
        assert!(store_is_under_project_root(&inside, repo.path()));
        assert!(store_is_under_project_root(
            &repo.path().join("./sub"),
            repo.path()
        ));
        let outside = TempDir::new().unwrap();
        assert!(!store_is_under_project_root(outside.path(), repo.path()));
    }

    #[test]
    fn engine_file_accepts_normal_and_dot_prefixed_basenames() {
        assert!(validate_engine_file_name("store.sqlite3").is_ok());
        assert!(validate_engine_file_name(".state.json").is_ok());
    }

    #[test]
    fn engine_file_rejects_non_basename_paths() {
        for name in [
            "",
            ".",
            "..",
            "/absolute",
            "nested/file",
            "nested\\file",
            "C:\\absolute",
            "C:relative",
        ] {
            assert!(
                validate_engine_file_name(name).is_err(),
                "accepted {name:?}"
            );
        }
    }

    #[test]
    fn a_symlinked_local_marker_is_refused() {
        let dir = TempDir::new().unwrap();
        let repo = dir.path().join("repo");
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, repo.join(LOCAL_STORE_DIR)).unwrap();

        let env = StoreEnv::new(None, false);
        let resolved = ResolvedStore::resolve(&repo, Engine::TokenZero, &env);
        assert_eq!(
            resolved.mode(),
            StoreMode::Legacy,
            "a symlinked marker must not be adopted as a local unified store"
        );
        assert!(resolved.unified_root().is_none());
        assert!(
            resolved
                .report(&env)
                .warnings
                .iter()
                .any(|w| w.contains("symlink"))
        );
        assert!(ensure_layout(&resolved).is_err());
    }

    #[test]
    fn a_real_local_marker_is_still_adopted() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(LOCAL_STORE_DIR)).unwrap();
        let env = StoreEnv::new(None, false);
        let resolved = ResolvedStore::resolve(dir.path(), Engine::TokenZero, &env);
        assert_eq!(resolved.mode(), StoreMode::LocalUnified);
        ensure_layout(&resolved).unwrap();
    }

    #[test]
    fn tilde_root_rejects_literal_first_component_without_creating_it() {
        for pin in ["~", "~/foo"] {
            let repo = TempDir::new().unwrap();
            let env = StoreEnv::new(Some(OsString::from(pin)), true);
            let resolved = ResolvedStore::resolve(repo.path(), Engine::TokenZero, &env);

            assert_eq!(resolved.mode(), StoreMode::Legacy);
            assert_eq!(resolved.engine_dir(), absolutize(repo.path()).join(".tokenzero"));
            assert!(
                resolved
                    .report(&env)
                    .warnings
                    .iter()
                    .any(|warning| warning.contains("literal '~'"))
            );
            let error = ensure_layout(&resolved).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains("literal '~'"));
            assert!(!repo.path().join("~").exists());
        }
    }

    #[test]
    fn tilde_root_allows_tilde_after_the_first_component() {
        for pin in ["safe/~", "safe~/store"] {
            let repo = TempDir::new().unwrap();
            let env = StoreEnv::new(Some(OsString::from(pin)), true);
            let resolved = ResolvedStore::resolve(repo.path(), Engine::GraphZero, &env);

            ensure_layout(&resolved).unwrap();
            assert!(repo.path().join(pin).is_dir());
        }
    }
