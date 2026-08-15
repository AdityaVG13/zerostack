    use super::*;
    use zero_abi::raw_worker::EngineIdentity;
    use zero_store::{AttemptRecoveryOutcomeV1, mark_dispatch_crossed_v1};

    #[test]
    fn dispatch_permit_defaults_and_expand_exception_are_bounded() {
        assert_eq!(dispatch_permit_slots(DispatchPermitClass::Analysis, 1), 1);
        assert_eq!(dispatch_permit_slots(DispatchPermitClass::Analysis, 32), 8);
        assert_eq!(dispatch_permit_slots(DispatchPermitClass::Index, 1), 1);
        assert_eq!(dispatch_permit_slots(DispatchPermitClass::Index, 32), 2);
        assert_eq!(dispatch_permit_slots(DispatchPermitClass::Heavy, 128), 1);
        assert_eq!(
            dispatch_permit_class(EngineIdentity::TokenZero, "expand"),
            None
        );
        assert_eq!(
            dispatch_permit_class(EngineIdentity::FsZero, "fs.read"),
            None
        );
        assert_eq!(
            dispatch_permit_class(EngineIdentity::FsZero, "fs.search"),
            Some(DispatchPermitClass::Analysis)
        );
        assert_eq!(
            dispatch_permit_class(EngineIdentity::GraphZero, "index"),
            Some(DispatchPermitClass::Index)
        );
        for (engine, method) in [
            (EngineIdentity::TokenZero, "expand"),
            (EngineIdentity::FsZero, "fs.read"),
            (EngineIdentity::FsZero, "fs.search"),
            (EngineIdentity::GraphZero, "index"),
        ] {
            let class = dispatch_permit_class(engine, method);
            eprintln!("CHAR permit engine={engine:?} method={method} class={class:?}");
        }
        eprintln!("CHAR permit engine=FsZero method=fs.expand class=None");
        assert_eq!(
            dispatch_permit_class(EngineIdentity::FsZero, "fs.expand"),
            None
        );
    }

    #[test]
    fn connector_and_session_budgets_match_named_arrangements() {
        use std::time::Duration;
        use zero_codemode::OUTPUT_WALL_ARRANGEMENTS;

        let connector = OUTPUT_WALL_ARRANGEMENTS
            .iter()
            .find(|row| row.name == "zsx-connector-host")
            .expect("connector row");
        let limits = host_limits().expect("connector host_limits");
        assert_eq!(limits.memory_bytes, connector.memory_bytes);
        assert_eq!(limits.wall_timeout, Duration::from_millis(connector.wall_ms));
        assert_eq!(limits.max_json_bytes, connector.output_bytes);

        let session = OUTPUT_WALL_ARRANGEMENTS
            .iter()
            .find(|row| row.name == "zsx-session-visible")
            .expect("session row");
        assert_eq!(crate::session::SESSION_VISIBLE_RESULT_BYTES, session.output_bytes);
        assert_ne!(
            limits.max_json_bytes,
            crate::session::SESSION_VISIBLE_RESULT_BYTES,
            "connector 16 MiB json wall is not the 12 KiB session visible budget"
        );
    }

    #[test]
    fn char_connector_grants_and_now_ms_owner() {
        eprintln!(
            "CHAR grant schema={s} max_grants={n} max_lifetime_ms={ms}",
            s = SESSION_APPROVAL_SCHEMA,
            n = MAX_SESSION_APPROVAL_GRANTS,
            ms = MAX_SESSION_APPROVAL_LIFETIME_MS
        );
        eprintln!("CHAR approvals_mutex=1");
        let _ = now_ms();
        eprintln!("CHAR now_ms_owner=connector");
    }

    #[test]
    fn execution_context_refs_bind_generation_and_request() {
        let context = AggregateExecutionContext {
            generation: 7,
            request_id: 19,
        };
        assert_eq!(
            execution_session_ref("session-7", context),
            "cm://session/session-7/generation/7"
        );
        assert_eq!(
            execution_cell_ref("session-7", context),
            "cm://cell/session-7/generation/7/request/19"
        );
    }

    #[test]
    fn token_job_result_is_revalidated_at_the_aggregate_boundary() {
        let canonical = serde_json::json!({
            "id":"tzjob-7","status":"running","pid":42,"tail":"ok\n",
            "tailUtf8Lossless":true,"tailBytes":3,"logBytes":3,"cursor":3,
            "version":2,"changed":true,"nextPollMs":20000
        });
        assert_eq!(
            normalize_aggregate_result_value(
                EngineIdentity::TokenZero,
                zero_abi::TOKEN_JOB_OPERATION_V1,
                canonical.clone(),
            )
            .unwrap(),
            canonical
        );

        let mut unknown = canonical.clone();
        unknown["log"] = serde_json::json!("/private/session.log");
        assert!(
            normalize_aggregate_result_value(
                EngineIdentity::TokenZero,
                zero_abi::TOKEN_JOB_OPERATION_V1,
                unknown,
            )
            .is_err()
        );

        let mut false_exactness = canonical;
        false_exactness["tailBytes"] = serde_json::json!(2);
        false_exactness["cursor"] = serde_json::json!(2);
        false_exactness["logBytes"] = serde_json::json!(2);
        assert!(
            normalize_aggregate_result_value(
                EngineIdentity::TokenZero,
                zero_abi::TOKEN_JOB_OPERATION_V1,
                false_exactness,
            )
            .is_err()
        );
    }

    #[test]
    fn mutation_effect_class_covers_only_journaled_operations() {
        assert_eq!(
            mutation_effect_class(EngineIdentity::FsZero, "fs.edit"),
            Some(EffectClass::ReversibleMutation)
        );
        assert_eq!(
            mutation_effect_class(EngineIdentity::FsZero, "fs.write"),
            Some(EffectClass::ApprovalRequiredMutation)
        );
        assert_eq!(
            mutation_effect_class(EngineIdentity::GraphZero, "index"),
            Some(EffectClass::ReversibleMutation)
        );
        assert_eq!(
            mutation_effect_class(EngineIdentity::GraphZero, "remember"),
            Some(EffectClass::ReversibleMutation)
        );
        assert_eq!(
            mutation_effect_class(EngineIdentity::TokenZero, "ingest"),
            Some(EffectClass::Irreversible)
        );
        assert_eq!(
            mutation_effect_class(EngineIdentity::TokenZero, "shell"),
            Some(EffectClass::Irreversible)
        );
        for (engine, read) in [
            (EngineIdentity::FsZero, "fs.ls"),
            (EngineIdentity::FsZero, "fs.read"),
            (EngineIdentity::FsZero, "fs.search"),
            (EngineIdentity::GraphZero, "blast"),
            (EngineIdentity::GraphZero, "query"),
            (EngineIdentity::TokenZero, "expand"),
            (EngineIdentity::TokenZero, "job"),
        ] {
            assert_eq!(
                mutation_effect_class(engine, read),
                None,
                "{read} must not be journaled"
            );
        }
        assert_eq!(
            mutation_effect_class(EngineIdentity::TokenZero, "expand"),
            None
        );
    }

    fn journal_test_state(root: &Path, session_id: &str) -> ZsxState {
        ZsxState {
            adapters: BTreeMap::new(),
            engine_locks: [Mutex::new(()), Mutex::new(()), Mutex::new(())],
            workspace_root: root.to_path_buf(),
            state_root: root.to_path_buf(),
            session_id: session_id.to_owned(),
            reachable_blobs: Mutex::new(BTreeMap::new()),
            attempts_root: attempts_root_for(root),
            consumed_approval_grants: Mutex::new(BTreeSet::new()),
            engine_wall_ns: [const { AtomicU64::new(0) }; 3],
            engine_dispatches: [const { AtomicU64::new(0) }; 3],
            outstanding_dispatches: AtomicU64::new(0),
            verdict_meter: Mutex::new(None),
            resource_gauge: Mutex::new(None),
            residency_gate: Mutex::new(None),
            layer_validity: Mutex::new(LayerValidityLedgerV1::new()),
            residency_report: Mutex::new(None),
        }
    }

    fn test_call_request(id: &str, op: &str, args: Value) -> CallRequest {
        CallRequest {
            request_id: id.to_owned(),
            op: op.to_owned(),
            args,
            deadline_unix_ms: None,
            trace: WorkerTrace {
                runtime_id: "sess-journal".into(),
                cell_id: "cm://cell/sess-journal/generation/7/request/19".into(),
                request_id: id.to_owned(),
                trace_id: id.to_owned(),
                parent_span_id: None,
                worker_revision: "test".into(),
                contract_digest: "0".repeat(64),
            },
            approval_grant: None,
            telemetry_request: None,
        }
    }

    #[test]
    fn mutation_journal_prepare_is_durable_before_admission_and_cross_is_immediate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = journal_test_state(dir.path(), "sess-journal");
        let execution = AggregateExecutionContext {
            generation: 7,
            request_id: 19,
        };
        let journal_dir = journal_dir_for(&state.attempts_root, execution, 42);
        let mut journal = prepare_mutation_journal(
            &state,
            execution,
            &journal_dir,
            &test_call_request(
                "sess-journal-g7-r19-42",
                "fs.edit",
                json!({"path": "a.txt"}),
            ),
            EngineIdentity::FsZero,
            EffectClass::ReversibleMutation,
        )
        .expect("prepare is durable before admission");

        let prepared = read_current_attempt_v1(&journal.paths)
            .expect("read journal")
            .expect("prepared entry present");
        assert_eq!(prepared.state, AttemptStateV1::Prepared);
        assert_eq!(prepared.sequence, 1);
        assert!(journal.dispatch_entry_digest.is_none());

        cross_mutation_journal(&mut journal).expect("dispatch boundary persists");
        let crossed = read_current_attempt_v1(&journal.paths)
            .expect("read journal")
            .expect("crossed entry present");
        assert_eq!(crossed.state, AttemptStateV1::DispatchCrossed);
        assert_eq!(crossed.sequence, 2);
        assert!(journal.dispatch_entry_digest.is_some());

        succeed_mutation_journal(&journal, attempt_digest(&json!({"ok": true})))
            .expect("completion evidence persists");
        let succeeded = read_current_attempt_v1(&journal.paths)
            .expect("read journal")
            .expect("terminal entry present");
        assert_eq!(succeeded.state, AttemptStateV1::Succeeded);
        assert_eq!(succeeded.sequence, 3);
    }

    #[test]
    fn prepared_journal_recovery_classifies_safe_to_retry_and_never_dispatches() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = journal_test_state(dir.path(), "sess-journal");
        let execution = AggregateExecutionContext {
            generation: 7,
            request_id: 20,
        };
        let journal_dir = journal_dir_for(&state.attempts_root, execution, 43);
        let journal = prepare_mutation_journal(
            &state,
            execution,
            &journal_dir,
            &test_call_request("sess-journal-g7-r20-43", "ingest", json!({"text": "x"})),
            EngineIdentity::TokenZero,
            EffectClass::Irreversible,
        )
        .expect("prepare");
        assert_eq!(journal.dispatch_entry_digest, None);

        let statuses = reconcile_request_attempts(&state.attempts_root, 7, 20).expect("reconcile");
        assert_eq!(statuses.len(), 1);
        assert_eq!(
            statuses[0].recovery.outcome,
            AttemptRecoveryOutcomeV1::ClassifiedSafeToRetry
        );
        assert_eq!(statuses[0].state, AttemptStateV1::SafeToRetry);
        assert_eq!(statuses[0].dispatch_id, "sess-journal-g7-r20-43");
        assert_eq!(statuses[0].operation.as_deref(), Some("ingest"));
        assert_eq!(statuses[0].effect_class, Some(EffectClass::Irreversible));
        eprintln!(
            "CHAR reconcile request=20 status={:?} adapter_calls=0",
            statuses[0].state
        );

        // Recovery is idempotent and the journal can never cross dispatch.
        let again = reconcile_request_attempts(&state.attempts_root, 7, 20).expect("reconcile");
        assert_eq!(
            again[0].recovery.outcome,
            AttemptRecoveryOutcomeV1::AlreadySafeToRetry
        );
        assert!(
            mark_dispatch_crossed_v1(&journal.paths, journal.prepared_entry_digest, 1,).is_err()
        );
    }

    #[test]
    fn crossed_journal_without_evidence_recovery_classifies_indeterminate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = journal_test_state(dir.path(), "sess-journal");
        let execution = AggregateExecutionContext {
            generation: 7,
            request_id: 21,
        };
        let journal_dir = journal_dir_for(&state.attempts_root, execution, 44);
        let mut journal = prepare_mutation_journal(
            &state,
            execution,
            &journal_dir,
            &test_call_request(
                "sess-journal-g7-r21-44",
                "remember",
                json!({"text": "fact"}),
            ),
            EngineIdentity::GraphZero,
            EffectClass::ReversibleMutation,
        )
        .expect("prepare");
        cross_mutation_journal(&mut journal).expect("cross");

        let statuses = reconcile_request_attempts(&state.attempts_root, 7, 21).expect("reconcile");
        assert_eq!(statuses.len(), 1);
        assert_eq!(
            statuses[0].recovery.outcome,
            AttemptRecoveryOutcomeV1::ClassifiedIndeterminate
        );
        assert_eq!(statuses[0].state, AttemptStateV1::Indeterminate);
        assert_eq!(statuses[0].operation.as_deref(), Some("remember"));

        // A recovered journal is terminal: no transition can redispatch it.
        assert!(
            mark_dispatch_crossed_v1(&journal.paths, journal.prepared_entry_digest, 1,).is_err()
        );
    }

    #[test]
    fn all_attempt_recovery_classifies_every_request_without_redispatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = journal_test_state(dir.path(), "sess-journal");
        let prepared_execution = AggregateExecutionContext {
            generation: 7,
            request_id: 22,
        };
        let prepared_dir = journal_dir_for(&state.attempts_root, prepared_execution, 45);
        let prepared = prepare_mutation_journal(
            &state,
            prepared_execution,
            &prepared_dir,
            &test_call_request("sess-journal-g7-r22-45", "fs.edit", json!({"path": "a"})),
            EngineIdentity::FsZero,
            EffectClass::ReversibleMutation,
        )
        .expect("prepare first request");

        let crossed_execution = AggregateExecutionContext {
            generation: 8,
            request_id: 1,
        };
        let crossed_dir = journal_dir_for(&state.attempts_root, crossed_execution, 46);
        let mut crossed = prepare_mutation_journal(
            &state,
            crossed_execution,
            &crossed_dir,
            &test_call_request("sess-journal-g8-r1-46", "remember", json!({"text": "x"})),
            EngineIdentity::GraphZero,
            EffectClass::ReversibleMutation,
        )
        .expect("prepare second request");
        cross_mutation_journal(&mut crossed).expect("cross second request");

        let statuses = reconcile_all_attempts(&state.attempts_root).expect("reconcile all");
        assert_eq!(statuses.len(), 2);
        assert_eq!((statuses[0].generation, statuses[0].request_id), (7, 22));
        assert_eq!(statuses[0].state, AttemptStateV1::SafeToRetry);
        assert_eq!((statuses[1].generation, statuses[1].request_id), (8, 1));
        assert_eq!(statuses[1].state, AttemptStateV1::Indeterminate);

        assert!(
            mark_dispatch_crossed_v1(&prepared.paths, prepared.prepared_entry_digest, 1).is_err()
        );
        assert!(
            mark_dispatch_crossed_v1(&crossed.paths, crossed.prepared_entry_digest, 1).is_err()
        );
    }

    #[test]
    fn attempts_root_is_stable_under_the_session_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let first = attempts_root_for(&root);
        let second = attempts_root_for(&root);
        assert_eq!(first, second);
        assert_eq!(first, root.join("attempts"));
    }

    #[test]
    fn distinct_state_root_contains_attempts_cas_and_gc_publication() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace_root = dir.path().join("workspace");
        let state_root = dir.path().join("state");
        std::fs::create_dir_all(&workspace_root).expect("workspace root");
        std::fs::create_dir_all(&state_root).expect("state root");

        let connector = ZsxConnector::new_with_state_root(
            workspace_root.clone(),
            state_root.clone(),
            "sess-state-root".to_owned(),
            BTreeMap::new(),
        )
        .expect("connector");

        assert_eq!(connector.state.workspace_root, workspace_root);
        assert_eq!(connector.state.state_root, state_root);
        assert_eq!(connector.state.attempts_root, state_root.join("attempts"));
        let cas = SharedCas::open(&state_root);
        let hash = cas.put(b"state-root-only ref").expect("publish CAS object");
        retain_reachability(
            &connector.state,
            EngineIdentity::FsZero,
            &[format!("fz://blob/{hash}")],
        )
        .expect("retain state-root ref");
        connector
            .publish_reachability()
            .expect("publish reachability");

        assert!(!workspace_root.join("gc").exists());
        assert!(!workspace_root.join("blobs").exists());
        let project_id = gc_project_id(&state_root).expect("project identity");
        for producer in ["fszero", "graphzero", "tokenzero"] {
            let snapshot = current_reachability_snapshot(&state_root, producer, &project_id)
                .expect("read reachability")
                .unwrap_or_else(|| panic!("missing {producer} reachability under state root"));
            if producer == "fszero" {
                assert_eq!(snapshot.blob_hashes, vec![hash.clone()]);
            } else {
                assert!(snapshot.blob_hashes.is_empty());
            }
        }
    }
