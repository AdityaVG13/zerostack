    use super::*;
    use crate::adapter::{AdapterCall, AdapterError, AdapterResponse};
    use zero_abi::raw_worker::EngineIdentity;

    #[test]
    fn shutdown_settle_is_strictly_inside_a_two_second_host_deadline() {
        assert_eq!(DEFAULT_SHUTDOWN_WAIT_MS, 500);
        assert!(DEFAULT_SHUTDOWN_WAIT_MS < 2000);
        assert_eq!(SESSION_SHUTDOWN_SETTLE_TIMEOUT.as_millis(), 500);
        assert_eq!(SESSION_REPLACEMENT_SETTLE_TIMEOUT.as_millis(), 5000);
    }

    #[test]
    fn join_thread_within_fails_closed_when_the_budget_is_gone() {
        let handle = std::thread::spawn(|| std::thread::sleep(Duration::from_secs(5)));
        let started = Instant::now();
        let error = join_thread_within(handle, Duration::from_millis(20)).expect_err("must not hang");
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "bounded join leaked past the budget: {:?}",
            started.elapsed()
        );
        assert!(
            error.contains("did not join"),
            "unexpected join error: {error}"
        );
    }

    #[test]
    fn default_session_id_is_unique_per_builder() {
        let first = ZsxBuilder::new(PathBuf::from("/tmp/one"));
        let second = ZsxBuilder::new(PathBuf::from("/tmp/two"));
        assert_ne!(first.session_id, second.session_id);
    }

    #[test]
    fn session_approval_contract_is_bounded_and_replay_safe() {
        let root = "/tmp/approved-root";
        let now = now_ms();
        let grant = SessionApprovalGrantV1 {
            schema: crate::connector::SESSION_APPROVAL_SCHEMA.into(),
            grant_id: "grant-1".into(),
            engine: EngineIdentity::FsZero,
            root: root.into(),
            generation: 7,
            request_id: 9,
            operation: "fs.write".into(),
            effect: EffectClass::ApprovalRequiredMutation,
            authority_digest: "a".repeat(64),
            policy_digest: "b".repeat(64),
            issued_at_unix_ms: now.saturating_sub(1),
            expires_at_unix_ms: now.saturating_add(1_000),
        };
        let mut state = ZsxSessionState {
            generation: 7,
            accepting: true,
            replacing: false,
            terminating: false,
            shutdown_sent: false,
            worker_stopped: false,
            seen_request_ids: BTreeSet::new(),
            active_request_ids: BTreeSet::new(),
            root: root.into(),
            state_root: root.into(),
            consumed_approval_ids: BTreeSet::new(),
        };
        let ids = validate_session_approvals(&state, 7, 9, std::slice::from_ref(&grant))
            .expect("valid approval");
        state.consumed_approval_ids.extend(ids);
        assert_eq!(
            validate_session_approvals(&state, 7, 9, std::slice::from_ref(&grant))
                .unwrap_err()
                .code,
            ZsxSessionFailureCode::ApprovalReplay
        );

        let mut fresh_state = state;
        fresh_state.consumed_approval_ids.clear();
        let mut wrong_root = grant.clone();
        wrong_root.root = "/tmp/other-root".into();
        assert_eq!(
            validate_session_approvals(&fresh_state, 7, 9, &[wrong_root])
                .unwrap_err()
                .code,
            ZsxSessionFailureCode::InvalidApproval
        );
        let mut wrong_effect = grant.clone();
        wrong_effect.effect = EffectClass::ReadOnly;
        assert_eq!(
            validate_session_approvals(&fresh_state, 7, 9, &[wrong_effect])
                .unwrap_err()
                .code,
            ZsxSessionFailureCode::InvalidApproval
        );
        let mut expired = grant.clone();
        expired.issued_at_unix_ms = now.saturating_sub(2);
        expired.expires_at_unix_ms = now.saturating_sub(1);
        assert_eq!(
            validate_session_approvals(&fresh_state, 7, 9, &[expired])
                .unwrap_err()
                .code,
            ZsxSessionFailureCode::InvalidApproval
        );
        assert_eq!(
            validate_session_approvals(&fresh_state, 7, 9, &[grant.clone(), grant.clone()])
                .unwrap_err()
                .code,
            ZsxSessionFailureCode::ApprovalReplay
        );
        assert_eq!(
            validate_session_approvals(
                &fresh_state,
                7,
                9,
                &vec![grant.clone(); MAX_SESSION_APPROVAL_GRANTS + 1],
            )
            .unwrap_err()
            .code,
            ZsxSessionFailureCode::InvalidApproval
        );
    }

    /// In-process adapter that honors `__delay_ms` so a flood can occupy the
    /// class-agnostic session FIFO without pulling fixture-adapters.
    struct QueueFloodAdapter {
        engine: EngineIdentity,
        session_id: String,
    }

    impl QueueFloodAdapter {
        fn new(engine: EngineIdentity, session_id: &str) -> Self {
            Self {
                engine,
                session_id: session_id.to_owned(),
            }
        }
    }

    impl DomainAdapter for QueueFloodAdapter {
        fn engine(&self) -> EngineIdentity {
            self.engine
        }

        fn binding(&self) -> AdapterBinding {
            AdapterBinding::new(
                self.engine,
                "test-revision",
                "test.v1",
                "a".repeat(64),
                "b".repeat(64),
                match self.engine {
                    EngineIdentity::FsZero => "fz://",
                    EngineIdentity::GraphZero => "gz://",
                    EngineIdentity::TokenZero => "tz://",
                },
            )
            .expect("test binding is valid")
        }

        fn call(&self, call: AdapterCall<'_>) -> Result<AdapterResponse, AdapterError> {
            let request = call.request;
            if let Some(delay_ms) = request.args["__delay_ms"].as_u64() {
                let started = std::time::Instant::now();
                let budget = std::time::Duration::from_millis(delay_ms);
                while started.elapsed() < budget {
                    if call.cancellation.is_cancelled() {
                        return Err(AdapterError::new(
                            "cancelled",
                            "queue flood adapter cancelled during delay",
                            false,
                            Some(request.trace.clone()),
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
            Ok(AdapterResponse {
                result: zero_abi::WorkerResult {
                    value: serde_json::json!({ "ok": true }),
                    metadata: zero_abi::WorkerResultMetadata {
                        effect: EffectClass::ReadOnly,
                        approval: zero_abi::ApprovalMetadata {
                            state: zero_abi::ApprovalState::NotRequired,
                            approval_id: None,
                            policy: None,
                        },
                        revert: zero_abi::RevertMetadata {
                            supported: false,
                            journal_id: None,
                            rollback_op: None,
                        },
                        ownership: zero_abi::RefOwnership {
                            engine: self.engine,
                            session_id: self.session_id.clone(),
                            refs: Vec::new(),
                            snapshot: None,
                        },
                        trace: request.trace.clone(),
                    },
                },
                engine_timeline: None,
                worker_token_accounting: None,
            })
        }
    }

    #[test]
    fn heavy_execute_is_backpressured_under_analysis_flood() {
        use std::sync::Arc;
        use std::time::Duration;

        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir
            .path()
            .canonicalize()
            .unwrap_or_else(|_| dir.path().to_path_buf());
        let session_id = format!("zsx-heavy-flood-{:x}", std::process::id());
        let session = Arc::new(
            ZsxSession::builder(root)
                .with_session_id(session_id.clone())
                .fszero(Arc::new(QueueFloodAdapter::new(
                    EngineIdentity::FsZero,
                    &session_id,
                )))
                .graphzero(Arc::new(QueueFloodAdapter::new(
                    EngineIdentity::GraphZero,
                    &session_id,
                )))
                .tokenzero(Arc::new(QueueFloodAdapter::new(
                    EngineIdentity::TokenZero,
                    &session_id,
                )))
                .build()
                .expect("session builds"),
        );

        let flood = SESSION_EXECUTION_QUEUE_CAPACITY + 2;
        let mut joins = Vec::with_capacity(flood);
        for index in 0..flood {
            let worker = Arc::clone(&session);
            joins.push(std::thread::spawn(move || {
                worker.execute(
                    1,
                    (index as u64) + 1,
                    r#"await zero.fs.compound('search', {query: 'x', __delay_ms: 1500});"#,
                    Duration::from_secs(15),
                )
            }));
        }

        // Flood threads only need to pass try_send. Give them time to occupy
        // the worker + all 8 FIFO slots before the Heavy probe.
        std::thread::sleep(Duration::from_millis(400));

        let error = session
            .execute(
                1,
                10_000,
                r#"await zero.token.shell("fixture", {});"#,
                Duration::from_secs(2),
            )
            .expect_err("full FIFO must refuse Heavy-class token.shell");
        assert_eq!(error.code, ZsxSessionFailureCode::Backpressure);
        assert!(
            error.detail.contains("class-agnostic FIFO"),
            "Backpressure must name the honest FIFO law: {}",
            error.detail
        );

        session.cancellation().cancel();
        for join in joins {
            let _ = join.join();
        }
    }

    #[cfg(all(feature = "fszero", feature = "graphzero", feature = "tokenzero"))]
    #[test]
    fn build_canonical_refuses_unusable_durable_fszero_root() {
        let workspace = tempfile::tempdir().expect("workspace");
        let state_file = workspace.path().join("not-a-dir");
        std::fs::write(&state_file, b"x").expect("blocker file");
        let error = match ZsxSession::builder(workspace.path())
            .with_state_root(&state_file)
            .with_session_id("canonical-mkdir-fail")
            .build_canonical()
        {
            Ok(_) => panic!("canonical must refuse inert FSZero"),
            Err(error) => error,
        };
        assert_eq!(error.code, ZsxSessionFailureCode::BackendUnavailable);
        assert!(
            error.detail.contains("durable store unavailable"),
            "canonical refuse must name the durable failure: {}",
            error.detail
        );
    }
