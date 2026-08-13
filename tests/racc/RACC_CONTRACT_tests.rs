    use super::*;

    fn d(x: u8) -> Digest { Digest([x; 32]) }

    #[test]
    fn checks_97_percent_saving() {
        let receipt = DominanceReceipt {
            ledger: TokenLedger {
                raw_input_tokens: 1_000_000,
                racc_input_tokens: 30_000,
                model_output_tokens: 0,
                model_calls: 1,
                fallback_tokens: 0,
            },
            target_retained_ppm: RetainedFractionPpm(30_000),
            archive_root: d(1),
            certificate_root: d(2),
            byte_exact: true,
            policy_exact_or_fallback: true,
            task_verified: true,
        };
        assert!(receipt.exact_phase_valid());
    }

    #[test]
    fn rejects_unproven_exactness() {
        let receipt = DominanceReceipt {
            ledger: TokenLedger {
                raw_input_tokens: 1_000_000,
                racc_input_tokens: 1_000,
                model_output_tokens: 0,
                model_calls: 1,
                fallback_tokens: 0,
            },
            target_retained_ppm: RetainedFractionPpm(1_000),
            archive_root: d(1),
            certificate_root: d(2),
            byte_exact: true,
            policy_exact_or_fallback: false,
            task_verified: true,
        };
        assert!(!receipt.exact_phase_valid());
    }
