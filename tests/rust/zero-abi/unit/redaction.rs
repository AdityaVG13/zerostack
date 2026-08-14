    use super::*;

    use serde_json::json;
    use crate::EffectClass;

    fn policy(secrets: &[&str]) -> RedactionPolicyV1 {
        RedactionPolicyV1::new(
            secrets.iter().map(|value| value.to_string()).collect(),
            DEFAULT_REDACTION_TOKEN,
        )
        .unwrap()
    }

    /// ZS-SEC-004 acceptance: secrets never appear in provider prompt, UI
    /// export, benchmark trace, or error strings after redaction.
    #[test]
    fn secrets_never_leak_after_redaction() {
        let redactor = RedactorV1::new(policy(&["sk-live-abc123", "password"])).unwrap();
        let value = json!({
            "provider_prompt": "use your key sk-live-abc123 now",
            "nested": {
                "export": ["sk-live-abc123", "safe"],
                "credential_key": "password"
            },
            "error": "auth failed with password"
        });
        let redacted = redactor.redact(&value);
        redactor
            .check_no_leak(&redacted)
            .expect("no secret may survive redaction");
        let text = redacted.to_string();
        assert!(!text.contains("sk-live-abc123"));
        assert!(!text.contains("password"));
        assert!(text.contains(DEFAULT_REDACTION_TOKEN));
        // The surrounding structure survives.
        assert_eq!(redacted["nested"]["export"][1], json!("safe"));
        assert_eq!(redacted["provider_prompt"], json!(format!("use your key {} now", DEFAULT_REDACTION_TOKEN)));
    }

    /// Redaction covers object keys too, and a redacted value is idempotent.
    #[test]
    fn redaction_covers_keys_and_is_idempotent() {
        let redactor = RedactorV1::new(policy(&["token"])).unwrap();
        let value = json!({"token_field": "value-with-token", "ok": [1, 2]});
        let once = redactor.redact(&value);
        redactor.check_no_leak(&once).unwrap();
        assert!(!once.to_string().contains("token"));
        let twice = redactor.redact(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn redaction_policy_validation_fails_closed() {
        // Empty token rejected; empty and duplicate secrets rejected; a
        // secret equal to the token would make redaction non-total or
        // loop, so it is rejected fail-closed.
        assert!(RedactionPolicyV1::new(vec!["a".into()], "").is_err());
        assert!(RedactionPolicyV1::new(vec![String::new()], "[R]").is_err());
        assert!(RedactionPolicyV1::new(vec!["a".into(), "a".into()], "[R]").is_err());
        assert!(RedactionPolicyV1::new(vec!["[R]".into()], "[R]").is_err());
        let empty = policy(&[]);
        assert!(empty.is_empty());
        // Redacting with no secrets changes nothing.
        let redactor = RedactorV1::new(empty).unwrap();
        let value = json!("anything");
        assert_eq!(redactor.redact(&value), value);
    }

    /// The plain-text emission helper scrubs every occurrence and fails
    /// closed; it is the primitive behind host error-string redaction.
    #[test]
    fn redact_text_checked_scrubs_and_fails_closed() {
        let redactor = RedactorV1::new(policy(&["sk-live-abc123"])).unwrap();
        let text = "worker crashed with ZEROSTACK_SESSION_TOKEN=sk-live-abc123";
        let redacted = redactor.redact_text_checked(text).unwrap();
        assert!(!redacted.contains("sk-live-abc123"));
        assert!(redacted.contains(DEFAULT_REDACTION_TOKEN));
        // Idempotent on the scrubbed form.
        assert_eq!(
            redactor.redact_text_checked(&redacted).unwrap(),
            redacted
        );
    }

    /// ZS-STORE-004 acceptance: an undeclared effect during candidate
    /// execution is blocked or Unsafe -- the verdict is never Safe.
    #[test]
    fn undeclared_effects_yield_unknown_never_safe() {
        fn read() -> TypedEffectOperationV1 {
            TypedEffectOperationV1::ReturnLiteral {
                bytes: b"data".to_vec(),
                payload_digest: crate::DigestV1::from_bytes([1; 32]),
            }
        }
        fn network() -> TypedEffectOperationV1 {
            TypedEffectOperationV1::InvokeCapability {
                owner: crate::ArtifactOwnerV1::PiZeroStack,
                capability: "network.fetch".into(),
                generation: 1,
                capability_contract_digest: crate::DigestV1::from_bytes([2; 32]),
                arguments_digest: crate::DigestV1::from_bytes([3; 32]),
                effect_class: EffectClass::ReadOnly,
            }
        }
        fn spawn() -> TypedEffectOperationV1 {
            TypedEffectOperationV1::InvokeCapability {
                owner: crate::ArtifactOwnerV1::ZeroStack,
                capability: "process.spawn".into(),
                generation: 1,
                capability_contract_digest: crate::DigestV1::from_bytes([4; 32]),
                arguments_digest: crate::DigestV1::from_bytes([5; 32]),
                effect_class: EffectClass::ApprovalRequiredMutation,
            }
        }

        // Declared == observed: Safe.
        let clean = EffectTraceV1::new(vec![read()], vec![read()]).unwrap();
        assert_eq!(clean.verdict(), SafetyVerdictV1::Safe);
        assert!(clean.undeclared_effects().is_empty());

        // Observed network fetch was NOT declared: Unknown, never Safe.
        let network_undeclared = EffectTraceV1::new(vec![read()], vec![read(), network()]).unwrap();
        assert_eq!(network_undeclared.undeclared_effects().len(), 1);
        assert!(matches!(
            network_undeclared.verdict(),
            SafetyVerdictV1::Unknown { .. }
        ));

        // Process spawn with a different capability digest than declared:
        // the observed operation is not the declared one -> Unknown.
        let mut declared_spawn = spawn();
        let TypedEffectOperationV1::InvokeCapability { arguments_digest, .. } = &mut declared_spawn else {
            unreachable!()
        };
        *arguments_digest = crate::DigestV1::from_bytes([9; 32]);
        let drifted = EffectTraceV1::new(vec![declared_spawn], vec![spawn()]).unwrap();
        assert!(matches!(drifted.verdict(), SafetyVerdictV1::Unknown { .. }));

        // Multiple undeclared effects: reasons are deterministic.
        let multi = EffectTraceV1::new(vec![read()], vec![read(), network(), spawn()]).unwrap();
        match multi.verdict() {
            SafetyVerdictV1::Unknown { reasons } => assert_eq!(reasons.len(), 2),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn artifact_owner_enum_is_exhaustive() {
        let _ = crate::ArtifactOwnerV1::PiZeroStack;
    }
