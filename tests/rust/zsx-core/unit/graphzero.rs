    use super::*;
    use zero_abi::WorkerTrace;

    fn request() -> CallRequest {
        CallRequest {
            request_id: "graph-cas-bridge".into(),
            op: "verify".into(),
            args: serde_json::json!({}),
            deadline_unix_ms: None,
            trace: WorkerTrace {
                runtime_id: "test".into(),
                cell_id: "test".into(),
                request_id: "graph-cas-bridge".into(),
                trace_id: "graph-cas-bridge".into(),
                parent_span_id: None,
                worker_revision: "test".into(),
                contract_digest: "0".repeat(64),
            },
            approval_grant: None,
            telemetry_request: None,
        }
    }

    #[test]
    fn bridges_legacy_graph_blob_into_aggregate_shared_cas() {
        let repo = tempfile::tempdir().expect("repo");
        let state = tempfile::tempdir().expect("state");
        let adapter = GraphZeroAdapter::new_with_state_root(
            repo.path(),
            state.path(),
            "graph-cas-bridge",
        );
        let bytes = b"graphzero aggregate CAS bridge";
        let reference = adapter
            .embedded()
            .put_blob(bytes)
            .expect("publish graph blob")
            .gz_ref;

        adapter
            .bridge_blob_refs(std::slice::from_ref(&reference), &request())
            .expect("bridge ref");

        let hash = reference
            .strip_prefix("gz://blob/")
            .expect("blob reference");
        assert_eq!(
            SharedCas::open(state.path())
                .get_verified(hash)
                .expect("aggregate CAS object"),
            bytes
        );
    }

    #[test]
    fn reserve_list_is_read_only_but_reserve_mutations_remain_irreversible() {
        assert_eq!(
            effect_class_for_request("reserve", &serde_json::json!({"action":"list"})),
            EffectClass::ReadOnly
        );
        assert_eq!(
            effect_class_for_request("reserve", &serde_json::json!({"action":"declare"})),
            EffectClass::Irreversible
        );
    }

    #[test]
    fn collects_fragment_refs_embedded_in_graph_results() {
        let reference = format!("gz://blob/{}#B16-35", "a".repeat(64));
        let value = serde_json::json!({"nested":[{"ref":reference}]});
        let mut refs = Vec::new();
        collect_graph_blob_refs(&value, &mut refs);
        assert_eq!(refs, vec![reference]);
    }
