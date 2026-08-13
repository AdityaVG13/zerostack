    use super::*;
    use zero_abi::raw_worker::EngineIdentity;

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
