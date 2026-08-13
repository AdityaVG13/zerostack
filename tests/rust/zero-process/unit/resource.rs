    use super::*;

    #[test]
    fn aggregate_defaults_are_exact_and_bounded() {
        let policy = ProcessResourcePolicy::active_default().validate().unwrap();
        assert_eq!(policy.idle_tree_rss_bytes, 96 * 1024 * 1024);
        assert_eq!(policy.active_tree_rss_bytes, 256 * 1024 * 1024);
        let receipt = ResourceReceipt::for_policy(policy);
        assert_eq!(receipt.schema, "zerostack.process.resource_receipt.v1");
        if cfg!(any(windows, unix)) {
            assert_ne!(receipt.enforcement, ResourceEnforcement::Unsupported);
        } else {
            assert_eq!(receipt.enforcement, ResourceEnforcement::Unsupported);
        }
    }

    #[test]
    fn three_worker_shares_stay_within_aggregate_budget() {
        let aggregate = ProcessResourcePolicy::active_default();
        let worker = aggregate.share(3).unwrap();
        assert!(worker.idle_tree_rss_bytes * 3 <= aggregate.idle_tree_rss_bytes);
        assert!(worker.active_tree_rss_bytes * 3 <= aggregate.active_tree_rss_bytes);
        assert!(
            worker.active_tree_rss_bytes * 3 + worker.idle_tree_rss_bytes * 2
                <= aggregate.active_tree_rss_bytes
        );
        assert!(worker.cpu_seconds * 3 <= aggregate.cpu_seconds);
    }

    #[test]
    fn oversized_or_inverted_profiles_fail_closed() {
        assert!(
            ProcessResourcePolicy {
                idle_tree_rss_bytes: DEFAULT_ACTIVE_TREE_RSS_BYTES,
                active_tree_rss_bytes: DEFAULT_IDLE_TREE_RSS_BYTES,
                cpu_seconds: 1,
            }
            .validate()
            .is_err()
        );
        assert!(
            ProcessResourcePolicy {
                active_tree_rss_bytes: DEFAULT_ACTIVE_TREE_RSS_BYTES + 1,
                ..ProcessResourcePolicy::default()
            }
            .validate()
            .is_err()
        );
    }
