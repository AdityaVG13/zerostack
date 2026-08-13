    use super::*;

    fn digest(byte: u8) -> SemanticCutDigestV1 {
        [byte; 32]
    }

    fn safepoint(receipt: u8) -> ReasoningSafepointV1 {
        ReasoningSafepointV1::new(
            digest(1),
            digest(2),
            digest(3),
            digest(4),
            digest(5),
            digest(6),
            ReasoningStateStatusV1::ExactPreserved,
            digest(7),
            digest(8),
            digest(9),
            digest(10),
            digest(receipt),
        )
        .unwrap()
    }

    fn exact_claim() -> SemanticCutClaimV1 {
        SemanticCutClaimV1::new_exact(
            digest(11),
            digest(12),
            digest(13),
            safepoint(14),
            safepoint(15),
            digest(16),
            digest(16),
            digest(17),
            digest(17),
            digest(18),
            digest(19),
            digest(20),
            digest(21),
            digest(22),
        )
        .unwrap()
    }

    #[test]
    fn exact_epoch_claim_is_canonical_and_receipt_heads_may_differ() {
        let claim = exact_claim();
        claim.validate_exact().unwrap();
        assert_ne!(
            claim.baseline_terminal.receipt_head_digest(),
            claim.compiled_terminal.receipt_head_digest()
        );
        let bytes = claim.canonical_bytes().unwrap();
        assert_eq!(
            SemanticCutClaimV1::from_canonical_bytes(&bytes).unwrap(),
            claim
        );
        let mut noncanonical = bytes;
        noncanonical.push(b'\n');
        assert_eq!(
            SemanticCutClaimV1::from_canonical_bytes(&noncanonical)
                .unwrap_err()
                .failure_code(),
            SemanticCutFailureCodeV1::NonCanonicalEncoding
        );
    }

    #[test]
    fn clean_restart_and_approximate_state_never_mint_exact_claims() {
        for status in [
            ReasoningStateStatusV1::ExactCleanRestart,
            ReasoningStateStatusV1::ScopedEquivalent,
            ReasoningStateStatusV1::Approximate,
            ReasoningStateStatusV1::Unavailable,
            ReasoningStateStatusV1::Expired,
            ReasoningStateStatusV1::IdentityMismatch,
        ] {
            let mut claim = exact_claim();
            claim.compiled_terminal.reasoning_state_status = status;
            assert_eq!(
                claim.validate_exact().unwrap_err().failure_code(),
                SemanticCutFailureCodeV1::ContinuationNotExact
            );
        }
    }

    #[test]
    fn every_protected_terminal_relation_fails_closed() {
        let mut claim = exact_claim();
        claim.compiled_terminal.opaque_reasoning_state_digest = digest(99);
        assert_eq!(
            claim.validate_exact().unwrap_err().failure_code(),
            SemanticCutFailureCodeV1::TerminalStateMismatch
        );
        let mut claim = exact_claim();
        claim.compiled_external_effects_digest = digest(99);
        assert_eq!(
            claim.validate_exact().unwrap_err().failure_code(),
            SemanticCutFailureCodeV1::ExternalEffectMismatch
        );
        let mut claim = exact_claim();
        claim.compiled_attribution_identity_digest = digest(99);
        assert_eq!(
            claim.validate_exact().unwrap_err().failure_code(),
            SemanticCutFailureCodeV1::AttributionMismatch
        );
        let mut claim = exact_claim();
        claim.semantic_authority = SemanticAuthorityV1::TaskSemanticSelection;
        assert_eq!(
            claim.validate_exact().unwrap_err().failure_code(),
            SemanticCutFailureCodeV1::SemanticAuthorityCrossing
        );
    }

    #[test]
    fn contract_and_claim_digests_are_stable() {
        assert_eq!(
            hex(&semantic_cut_contract_digest_v1()),
            "5701b3a000c045c39d86886801c7abbc9f4cf651b20ba47ba8ac3964fce88c6a"
        );
        assert_eq!(
            hex(&exact_claim().digest().unwrap()),
            "249d8029a25780f819b1c70dbbfd04faaaef7adbc0997a365c7c967599a83894"
        );
    }

    fn hex(bytes: &SemanticCutDigestV1) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
